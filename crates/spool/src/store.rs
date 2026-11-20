//! Identity state in DuckDB: the changeset log and the projection tables.
//!
//! The rules live in `spool-core`. This module answers the validator's
//! questions with one query each, and writes a validated changeset's log rows
//! and projection rows in one transaction.

use std::path::Path;
use std::str::FromStr;

use duckdb::Connection;
#[cfg(test)]
use duckdb::params;
use duckdb::vtab::arrow::ArrowVTab;
use spool_core::{
    ChangesetId, Clock, Committed, Constraint, EntityType, IdentityEntry, IdentityId,
    IdentityState, IdentityStore, LogEntry, RecordId, Seq, Snapshot, StateError, Validated,
};

use crate::error::{Error, Result};
use crate::schema;

mod log;
mod ops;
mod projection;
mod write;

pub(crate) use log::{
    log_entry, read_changeset_seq, read_head, read_log, read_log_after, read_log_ops,
    read_merged_before,
};
pub(crate) use ops::{OpRow, op_from_row, op_rows};
pub(crate) use projection::{
    constraint_from_row, read_assignments, read_constraints_of_members, read_constraints_on,
    read_identities, read_left_at, read_members, read_membership_changed, read_record_types,
    read_snapshot,
};

pub(crate) struct DuckStore {
    pub(crate) conn: Connection,
    pub(crate) clock: Box<dyn Clock + Send>,
    /// Whether the database is a file attached under [`CATALOG`].
    attached: bool,
}

/// The name the database file is attached under.
const CATALOG: &str = "spool_store";

impl DuckStore {
    /// Opens the database file at `path`, or a database in memory, and
    /// creates or upgrades the schema.
    pub(crate) fn open(path: Option<&Path>, clock: Box<dyn Clock + Send>) -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        if let Some(path) = path {
            // DuckDB names a database after its file, and a file called
            // `spool.duckdb` would make every `spool.<table>` ambiguous
            // between the database and the schema. Attaching under a fixed
            // name avoids that whatever the file is called.
            let file = path.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("ATTACH '{file}' AS {CATALOG}; USE {CATALOG};"))
                .map_err(|error| in_use(path, error))?;
        }
        // Timestamps are stored as instants. A fixed session zone keeps their
        // text form the same on every machine.
        conn.execute_batch("SET TimeZone = 'UTC'")?;
        conn.register_table_function::<ArrowVTab>("arrow")?;
        schema::ensure(&mut conn)?;
        Ok(Self {
            conn,
            clock,
            attached: path.is_some(),
        })
    }

    /// Whether the database is a file.
    pub(crate) fn attached(&self) -> bool {
        self.attached
    }

    /// Another connection to the same database, for reading while this one
    /// writes. It sees what has been committed.
    pub(crate) fn reader(&self) -> Result<Connection> {
        open_reader(&self.conn, self.attached)
    }
}

/// A store's file held the way a reader holds it: no server opens the
/// store until this is dropped, and nothing is written to the file.
pub struct StoreHold {
    _conn: Connection,
}

/// Holds the store's file, so that it can be removed while no server can
/// open it. [`Error::StoreInUse`] if a server has it open. A file that
/// DuckDB cannot read as a database is another error.
pub fn hold_store(path: &Path) -> Result<StoreHold> {
    let conn = Connection::open_in_memory()?;
    let file = path.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("ATTACH '{file}' AS {CATALOG} (READ_ONLY)"))
        .map_err(|error| in_use(path, error))?;
    Ok(StoreHold { _conn: conn })
}

/// What opening the file at `path` failed with. DuckDB lets one process
/// write to a database file, and refuses a second in words of its own: the
/// file, the program and process that has it, and a page of DuckDB's
/// documentation. That refusal becomes [`Error::StoreInUse`], and anything
/// else stays as it is.
fn in_use(path: &Path, error: duckdb::Error) -> Error {
    match lock_holder(&error.to_string()) {
        Some(holder) => Error::StoreInUse {
            path: path.to_owned(),
            holder,
        },
        None => error.into(),
    }
}

/// Reads DuckDB's refusal of a file that another process has. `None` if
/// `message` is not that refusal, and `Some(None)` if it is and does not
/// say which process: the system does not always tell.
fn lock_holder(message: &str) -> Option<Option<u32>> {
    let (_, held) = message.split_once("Could not set lock on file")?;
    // The file's path comes first and may hold anything, so the process is
    // read from the end.
    let holder = held
        .rsplit_once("(PID ")
        .and_then(|(_, rest)| rest.split_once(')'))
        .and_then(|(pid, _)| pid.trim().parse().ok());
    Some(holder)
}

/// A new connection to the database of `conn`, set up like the one a store
/// opens.
pub(crate) fn open_reader(conn: &Connection, attached: bool) -> Result<Connection> {
    let reader = conn.try_clone()?;
    if attached {
        reader.execute_batch(&format!("USE {CATALOG}"))?;
    }
    reader.execute_batch("SET TimeZone = 'UTC'")?;
    Ok(reader)
}

pub(crate) fn parse<T>(text: &str) -> Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    Ok(text.parse().map_err(StateError::new)?)
}

fn state_error(error: crate::error::Error) -> StateError {
    StateError::new(error)
}

impl IdentityState for DuckStore {
    fn head(&self) -> Result<Seq, StateError> {
        read_head(&self.conn).map_err(state_error)
    }

    fn changeset_seq(&self, id: ChangesetId) -> Result<Option<Seq>, StateError> {
        read_changeset_seq(&self.conn, id).map_err(state_error)
    }

    fn record_types(&self, records: &[RecordId]) -> Result<Vec<Option<EntityType>>, StateError> {
        read_record_types(&self.conn, records).map_err(state_error)
    }

    fn identities(&self, ids: &[IdentityId]) -> Result<Vec<Option<IdentityEntry>>, StateError> {
        read_identities(&self.conn, ids).map_err(state_error)
    }

    fn assignments(&self, records: &[RecordId]) -> Result<Vec<Option<IdentityId>>, StateError> {
        read_assignments(&self.conn, records).map_err(state_error)
    }

    fn members(&self, ids: &[IdentityId]) -> Result<Vec<(IdentityId, RecordId)>, StateError> {
        read_members(&self.conn, ids).map_err(state_error)
    }

    fn membership_changed(&self, ids: &[IdentityId]) -> Result<Vec<Seq>, StateError> {
        read_membership_changed(&self.conn, ids).map_err(state_error)
    }

    fn constraints_on(&self, records: &[RecordId]) -> Result<Vec<Constraint>, StateError> {
        read_constraints_on(&self.conn, records).map_err(state_error)
    }

    fn constraints_of_members(&self, ids: &[IdentityId]) -> Result<Vec<Constraint>, StateError> {
        read_constraints_of_members(&self.conn, ids).map_err(state_error)
    }

    fn left_at(&self, identity: IdentityId, seq: Seq) -> Result<Vec<RecordId>, StateError> {
        read_left_at(&self.conn, identity, seq).map_err(state_error)
    }

    fn merged_before(
        &self,
        identity: IdentityId,
        seq: Seq,
    ) -> Result<Option<IdentityId>, StateError> {
        read_merged_before(&self.conn, identity, seq).map_err(state_error)
    }
}

impl IdentityStore for DuckStore {
    fn apply(&mut self, changeset: Validated) -> Result<Committed, StateError> {
        let seq = self
            .append(changeset.changeset(), changeset.content_hash())
            .map_err(state_error)?;
        Ok(Committed {
            seq,
            changeset: changeset.changeset().id,
            content_hash: changeset.content_hash().clone(),
        })
    }

    fn log(&self) -> Result<Vec<LogEntry>, StateError> {
        read_log(&self.conn).map_err(state_error)
    }

    fn snapshot(&self) -> Result<Snapshot, StateError> {
        read_snapshot(&self.conn).map_err(state_error)
    }
}

/// Used by tests in this crate to put records in a store without going
/// through load.
#[cfg(test)]
pub(crate) fn insert_records(conn: &Connection, records: &[(RecordId, EntityType)]) -> Result<()> {
    for (id, entity_type) in records {
        conn.execute(
            "INSERT INTO spool.record
             VALUES (?::UUID, ?, 'test', NULL, NULL, '{}', NULL, NULL, make_timestamptz(0), 0)",
            params![id.to_string(), entity_type.as_str()],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use spool_core::{Actor, Changeset, ConstraintKind, Op, RunId, StepClock, conformance_tests};

    use super::*;

    fn store(records: &[(RecordId, EntityType)]) -> DuckStore {
        let store = DuckStore::open(None, Box::new(StepClock::new(0, 1_000_000))).unwrap();
        insert_records(&store.conn, records).unwrap();
        store
    }

    conformance_tests!(store);

    /// The smaller ID of a pair is picked in SQL when a changeset is applied
    /// and in Rust when the log is replayed. The two have to agree.
    #[test]
    fn duckdb_orders_ids_as_the_core_does() {
        let store = store(&[]);
        let ids: Vec<u128> = vec![
            0,
            1,
            0x7fff_ffff_ffff_ffff_ffff_ffff_ffff_ffff,
            0x8000_0000_0000_0000_0000_0000_0000_0000,
            0x8000_0000_0000_0000_0000_0000_0000_0001,
            0x0123_4567_89ab_cdef_0123_4567_89ab_cdef,
            0xfedc_ba98_7654_3210_fedc_ba98_7654_3210,
            u128::MAX,
        ];
        for a in &ids {
            for b in &ids {
                let (a, b) = (RecordId::from_u128(*a), RecordId::from_u128(*b));
                let smaller: String = store
                    .conn
                    .query_row(
                        "SELECT least(?::UUID, ?::UUID)::VARCHAR",
                        params![a.to_string(), b.to_string()],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(smaller, a.min(b).to_string(), "{a} and {b}");
                assert_eq!(Constraint::new(ConstraintKind::MustLink, a, b).a, a.min(b));
            }
        }
    }

    #[test]
    fn a_store_reopens_with_its_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.duckdb");
        let widget = EntityType::new("widget").unwrap();
        let record = RecordId::from_u128(1);
        let mut ids = spool_core::SeededIds::new(5);
        use spool_core::IdGenerator;
        let identity = ids.identity_id();

        let before = {
            let mut store =
                DuckStore::open(Some(&path), Box::new(StepClock::new(0, 1_000_000))).unwrap();
            insert_records(&store.conn, &[(record, widget.clone())]).unwrap();
            let changeset = Changeset {
                id: ids.changeset_id(),
                base: Seq::ZERO,
                actor: Actor::Engine,
                reason: None,
                run: Some(RunId(7)),
                ops: vec![
                    Op::CreateIdentity {
                        identity,
                        entity_type: widget,
                    },
                    Op::Assign {
                        record,
                        from: None,
                        to: Some(identity),
                    },
                ],
            };
            let validated = spool_core::validate(&store, changeset).unwrap();
            store.apply(validated).unwrap();
            (store.snapshot().unwrap(), store.log().unwrap())
        };

        let store = DuckStore::open(Some(&path), Box::new(StepClock::new(0, 1_000_000))).unwrap();
        assert_eq!(store.snapshot().unwrap(), before.0);
        assert_eq!(store.log().unwrap(), before.1);
        assert_eq!(store.log().unwrap()[0].changeset.run, Some(RunId(7)));
    }

    #[test]
    fn a_refusal_of_a_store_another_process_has_is_told_from_other_failures() {
        // What DuckDB says on Linux, and the same without the process.
        let refusal = "IO Error: Could not set lock on file \"/tmp/a (PID 1)/dev.duckdb\": \
                       Conflicting lock is held in /usr/local/bin/spool (PID 7989). \
                       See also https://duckdb.org/docs/current/connect/concurrency";
        assert_eq!(lock_holder(refusal), Some(Some(7989)));
        assert_eq!(
            lock_holder(
                "IO Error: Could not set lock on file \"dev.duckdb\": Conflicting lock is held"
            ),
            Some(None)
        );
        assert_eq!(
            lock_holder("IO Error: Cannot open file \"dev.duckdb\": Permission denied"),
            None
        );

        let error = || duckdb::Error::InvalidParameterName(refusal.to_owned());
        assert!(matches!(
            in_use(Path::new("dev.duckdb"), error()),
            Error::StoreInUse { path, holder: Some(7989) } if path == Path::new("dev.duckdb")
        ));
        let other = duckdb::Error::InvalidParameterName("no such thing".to_owned());
        assert!(matches!(
            in_use(Path::new("dev.duckdb"), other),
            Error::Duckdb(duckdb::Error::InvalidParameterName(_))
        ));
    }

    #[test]
    fn a_store_with_an_unsupported_format_version_is_refused() {
        for other in ["10", "999"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("store.duckdb");
            {
                let store =
                    DuckStore::open(Some(&path), Box::new(StepClock::new(0, 1_000_000))).unwrap();
                schema::set_meta(&store.conn, "format_version", other).unwrap();
            }
            let error = DuckStore::open(Some(&path), Box::new(StepClock::new(0, 1)))
                .err()
                .unwrap();
            assert!(
                matches!(&error, Error::FormatVersion { found, expected }
                    if found == other && expected == schema::FORMAT_VERSION),
                "{error}"
            );
            // The store is as it was.
            let conn = duckdb::Connection::open(&path).unwrap();
            assert_eq!(
                schema::read_meta(&conn, "format_version").unwrap(),
                Some(other.to_owned())
            );
        }
    }

    #[test]
    fn a_failed_upgrade_leaves_the_store_at_its_original_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.duckdb");
        let open = || DuckStore::open(Some(&path), Box::new(StepClock::new(0, 1)));
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(include_str!("../migrations/011_baseline.sql"))
                .unwrap();
            conn.execute_batch("CREATE VIEW spool.extraction_found AS SELECT 1;")
                .unwrap();
            schema::set_meta(&conn, "format_version", "11").unwrap();
        }

        // The view blocks creation of the new table after the column is added.
        let Err(Error::Duckdb(error)) = open() else {
            panic!("expected the upgrade to fail");
        };
        assert!(error.to_string().contains("extraction_found"), "{error}");
        {
            let conn = Connection::open(&path).unwrap();
            assert_eq!(
                schema::read_meta(&conn, "format_version")
                    .unwrap()
                    .as_deref(),
                Some("11")
            );
            let columns: i64 = conn
                .query_row(
                    "SELECT count(*) FROM information_schema.columns
                     WHERE table_schema = 'spool' AND table_name = 'extraction'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(columns, 5);
            conn.execute_batch("DROP VIEW spool.extraction_found")
                .unwrap();
        }

        let store = open().unwrap();
        assert_eq!(
            schema::read_meta(&store.conn, "format_version")
                .unwrap()
                .as_deref(),
            Some(schema::FORMAT_VERSION)
        );
    }
}
