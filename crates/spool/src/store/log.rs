//! Reading the changeset log: the committed changesets, their operations,
//! and where the log stands.

use duckdb::{Connection, params};
use spool_core::{
    Changeset, ChangesetId, ContentHash, IdentityId, LogEntry, Op, RunId, Seq, StateError,
};

use super::ops::{OpRow, op_from_row};
use super::parse;
use crate::error::{Error, Result};

pub(crate) fn read_log(conn: &Connection) -> Result<Vec<LogEntry>> {
    read_log_after(conn, Seq::ZERO, u64::MAX)
}

/// At most `limit` committed changesets after a log position, in log order.
pub(crate) fn read_log_after(conn: &Connection, after: Seq, limit: u64) -> Result<Vec<LogEntry>> {
    let mut entries = Vec::new();
    // The database counts positions in signed 64 bits, so no log reaches a
    // position past those, and nothing is after one. Cast, such a position
    // would be a negative one that the whole log is after.
    if i64::try_from(after.0).is_err() {
        return Ok(entries);
    }
    let mut statement = conn.prepare(
        "SELECT seq, changeset_id::VARCHAR, base_seq, actor, reason, run_id, content_hash
         FROM spool.changeset WHERE seq > ? ORDER BY seq LIMIT ?",
    )?;
    let mut rows = statement.query(params![after.0 as i64, limit.min(i64::MAX as u64) as i64])?;
    while let Some(row) = rows.next()? {
        entries.push(log_entry(row)?);
    }
    let Some(last) = entries.last().map(|entry| entry.seq) else {
        return Ok(entries);
    };

    let mut statement = conn.prepare(
        "SELECT seq, kind, identity_id::VARCHAR, from_identity_id::VARCHAR,
                record_id::VARCHAR, other_record_id::VARCHAR,
                entity_type, constraint_kind
         FROM spool.changeset_op WHERE seq > ? AND seq <= ? ORDER BY seq, op_index",
    )?;
    let mut rows = statement.query([after.0 as i64, last.0 as i64])?;
    while let Some(row) = rows.next()? {
        let seq = Seq(row.get::<_, i64>(0)? as u64);
        let op = log_op(row, 1)?;
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

/// Reads the fields in the log row. Operations are held in their own rows.
pub(crate) fn log_entry(row: &duckdb::Row<'_>) -> Result<LogEntry> {
    Ok(LogEntry {
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
    })
}

/// Operations of a committed changeset in their original order.
pub(crate) fn read_log_ops(
    conn: &Connection,
    seq: Seq,
    offset: u64,
    limit: u64,
) -> Result<Vec<Op>> {
    let position = i64::try_from(seq.0).map_err(|_| Error::UnknownChangeset(seq))?;
    // The log is dense and append-only, and a commit writes its operations
    // in the same transaction as its log position.
    if seq == Seq::ZERO || seq > read_head(conn)? {
        return Err(Error::UnknownChangeset(seq));
    }
    let Ok(offset) = i32::try_from(offset) else {
        return Ok(Vec::new());
    };
    let mut statement = conn.prepare(
        "SELECT kind, identity_id::VARCHAR, from_identity_id::VARCHAR,
                record_id::VARCHAR, other_record_id::VARCHAR,
                entity_type, constraint_kind
         FROM spool.changeset_op WHERE seq = ? AND op_index >= ?
         ORDER BY op_index LIMIT ?",
    )?;
    let mut rows = statement.query(params![position, offset, limit.min(i64::MAX as u64) as i64])?;
    let mut ops = Vec::new();
    while let Some(row) = rows.next()? {
        ops.push(log_op(row, 0)?);
    }
    Ok(ops)
}

fn log_op(row: &duckdb::Row<'_>, at: usize) -> Result<Op> {
    op_from_row(OpRow {
        kind: row.get(at)?,
        identity: row.get(at + 1)?,
        from_identity: row.get(at + 2)?,
        record: row.get(at + 3)?,
        other_record: row.get(at + 4)?,
        entity_type: row.get(at + 5)?,
        constraint_kind: row.get(at + 6)?,
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

pub(crate) fn read_merged_before(
    conn: &Connection,
    identity: IdentityId,
    seq: Seq,
) -> Result<Option<IdentityId>> {
    let mut statement = conn.prepare(
        "SELECT identity_id::VARCHAR FROM spool.changeset_op
         WHERE kind = 'merge' AND from_identity_id = ?::UUID AND seq < ?
         ORDER BY seq DESC LIMIT 1",
    )?;
    let mut rows = statement.query(params![identity.to_string(), seq.0 as i64])?;
    Ok(match rows.next()? {
        Some(row) => Some(parse(&row.get::<_, String>(0)?)?),
        None => None,
    })
}
