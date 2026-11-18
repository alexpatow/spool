//! Rows a source takes back.
//!
//! A source that no longer holds a row says so by its key. Every version of
//! the row the store holds is then retracted:
//!
//! - The records stay, with their payload, as evidence of what the source
//!   once said. Nothing is deleted from `spool.record`.
//! - They leave their identity, by a changeset like any other: one
//!   `assign` to nothing per record, under the name of whoever asked. The
//!   log shows what each was a member of, and the identity keeps its ID and
//!   its other members.
//! - They have no feature rows, so no rule and no strategy sees them, and no
//!   run examines them. A changeset that would give one an identity is
//!   refused.
//!
//! A retraction ends when the source sends the row again, changed or not.
//! Every version comes back, to the identity it left: what put it there
//! still holds, whether a run did or a reviewer. The ingest commits one
//! `assign` per record for it, as the engine. A record that had no
//! identity, or that can no longer be where it was, is new to the next
//! match run. Both the retraction and its end are kept in
//! `spool.retraction`.
//!
//! A row without a key cannot be taken back: nothing but its content names
//! it.

use duckdb::arrow::record_batch::RecordBatch;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{Committed, EntityType, IdentityId, Op, RecordId, RunId, Seq};

use crate::batch::{Column, batch};
use crate::error::{Error, Result};
use crate::features;
use crate::review::ABOUT_WAITING;
use crate::store::parse;

/// What a retraction did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RetractReport {
    pub run: RunId,
    /// Keys named, counting a key that was named twice once.
    pub received: u64,
    /// Keys under which the source has no row of the entity type.
    pub unknown: u64,
    /// Records taken back: every version of each row. A record that was
    /// already taken back is not counted again.
    pub retracted: u64,
    /// Records that left an identity.
    pub detached: u64,
    /// The changeset that detached them. `None` when none had an identity.
    pub committed: Option<Committed>,
    /// Identities the records left whose remaining members nothing holds
    /// together any more. Each has an open `possible_split` review item,
    /// which this run raised.
    #[serde(default)]
    pub possible_splits: u64,
}

/// A condition on `record_id` that leaves out the records that are taken
/// back.
pub(crate) const NOT_RETRACTED: &str = "record_id NOT IN (
    SELECT record_id FROM spool.retraction WHERE restored_run IS NULL)";

/// A query for the records that came back in an ingest run after `after`
/// and before `before`.
pub(crate) fn restored_between(after: RunId, before: RunId) -> String {
    format!(
        "SELECT record_id FROM spool.retraction
         WHERE restored_run > {after} AND restored_run < {before}"
    )
}

/// A query for the records that came back in one ingest run.
pub(crate) fn restored_in(run: RunId) -> String {
    format!("SELECT record_id FROM spool.retraction WHERE restored_run = {run}")
}

/// The name of the temporary table that holds the records of the keys a
/// retraction names.
const NAMED: &str = "spool_retract_named";

/// How many keys a retraction named, and how many of them the source has no
/// row under.
pub(crate) struct Named {
    pub received: u64,
    pub unknown: u64,
}

/// Finds every version of the rows the source holds under `keys`, and keeps
/// them for the rest of the retraction.
pub(crate) fn name(
    conn: &Connection,
    entity_type: &EntityType,
    source: &str,
    keys: &[String],
) -> Result<Named> {
    let keys = batch(vec![(
        "source_key",
        Column::Text(keys.iter().cloned().map(Some).collect()),
    )])?;
    conn.execute_batch("CREATE OR REPLACE TEMP TABLE spool_retract_keys (source_key VARCHAR)")?;
    conn.execute(
        "INSERT INTO spool_retract_keys SELECT DISTINCT source_key FROM arrow(?)",
        [&ArrowBatchRegistration::new(keys)],
    )?;
    conn.execute(
        &format!(
            "CREATE OR REPLACE TEMP TABLE {NAMED} AS
             SELECT o.record_id, o.source_key
             FROM spool.record o
             JOIN spool_retract_keys k ON k.source_key = o.source_key
             WHERE o.entity_type = ? AND o.source_id = ?"
        ),
        params![entity_type.as_str(), source],
    )?;
    let (received, unknown): (i64, i64) = conn.query_row(
        &format!(
            "SELECT count(*),
                    count(*) FILTER (WHERE k.source_key NOT IN (SELECT source_key FROM {NAMED}))
             FROM spool_retract_keys k"
        ),
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    conn.execute_batch("DROP TABLE spool_retract_keys")?;
    Ok(Named {
        received: received as u64,
        unknown: unknown as u64,
    })
}

/// The named records that have an identity, with the identity, ordered by
/// record.
pub(crate) fn members(conn: &Connection) -> Result<Vec<(RecordId, IdentityId)>> {
    let mut statement = conn.prepare(&format!(
        "SELECT m.record_id::VARCHAR, m.identity_id::VARCHAR
         FROM spool.membership m
         WHERE m.to_seq IS NULL AND m.record_id IN (SELECT record_id FROM {NAMED})
         ORDER BY m.record_id"
    ))?;
    let mut rows = statement.query([])?;
    let mut members = Vec::new();
    while let Some(row) = rows.next()? {
        members.push((
            parse(&row.get::<_, String>(0)?)?,
            parse(&row.get::<_, String>(1)?)?,
        ));
    }
    Ok(members)
}

/// Records the named records as taken back by `run`, and returns how many
/// were not taken back already. `detached` is the log position of the
/// changeset that took them out of their identities, if any had one. With
/// them go their feature rows, an open draft that places one of them, and
/// the open review items that are left with nothing to ask about.
pub(crate) fn record(
    conn: &Connection,
    entity_type: &EntityType,
    run: RunId,
    detached: Option<Seq>,
) -> Result<u64> {
    let retracted = conn.execute(
        &format!(
            "INSERT INTO spool.retraction
             SELECT record_id, $1, NULL,
                    (SELECT max(m.to_seq) FROM spool.membership m
                     WHERE m.record_id = n.record_id AND m.to_seq = $2)
             FROM {NAMED} n WHERE {NOT_RETRACTED}"
        ),
        params![run.0 as i64, detached.map(|seq| seq.0 as i64)],
    )?;
    conn.execute_batch(&format!(
        "DELETE FROM {} WHERE record_id IN (SELECT record_id FROM {NAMED})",
        features::table(entity_type)
    ))?;
    // The draft could never be approved: it gives a record an identity that
    // may not have one. Its match run has not succeeded, so the records
    // it is about are examined again.
    conn.execute(
        &format!(
            "UPDATE spool.draft SET status = 'superseded', closed_run = ?
             WHERE status = 'open' AND entity_type = ? AND changeset_id IN (
               SELECT changeset_id FROM spool.draft_op
               WHERE record_id IN (SELECT record_id FROM {NAMED})
                  OR other_record_id IN (SELECT record_id FROM {NAMED}))"
        ),
        params![run.0 as i64, entity_type.as_str()],
    )?;
    // An item stays open while one of its records waits for an identity. A
    // record that was taken back does not wait. A `possible_split` is not
    // about records that wait: the changeset that took its record out of
    // the identity has already closed it if nothing is left to ask.
    conn.execute(
        &format!(
            "UPDATE spool.review_item SET status = 'superseded', closed_run = ?
             WHERE status = 'open' AND entity_type = ? AND {ABOUT_WAITING}
               AND review_id IN (
                 SELECT review_id
                 FROM (SELECT review_id, unnest(record_ids) AS record_id
                       FROM spool.review_item WHERE status = 'open')
                 WHERE record_id IN (SELECT record_id FROM {NAMED}))
               AND review_id NOT IN ({WAITING})"
        ),
        params![run.0 as i64, entity_type.as_str()],
    )?;
    conn.execute_batch(&format!("DROP TABLE {NAMED}"))?;
    Ok(retracted as u64)
}

/// A query for the open review items that have a record waiting for an
/// identity: one that has none and was not taken back.
pub(crate) const WAITING: &str = "
    SELECT r.review_id
    FROM (SELECT review_id, unnest(record_ids) AS record_id
          FROM spool.review_item WHERE status = 'open') r
    WHERE NOT EXISTS (
            SELECT 1 FROM spool.membership m
            WHERE m.record_id = r.record_id AND m.to_seq IS NULL)
      AND r.record_id NOT IN (
            SELECT record_id FROM spool.retraction WHERE restored_run IS NULL)";

/// Ends the retraction of every version of the rows that are staged for an
/// ingest: the source holds them again. Returns how many records came
/// back.
pub(crate) fn restore(
    conn: &Connection,
    entity_type: &str,
    source: &str,
    run: RunId,
    staged: &RecordBatch,
) -> Result<u64> {
    // Most ingests find nothing here, and this is one cheap look.
    let any: bool = conn.query_row(
        "SELECT count(*) > 0 FROM spool.retraction WHERE restored_run IS NULL",
        [],
        |row| row.get(0),
    )?;
    if !any {
        return Ok(0);
    }
    let restored = conn.execute(
        "UPDATE spool.retraction SET restored_run = $1
         WHERE restored_run IS NULL AND record_id IN (
           SELECT o.record_id
           FROM spool.record o
           WHERE o.entity_type = $2 AND o.source_id = $3
             AND o.source_key IN (SELECT s.source_key FROM arrow($4) s))",
        params![
            run.0 as i64,
            entity_type,
            source,
            &ArrowBatchRegistration::new(staged.clone())
        ],
    )?;
    Ok(restored as u64)
}

/// The operations that put the records that came back in the ingest run
/// `run` into the identity each of them left, or into the one that stands
/// for it now, ordered by record. A record that had no identity when it was
/// taken back has none here.
pub(crate) fn returning(conn: &Connection, run: RunId) -> Result<Vec<Op>> {
    let mut statement = conn.prepare(
        "SELECT t.record_id::VARCHAR, coalesce(i.merged_into, i.identity_id)::VARCHAR
         FROM spool.retraction t
         JOIN spool.membership m ON m.record_id = t.record_id AND m.to_seq = t.detached_seq
         JOIN spool.identity i ON i.identity_id = m.identity_id
         WHERE t.restored_run = ?
           AND NOT EXISTS (
             SELECT 1 FROM spool.membership now
             WHERE now.record_id = t.record_id AND now.to_seq IS NULL)
         ORDER BY t.record_id",
    )?;
    let mut rows = statement.query([run.0 as i64])?;
    let mut ops = Vec::new();
    while let Some(row) = rows.next()? {
        ops.push(Op::Assign {
            record: parse(&row.get::<_, String>(0)?)?,
            from: None,
            to: Some(parse(&row.get::<_, String>(1)?)?),
        });
    }
    Ok(ops)
}

/// Refuses operations that would give an identity to a record that is
/// taken back, naming the first such record.
pub(crate) fn refuse_placing(conn: &Connection, ops: &[Op]) -> Result<()> {
    let placed: Vec<Option<String>> = ops
        .iter()
        .filter_map(|op| match op {
            Op::Assign {
                record,
                to: Some(_),
                ..
            } => Some(Some(record.to_string())),
            _ => None,
        })
        .collect();
    if placed.is_empty() {
        return Ok(());
    }
    let placed = batch(vec![("record_id", Column::Text(placed))])?;
    let mut statement = conn.prepare(
        "SELECT r.record_id::VARCHAR
         FROM arrow(?) p
         JOIN spool.retraction r
           ON r.record_id = p.record_id::UUID AND r.restored_run IS NULL
         ORDER BY r.record_id LIMIT 1",
    )?;
    let mut rows = statement.query([&ArrowBatchRegistration::new(placed)])?;
    match rows.next()? {
        Some(row) => Err(Error::Retracted(parse(&row.get::<_, String>(0)?)?)),
        None => Ok(()),
    }
}
