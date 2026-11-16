//! Identity state in DuckDB: the changeset log and the projection tables.
//!
//! The rules live in `spool-core`. This module answers the validator's
//! questions with one query each, and writes a validated changeset's log rows
//! and projection rows in one transaction.

use std::path::Path;
use std::str::FromStr;

use duckdb::vtab::arrow::{ArrowBatchRegistration, ArrowVTab};
use duckdb::{Connection, params};
use spool_core::{
    Changeset, ChangesetId, Clock, Committed, ConstraintKind, ContentHash, EntityType, IdentityId,
    IdentityRecord, IdentityState, IdentityStatus, IdentityStore, LogEntry, Membership,
    ObservationId, Op, RunId, Seq, Snapshot, StateError, Validated,
};

use crate::batch::{Column, batch, id_list};
use crate::drafts;
use crate::error::Result;
use crate::review;
use crate::schema;

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
    /// creates the schema if the database is empty.
    pub(crate) fn open(path: Option<&Path>, clock: Box<dyn Clock + Send>) -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        if let Some(path) = path {
            // DuckDB names a database after its file, and a file called
            // `spool.duckdb` would make every `spool.<table>` ambiguous
            // between the database and the schema. Attaching under a fixed
            // name avoids that whatever the file is called.
            let file = path.to_string_lossy().replace('\'', "''");
            conn.execute_batch(&format!("ATTACH '{file}' AS {CATALOG}; USE {CATALOG};"))?;
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
        reader(&self.conn, self.attached)
    }

    /// Replaces the projection tables with the given state. Every identity is
    /// written as active: nothing can merge identities yet.
    pub(crate) fn write_projection(&mut self, snapshot: &Snapshot) -> Result<()> {
        let identities = batch(vec![
            (
                "identity_id",
                text(snapshot.identities.iter().map(|record| Some(record.id))),
            ),
            (
                "entity_type",
                text(
                    snapshot
                        .identities
                        .iter()
                        .map(|record| Some(&record.entity_type)),
                ),
            ),
            (
                "created_seq",
                Column::Integer(
                    snapshot
                        .identities
                        .iter()
                        .map(|record| record.created.0 as i64)
                        .collect(),
                ),
            ),
        ])?;
        let memberships = batch(vec![
            (
                "observation_id",
                text(snapshot.memberships.iter().map(|row| Some(row.observation))),
            ),
            (
                "identity_id",
                text(snapshot.memberships.iter().map(|row| Some(row.identity))),
            ),
            (
                "from_seq",
                Column::Integer(
                    snapshot
                        .memberships
                        .iter()
                        .map(|row| row.from.0 as i64)
                        .collect(),
                ),
            ),
            (
                "to_seq",
                text(
                    snapshot
                        .memberships
                        .iter()
                        .map(|row| row.to.map(|seq| seq.0)),
                ),
            ),
        ])?;

        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM spool.identity; DELETE FROM spool.membership; DELETE FROM spool.constraint_link;",
        )?;
        tx.execute(
            "INSERT INTO spool.identity
             SELECT identity_id::UUID, entity_type, created_seq, NULL, NULL FROM arrow(?)",
            [&ArrowBatchRegistration::new(identities)],
        )?;
        tx.execute(
            "INSERT INTO spool.membership
             SELECT observation_id::UUID, identity_id::UUID, from_seq, to_seq::BIGINT FROM arrow(?)",
            [&ArrowBatchRegistration::new(memberships)],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn append(&mut self, changeset: &Changeset, content_hash: &ContentHash) -> Result<Seq> {
        let committed_at = self.clock.now_micros();
        let ops = op_rows(&changeset.ops)?;

        let tx = self.conn.transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT coalesce(max(seq), 0) + 1 FROM spool.changeset",
            [],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO spool.changeset
             VALUES (?, ?::UUID, ?, ?, ?, ?, ?, make_timestamptz(?))",
            params![
                seq,
                changeset.id.to_string(),
                changeset.base.0 as i64,
                changeset.actor.to_string(),
                changeset.reason,
                changeset.run.map(|run| run.0 as i64),
                content_hash.as_str(),
                committed_at,
            ],
        )?;
        tx.execute(
            "INSERT INTO spool.changeset_op
             SELECT ?, op_index::INTEGER, kind, identity_id::UUID, from_identity_id::UUID,
                    observation_id::UUID, other_observation_id::UUID, entity_type, constraint_kind
             FROM arrow(?)",
            params![seq, &ArrowBatchRegistration::new(ops)],
        )?;

        // The projection is derived from the log rows just written.
        tx.execute(
            "INSERT INTO spool.identity
             SELECT identity_id, entity_type, seq, NULL, NULL
             FROM spool.changeset_op WHERE seq = ? AND kind = 'create_identity'",
            [seq],
        )?;
        tx.execute(
            "UPDATE spool.membership SET to_seq = ?
             WHERE to_seq IS NULL AND observation_id IN (
               SELECT observation_id FROM spool.changeset_op
               WHERE seq = ? AND kind = 'assign' AND from_identity_id IS NOT NULL)",
            [seq, seq],
        )?;
        tx.execute(
            "INSERT INTO spool.membership
             SELECT observation_id, identity_id, seq, NULL
             FROM spool.changeset_op
             WHERE seq = ? AND kind = 'assign' AND identity_id IS NOT NULL",
            [seq],
        )?;
        // Not identity state, but it has to agree with membership at every
        // log position, so it is updated in the same transaction.
        review::resolve_assigned(&tx, Seq(seq as u64))?;
        // Nor is a draft, and committing one is what ends its match run.
        drafts::committed(&tx, changeset.id, Seq(seq as u64), committed_at)?;
        tx.commit()?;
        Ok(Seq(seq as u64))
    }
}

/// A new connection to the database of `conn`, set up like the one a store
/// opens.
pub(crate) fn reader(conn: &Connection, attached: bool) -> Result<Connection> {
    let reader = conn.try_clone()?;
    if attached {
        reader.execute_batch(&format!("USE {CATALOG}"))?;
    }
    reader.execute_batch("SET TimeZone = 'UTC'")?;
    Ok(reader)
}

pub(crate) fn read_log(conn: &Connection) -> Result<Vec<LogEntry>> {
    read_log_after(conn, Seq::ZERO, u64::MAX)
}

/// At most `limit` committed changesets after a log position, in log order.
pub(crate) fn read_log_after(conn: &Connection, after: Seq, limit: u64) -> Result<Vec<LogEntry>> {
    let mut entries = Vec::new();
    let mut statement = conn.prepare(
        "SELECT seq, changeset_id::VARCHAR, base_seq, actor, reason, run_id, content_hash
         FROM spool.changeset WHERE seq > ? ORDER BY seq LIMIT ?",
    )?;
    let mut rows = statement.query(params![after.0 as i64, limit.min(i64::MAX as u64) as i64])?;
    while let Some(row) = rows.next()? {
        entries.push(LogEntry {
            seq: Seq(row.get::<_, i64>(0)? as u64),
            changeset: Changeset {
                id: parse(&row.get::<_, String>(1)?)?,
                base: Seq(row.get::<_, i64>(2)? as u64),
                actor: parse(&row.get::<_, String>(3)?)?,
                reason: row.get(4)?,
                run: row.get::<_, Option<i64>>(5)?.map(|run| RunId(run as u64)),
                ops: Vec::new(),
            },
            content_hash: ContentHash::from_stored(row.get(6)?),
        });
    }
    let Some(last) = entries.last().map(|entry| entry.seq) else {
        return Ok(entries);
    };

    let mut statement = conn.prepare(
        "SELECT seq, kind, identity_id::VARCHAR, from_identity_id::VARCHAR,
                observation_id::VARCHAR, other_observation_id::VARCHAR,
                entity_type, constraint_kind
         FROM spool.changeset_op WHERE seq > ? AND seq <= ? ORDER BY seq, op_index",
    )?;
    let mut rows = statement.query([after.0 as i64, last.0 as i64])?;
    while let Some(row) = rows.next()? {
        let seq = Seq(row.get::<_, i64>(0)? as u64);
        let op = op_from_row(OpRow {
            kind: row.get(1)?,
            identity: row.get(2)?,
            from_identity: row.get(3)?,
            observation: row.get(4)?,
            other_observation: row.get(5)?,
            entity_type: row.get(6)?,
            constraint_kind: row.get(7)?,
        })?;
        // Positions are dense, so an entry's index is its distance from the
        // first position read.
        let entry = (seq.0 as usize)
            .checked_sub(after.0 as usize + 1)
            .and_then(|index| entries.get_mut(index))
            .filter(|entry| entry.seq == seq)
            .ok_or_else(|| {
                StateError::new(format!(
                    "operation rows at log position {seq} have no changeset"
                ))
            })?;
        entry.changeset.ops.push(op);
    }
    Ok(entries)
}

pub(crate) fn read_snapshot(conn: &Connection) -> Result<Snapshot> {
    let head = read_head(conn)?;

    let mut identities = Vec::new();
    let mut statement = conn.prepare(
        "SELECT identity_id::VARCHAR, entity_type, created_seq, merged_into::VARCHAR
         FROM spool.identity",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        identities.push(identity_record(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
        )?);
    }
    identities.sort_by_key(|record| record.id);

    let mut memberships = Vec::new();
    let mut statement = conn.prepare(
        "SELECT observation_id::VARCHAR, identity_id::VARCHAR, from_seq, to_seq
         FROM spool.membership",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        memberships.push(Membership {
            observation: parse(&row.get::<_, String>(0)?)?,
            identity: parse(&row.get::<_, String>(1)?)?,
            from: Seq(row.get::<_, i64>(2)? as u64),
            to: row.get::<_, Option<i64>>(3)?.map(|seq| Seq(seq as u64)),
        });
    }
    memberships.sort();

    Ok(Snapshot {
        head,
        identities,
        memberships,
    })
}

pub(crate) fn read_head(conn: &Connection) -> Result<Seq> {
    let head: i64 = conn.query_row(
        "SELECT coalesce(max(seq), 0) FROM spool.changeset",
        [],
        |row| row.get(0),
    )?;
    Ok(Seq(head as u64))
}

pub(crate) fn read_changeset_seq(conn: &Connection, id: ChangesetId) -> Result<Option<Seq>> {
    let mut statement =
        conn.prepare("SELECT seq FROM spool.changeset WHERE changeset_id = ?::UUID")?;
    let mut rows = statement.query([id.to_string()])?;
    Ok(match rows.next()? {
        Some(row) => Some(Seq(row.get::<_, i64>(0)? as u64)),
        None => None,
    })
}

pub(crate) fn read_observation_types(
    conn: &Connection,
    observations: &[ObservationId],
) -> Result<Vec<Option<EntityType>>> {
    if observations.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT o.entity_type
         FROM arrow(?) a LEFT JOIN spool.observation o ON o.observation_id = a.id::UUID
         ORDER BY a.ord",
    )?;
    let mut rows = statement.query([&id_list(observations)?])?;
    let mut types = Vec::with_capacity(observations.len());
    while let Some(row) = rows.next()? {
        types.push(match row.get::<_, Option<String>>(0)? {
            Some(name) => Some(EntityType::new(name).map_err(StateError::new)?),
            None => None,
        });
    }
    Ok(types)
}

pub(crate) fn read_identities(
    conn: &Connection,
    ids: &[IdentityId],
) -> Result<Vec<Option<IdentityRecord>>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT i.identity_id::VARCHAR, i.entity_type, i.created_seq, i.merged_into::VARCHAR
         FROM arrow(?) a LEFT JOIN spool.identity i ON i.identity_id = a.id::UUID
         ORDER BY a.ord",
    )?;
    let mut rows = statement.query([&id_list(ids)?])?;
    let mut records = Vec::with_capacity(ids.len());
    while let Some(row) = rows.next()? {
        records.push(match row.get::<_, Option<String>>(0)? {
            Some(id) => Some(identity_record(id, row.get(1)?, row.get(2)?, row.get(3)?)?),
            None => None,
        });
    }
    Ok(records)
}

pub(crate) fn read_assignments(
    conn: &Connection,
    observations: &[ObservationId],
) -> Result<Vec<Option<IdentityId>>> {
    if observations.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT m.identity_id::VARCHAR
         FROM arrow(?) a
         LEFT JOIN spool.membership m ON m.observation_id = a.id::UUID AND m.to_seq IS NULL
         ORDER BY a.ord",
    )?;
    let mut rows = statement.query([&id_list(observations)?])?;
    let mut assignments = Vec::with_capacity(observations.len());
    while let Some(row) = rows.next()? {
        assignments.push(match row.get::<_, Option<String>>(0)? {
            Some(id) => Some(parse(&id)?),
            None => None,
        });
    }
    Ok(assignments)
}

pub(crate) fn read_members(
    conn: &Connection,
    ids: &[IdentityId],
) -> Result<Vec<(IdentityId, ObservationId)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT m.identity_id::VARCHAR, m.observation_id::VARCHAR
         FROM spool.membership m
         WHERE m.to_seq IS NULL AND m.identity_id IN (SELECT id::UUID FROM arrow(?))",
    )?;
    let mut rows = statement.query([&id_list(ids)?])?;
    let mut members = Vec::new();
    while let Some(row) = rows.next()? {
        members.push((
            parse(&row.get::<_, String>(0)?)?,
            parse(&row.get::<_, String>(1)?)?,
        ));
    }
    members.sort_unstable();
    Ok(members)
}

fn text<T: ToString>(values: impl Iterator<Item = Option<T>>) -> Column {
    Column::Text(
        values
            .map(|value| value.map(|value| value.to_string()))
            .collect(),
    )
}

pub(crate) fn parse<T>(text: &str) -> Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    Ok(text.parse().map_err(StateError::new)?)
}

fn identity_record(
    id: String,
    entity_type: String,
    created_seq: i64,
    merged_into: Option<String>,
) -> Result<IdentityRecord> {
    Ok(IdentityRecord {
        id: parse(&id)?,
        entity_type: EntityType::new(entity_type).map_err(StateError::new)?,
        created: Seq(created_seq as u64),
        status: match merged_into {
            Some(survivor) => IdentityStatus::MergedInto(parse(&survivor)?),
            None => IdentityStatus::Active,
        },
    })
}

/// One row of `spool.changeset_op`, IDs as text.
#[derive(Default)]
pub(crate) struct OpRow {
    pub(crate) kind: String,
    pub(crate) identity: Option<String>,
    pub(crate) from_identity: Option<String>,
    pub(crate) observation: Option<String>,
    pub(crate) other_observation: Option<String>,
    pub(crate) entity_type: Option<String>,
    pub(crate) constraint_kind: Option<String>,
}

fn op_to_row(op: &Op) -> OpRow {
    let text = |value: &dyn ToString| Some(value.to_string());
    let row = OpRow {
        kind: op.kind().to_owned(),
        ..OpRow::default()
    };
    match op {
        Op::CreateIdentity {
            identity,
            entity_type,
        } => OpRow {
            identity: text(identity),
            entity_type: text(entity_type),
            ..row
        },
        Op::Assign {
            observation,
            from,
            to,
        } => OpRow {
            observation: text(observation),
            from_identity: from.map(|id| id.to_string()),
            identity: to.map(|id| id.to_string()),
            ..row
        },
        Op::Merge { from, into } => OpRow {
            from_identity: text(from),
            identity: text(into),
            ..row
        },
        Op::Constrain { kind, a, b } | Op::Unconstrain { kind, a, b } => OpRow {
            observation: text(a),
            other_observation: text(b),
            constraint_kind: Some(kind.as_str().to_owned()),
            ..row
        },
        Op::Reinstate { identity } => OpRow {
            identity: text(identity),
            ..row
        },
    }
}

pub(crate) fn op_from_row(row: OpRow) -> Result<Op> {
    fn required<T>(value: Option<String>, column: &str) -> Result<T>
    where
        T: FromStr,
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        let value = value.ok_or_else(|| {
            StateError::new(format!("an operation row has no value in `{column}`"))
        })?;
        parse(&value)
    }
    fn optional<T>(value: Option<String>) -> Result<Option<T>>
    where
        T: FromStr,
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        value.as_deref().map(parse).transpose()
    }
    let constraint_kind = |value: Option<String>| match value.as_deref() {
        Some("must_link") => Ok(ConstraintKind::MustLink),
        Some("cannot_link") => Ok(ConstraintKind::CannotLink),
        other => Err(StateError::new(format!(
            "unknown constraint kind {other:?} in an operation row"
        ))),
    };

    Ok(match row.kind.as_str() {
        "create_identity" => Op::CreateIdentity {
            identity: required(row.identity, "identity_id")?,
            entity_type: EntityType::new(row.entity_type.unwrap_or_default())
                .map_err(StateError::new)?,
        },
        "assign" => Op::Assign {
            observation: required(row.observation, "observation_id")?,
            from: optional(row.from_identity)?,
            to: optional(row.identity)?,
        },
        "merge" => Op::Merge {
            from: required(row.from_identity, "from_identity_id")?,
            into: required(row.identity, "identity_id")?,
        },
        "constrain" => Op::Constrain {
            kind: constraint_kind(row.constraint_kind)?,
            a: required(row.observation, "observation_id")?,
            b: required(row.other_observation, "other_observation_id")?,
        },
        "unconstrain" => Op::Unconstrain {
            kind: constraint_kind(row.constraint_kind)?,
            a: required(row.observation, "observation_id")?,
            b: required(row.other_observation, "other_observation_id")?,
        },
        "reinstate" => Op::Reinstate {
            identity: required(row.identity, "identity_id")?,
        },
        other => {
            return Err(
                StateError::new(format!("unknown operation kind {other:?} in the log")).into(),
            );
        }
    })
}

pub(crate) fn op_rows(ops: &[Op]) -> Result<duckdb::arrow::record_batch::RecordBatch> {
    let rows: Vec<OpRow> = ops.iter().map(op_to_row).collect();
    let column = |pick: fn(&OpRow) -> Option<String>| Column::Text(rows.iter().map(pick).collect());
    batch(vec![
        (
            "op_index",
            Column::Integer((0..rows.len() as i64).collect()),
        ),
        ("kind", column(|row| Some(row.kind.clone()))),
        ("identity_id", column(|row| row.identity.clone())),
        ("from_identity_id", column(|row| row.from_identity.clone())),
        ("observation_id", column(|row| row.observation.clone())),
        (
            "other_observation_id",
            column(|row| row.other_observation.clone()),
        ),
        ("entity_type", column(|row| row.entity_type.clone())),
        ("constraint_kind", column(|row| row.constraint_kind.clone())),
    ])
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

    fn observation_types(
        &self,
        observations: &[ObservationId],
    ) -> Result<Vec<Option<EntityType>>, StateError> {
        read_observation_types(&self.conn, observations).map_err(state_error)
    }

    fn identities(&self, ids: &[IdentityId]) -> Result<Vec<Option<IdentityRecord>>, StateError> {
        read_identities(&self.conn, ids).map_err(state_error)
    }

    fn assignments(
        &self,
        observations: &[ObservationId],
    ) -> Result<Vec<Option<IdentityId>>, StateError> {
        read_assignments(&self.conn, observations).map_err(state_error)
    }

    fn members(&self, ids: &[IdentityId]) -> Result<Vec<(IdentityId, ObservationId)>, StateError> {
        read_members(&self.conn, ids).map_err(state_error)
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

/// Used by tests in this crate to put observations in a store without going
/// through ingest.
#[cfg(test)]
pub(crate) fn insert_observations(
    conn: &Connection,
    observations: &[(ObservationId, EntityType)],
) -> Result<()> {
    for (id, entity_type) in observations {
        conn.execute(
            "INSERT INTO spool.observation
             VALUES (?::UUID, ?, 'test', NULL, NULL, '{}', NULL, NULL, make_timestamptz(0), 0)",
            params![id.to_string(), entity_type.as_str()],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use spool_core::{Actor, StepClock, conformance_tests};

    use super::*;

    fn store(observations: &[(ObservationId, EntityType)]) -> DuckStore {
        let store = DuckStore::open(None, Box::new(StepClock::new(0, 1_000_000))).unwrap();
        insert_observations(&store.conn, observations).unwrap();
        store
    }

    conformance_tests!(store);

    #[test]
    fn every_operation_kind_survives_the_log_row_format() {
        let mut ids = spool_core::SeededIds::new(5);
        use spool_core::IdGenerator;
        let (x, y) = (ids.identity_id(), ids.identity_id());
        let (a, b) = (ObservationId::from_u128(1), ObservationId::from_u128(2));
        let ops = [
            Op::CreateIdentity {
                identity: x,
                entity_type: EntityType::new("widget").unwrap(),
            },
            Op::Assign {
                observation: a,
                from: None,
                to: Some(x),
            },
            Op::Assign {
                observation: a,
                from: Some(x),
                to: Some(y),
            },
            Op::Assign {
                observation: a,
                from: Some(y),
                to: None,
            },
            Op::Merge { from: x, into: y },
            Op::Constrain {
                kind: ConstraintKind::CannotLink,
                a,
                b,
            },
            Op::Unconstrain {
                kind: ConstraintKind::MustLink,
                a,
                b,
            },
            Op::Reinstate { identity: x },
        ];
        for op in ops {
            assert_eq!(op_from_row(op_to_row(&op)).unwrap(), op);
        }
    }

    #[test]
    fn a_store_reopens_with_its_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.duckdb");
        let widget = EntityType::new("widget").unwrap();
        let observation = ObservationId::from_u128(1);
        let mut ids = spool_core::SeededIds::new(5);
        use spool_core::IdGenerator;
        let identity = ids.identity_id();

        let before = {
            let mut store =
                DuckStore::open(Some(&path), Box::new(StepClock::new(0, 1_000_000))).unwrap();
            insert_observations(&store.conn, &[(observation, widget.clone())]).unwrap();
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
                        observation,
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
    fn a_store_with_another_format_version_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.duckdb");
        {
            let store =
                DuckStore::open(Some(&path), Box::new(StepClock::new(0, 1_000_000))).unwrap();
            schema::set_meta(&store.conn, "format_version", "999").unwrap();
        }
        let error = DuckStore::open(Some(&path), Box::new(StepClock::new(0, 1))).err();
        assert!(matches!(
            error,
            Some(crate::error::Error::FormatVersion { .. })
        ));
    }
}
