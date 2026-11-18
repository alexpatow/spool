//! Drafts: changesets a reconcile run drafted and left for review, kept in
//! the store.
//!
//! A draft is the changeset exactly as the reconciler wrote it, and its ID is
//! the changeset's ID. It is evidence of intent, not identity state: it is
//! not in the log until it is committed, and replay does not need it. It is
//! open until one of three things happens:
//!
//! - `committed`: it was approved, or its changeset was committed as it
//!   stands. It passes the same validation as any changeset, so a draft whose
//!   preconditions no longer hold is rejected and stays open.
//! - `rejected`: a reviewer said no.
//! - `superseded`: a later reconcile run of the entity type drafted again.
//!   That run examined everything this draft covers, so this one is stale.
//!
//! A match run whose draft is open has not succeeded. What is new to the next
//! run is counted from the last run that succeeded, so the records of a
//! draft nobody commits are examined again.

use duckdb::vtab::arrow::ArrowBatchRegistration;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{Changeset, ChangesetId, EntityType, Op, RunId, Seq, StateError};

use crate::error::Result;
use crate::store::{OpRow, op_from_row, op_rows, parse};
use crate::time::rfc3339;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    Open,
    Committed,
    Rejected,
    Superseded,
}

impl DraftStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            DraftStatus::Open => "open",
            DraftStatus::Committed => "committed",
            DraftStatus::Rejected => "rejected",
            DraftStatus::Superseded => "superseded",
        }
    }

    pub(crate) fn from_stored(text: &str) -> Result<Self> {
        Ok(match text {
            "open" => DraftStatus::Open,
            "committed" => DraftStatus::Committed,
            "rejected" => DraftStatus::Rejected,
            "superseded" => DraftStatus::Superseded,
            other => {
                return Err(StateError::new(format!("unknown draft status {other:?}")).into());
            }
        })
    }
}

impl std::fmt::Display for DraftStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A draft without its operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DraftSummary {
    /// The ID of the changeset, which is how a draft is named.
    pub id: ChangesetId,
    pub entity_type: EntityType,
    /// The match run whose judgements the draft rests on.
    pub match_run: RunId,
    /// The reconcile run that drafted it.
    pub reconcile_run: RunId,
    /// The log position it was drafted against.
    pub base: Seq,
    pub status: DraftStatus,
    pub operations: u64,
    pub identities_created: u64,
    /// The records it would place or move, the earlier versions of rows
    /// it places included.
    pub records_assigned: u64,
    /// Committed: its position in the log.
    pub committed_seq: Option<Seq>,
    /// Superseded: the reconcile run that drafted the entity type again.
    pub superseded_by: Option<RunId>,
    /// Rejected: when.
    pub rejected_at: Option<String>,
    /// Rejected: why, if the reviewer said.
    pub rejected_reason: Option<String>,
}

/// A draft with the changeset it would commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DraftEntry {
    pub draft: DraftSummary,
    pub changeset: Changeset,
}

/// Which drafts to list. The default lists every draft.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftFilter {
    pub status: Option<DraftStatus>,
    pub entity_type: Option<EntityType>,
}

impl DraftFilter {
    /// The drafts that wait for a decision.
    pub fn open() -> Self {
        Self {
            status: Some(DraftStatus::Open),
            entity_type: None,
        }
    }
}

/// Stores a draft as open. `changeset.run` is the reconcile run.
pub(crate) fn store(
    conn: &Connection,
    entity_type: &EntityType,
    match_run: RunId,
    changeset: &Changeset,
) -> Result<()> {
    let reconcile_run = changeset
        .run
        .ok_or_else(|| StateError::new("a draft without a reconcile run"))?;
    conn.execute(
        "INSERT INTO spool.draft
         VALUES (?::UUID, ?, ?, ?, ?, ?, ?, 'open', NULL, NULL, NULL, NULL)",
        params![
            changeset.id.to_string(),
            reconcile_run.0 as i64,
            match_run.0 as i64,
            entity_type.as_str(),
            changeset.base.0 as i64,
            changeset.actor.to_string(),
            changeset.reason,
        ],
    )?;
    conn.execute(
        "INSERT INTO spool.draft_op
         SELECT ?::UUID, op_index::INTEGER, kind, identity_id::UUID, from_identity_id::UUID,
                record_id::UUID, other_record_id::UUID, entity_type, constraint_kind
         FROM arrow(?)",
        params![
            changeset.id.to_string(),
            &ArrowBatchRegistration::new(op_rows(&changeset.ops)?)
        ],
    )?;
    Ok(())
}

/// Closes the open drafts of an entity type as superseded by a reconcile run
/// that examined the type again.
pub(crate) fn supersede(conn: &Connection, entity_type: &EntityType, run: RunId) -> Result<()> {
    conn.execute(
        "UPDATE spool.draft SET status = 'superseded', closed_run = ?
         WHERE status = 'open' AND entity_type = ?",
        params![run.0 as i64, entity_type.as_str()],
    )?;
    Ok(())
}

/// Records that the changeset at `seq` was an open draft, if it was one: the
/// draft is committed and its match run has succeeded. Runs inside the
/// transaction that commits the changeset.
pub(crate) fn committed(conn: &Connection, id: ChangesetId, seq: Seq, at: i64) -> Result<()> {
    let id = id.to_string();
    conn.execute(
        "UPDATE spool.run SET status = 'succeeded', finished_at = make_timestamptz(?)
         WHERE status = 'drafted' AND run_id IN (
           SELECT match_run FROM spool.draft WHERE changeset_id = ?::UUID AND status = 'open')",
        params![at, id],
    )?;
    conn.execute(
        "UPDATE spool.draft SET status = 'committed', closed_seq = ?
         WHERE changeset_id = ?::UUID AND status = 'open'",
        params![seq.0 as i64, id],
    )?;
    Ok(())
}

/// Closes an open draft as rejected.
pub(crate) fn reject(
    conn: &Connection,
    id: ChangesetId,
    at: i64,
    reason: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE spool.draft
         SET status = 'rejected', closed_at = make_timestamptz(?), closed_reason = ?
         WHERE changeset_id = ?::UUID AND status = 'open'",
        params![at, reason, id.to_string()],
    )?;
    Ok(())
}

const SUMMARY: &str = "
    SELECT d.changeset_id::VARCHAR, d.entity_type, d.match_run, d.run_id, d.base_seq, d.status,
           (SELECT count(*) FROM spool.draft_op o WHERE o.changeset_id = d.changeset_id),
           (SELECT count(*) FROM spool.draft_op o
            WHERE o.changeset_id = d.changeset_id AND o.kind = 'create_identity'),
           (SELECT count(*) FROM spool.draft_op o
            WHERE o.changeset_id = d.changeset_id AND o.kind = 'assign'),
           d.closed_seq, d.closed_run, epoch_us(d.closed_at), d.closed_reason,
           d.actor, d.reason
    FROM spool.draft d";

fn summary(row: &duckdb::Row<'_>) -> Result<(DraftSummary, String, Option<String>)> {
    let seq = |value: Option<i64>| value.map(|seq| Seq(seq as u64));
    let run = |value: Option<i64>| value.map(|run| RunId(run as u64));
    let summary = DraftSummary {
        id: parse(&row.get::<_, String>(0)?)?,
        entity_type: EntityType::new(row.get::<_, String>(1)?).map_err(StateError::new)?,
        match_run: RunId(row.get::<_, i64>(2)? as u64),
        reconcile_run: RunId(row.get::<_, i64>(3)? as u64),
        base: Seq(row.get::<_, i64>(4)? as u64),
        status: DraftStatus::from_stored(&row.get::<_, String>(5)?)?,
        operations: row.get::<_, i64>(6)? as u64,
        identities_created: row.get::<_, i64>(7)? as u64,
        records_assigned: row.get::<_, i64>(8)? as u64,
        committed_seq: seq(row.get(9)?),
        superseded_by: run(row.get(10)?),
        rejected_at: row.get::<_, Option<i64>>(11)?.map(rfc3339),
        rejected_reason: row.get(12)?,
    };
    Ok((summary, row.get(13)?, row.get(14)?))
}

/// Drafts without their operations, newest first.
pub(crate) fn list(conn: &Connection, filter: &DraftFilter) -> Result<Vec<DraftSummary>> {
    let mut statement = conn.prepare(&format!(
        "{SUMMARY}
         WHERE ($1::VARCHAR IS NULL OR d.status = $1::VARCHAR)
           AND ($2::VARCHAR IS NULL OR d.entity_type = $2::VARCHAR)
         ORDER BY d.run_id DESC"
    ))?;
    let mut rows = statement.query(params![
        filter.status.map(DraftStatus::as_str),
        filter.entity_type.as_ref().map(EntityType::as_str),
    ])?;
    let mut drafts = Vec::new();
    while let Some(row) = rows.next()? {
        drafts.push(summary(row)?.0);
    }
    Ok(drafts)
}

/// One draft with its changeset, or `None` if there is no draft by that ID.
pub(crate) fn get(conn: &Connection, id: ChangesetId) -> Result<Option<DraftEntry>> {
    let found = {
        let mut statement = conn.prepare(&format!("{SUMMARY} WHERE d.changeset_id = ?::UUID"))?;
        let mut rows = statement.query([id.to_string()])?;
        match rows.next()? {
            Some(row) => Some(summary(row)?),
            None => None,
        }
    };
    let Some((draft, actor, reason)) = found else {
        return Ok(None);
    };

    let mut ops: Vec<Op> = Vec::new();
    let mut statement = conn.prepare(
        "SELECT kind, identity_id::VARCHAR, from_identity_id::VARCHAR,
                record_id::VARCHAR, other_record_id::VARCHAR,
                entity_type, constraint_kind
         FROM spool.draft_op WHERE changeset_id = ?::UUID ORDER BY op_index",
    )?;
    let mut rows = statement.query([id.to_string()])?;
    while let Some(row) = rows.next()? {
        ops.push(op_from_row(OpRow {
            kind: row.get(0)?,
            identity: row.get(1)?,
            from_identity: row.get(2)?,
            record: row.get(3)?,
            other_record: row.get(4)?,
            entity_type: row.get(5)?,
            constraint_kind: row.get(6)?,
        })?);
    }
    let changeset = Changeset {
        id: draft.id,
        base: draft.base,
        actor: parse(&actor)?,
        reason,
        run: Some(draft.reconcile_run),
        ops,
    };
    Ok(Some(DraftEntry { draft, changeset }))
}
