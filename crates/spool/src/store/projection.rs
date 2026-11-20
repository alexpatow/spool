//! Reading the projection tables: identities, memberships and constraints,
//! as a whole snapshot and as the answers to the validator's questions.

use duckdb::{Connection, params};
use spool_core::{
    Constraint, ConstraintEntry, EntityType, IdentityEntry, IdentityId, IdentityStatus, Membership,
    Merged, RecordId, Seq, Snapshot, StateError,
};

use super::log::read_head;
use super::ops::constraint_kind;
use super::parse;
use crate::batch::id_list;
use crate::error::Result;

pub(crate) fn read_snapshot(conn: &Connection) -> Result<Snapshot> {
    let head = read_head(conn)?;

    let mut identities = Vec::new();
    let mut statement = conn.prepare(
        "SELECT identity_id::VARCHAR, entity_type, created_seq, merged_into::VARCHAR,
                merged_seq, merge_target::VARCHAR
         FROM spool.identity",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        identities.push(identity_entry(row, 0)?);
    }
    identities.sort_by_key(|record| record.id);

    let mut memberships = Vec::new();
    let mut statement = conn.prepare(
        "SELECT record_id::VARCHAR, identity_id::VARCHAR, from_seq, to_seq
         FROM spool.membership",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        memberships.push(Membership {
            record: parse(&row.get::<_, String>(0)?)?,
            identity: parse(&row.get::<_, String>(1)?)?,
            from: Seq(row.get::<_, i64>(2)? as u64),
            to: row.get::<_, Option<i64>>(3)?.map(|seq| Seq(seq as u64)),
        });
    }
    memberships.sort();

    let mut constraints = Vec::new();
    let mut statement = conn.prepare(
        "SELECT record_a::VARCHAR, record_b::VARCHAR, kind, from_seq, to_seq
         FROM spool.constraint_link",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        constraints.push(ConstraintEntry {
            constraint: constraint(row, 0)?,
            from: Seq(row.get::<_, i64>(3)? as u64),
            to: row.get::<_, Option<i64>>(4)?.map(|seq| Seq(seq as u64)),
        });
    }
    constraints.sort();

    Ok(Snapshot {
        head,
        identities,
        memberships,
        constraints,
    })
}

pub(crate) fn read_record_types(
    conn: &Connection,
    records: &[RecordId],
) -> Result<Vec<Option<EntityType>>> {
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT o.entity_type
         FROM arrow(?) a LEFT JOIN spool.record o ON o.record_id = a.id::UUID
         ORDER BY a.ord",
    )?;
    let mut rows = statement.query([&id_list(records)?])?;
    let mut types = Vec::with_capacity(records.len());
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
) -> Result<Vec<Option<IdentityEntry>>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT i.identity_id::VARCHAR, i.entity_type, i.created_seq, i.merged_into::VARCHAR,
                i.merged_seq, i.merge_target::VARCHAR
         FROM arrow(?) a LEFT JOIN spool.identity i ON i.identity_id = a.id::UUID
         ORDER BY a.ord",
    )?;
    let mut rows = statement.query([&id_list(ids)?])?;
    let mut records = Vec::with_capacity(ids.len());
    while let Some(row) = rows.next()? {
        records.push(match row.get::<_, Option<String>>(0)? {
            Some(_) => Some(identity_entry(row, 0)?),
            None => None,
        });
    }
    Ok(records)
}

pub(crate) fn read_assignments(
    conn: &Connection,
    records: &[RecordId],
) -> Result<Vec<Option<IdentityId>>> {
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT m.identity_id::VARCHAR
         FROM arrow(?) a
         LEFT JOIN spool.membership m ON m.record_id = a.id::UUID AND m.to_seq IS NULL
         ORDER BY a.ord",
    )?;
    let mut rows = statement.query([&id_list(records)?])?;
    let mut assignments = Vec::with_capacity(records.len());
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
) -> Result<Vec<(IdentityId, RecordId)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT m.identity_id::VARCHAR, m.record_id::VARCHAR
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

pub(crate) fn read_membership_changed(conn: &Connection, ids: &[IdentityId]) -> Result<Vec<Seq>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT coalesce(max(greatest(m.from_seq, coalesce(m.to_seq, 0))), 0)
         FROM arrow(?) a LEFT JOIN spool.membership m ON m.identity_id = a.id::UUID
         GROUP BY a.ord ORDER BY a.ord",
    )?;
    let mut rows = statement.query([&id_list(ids)?])?;
    let mut changed = Vec::with_capacity(ids.len());
    while let Some(row) = rows.next()? {
        changed.push(Seq(row.get::<_, i64>(0)? as u64));
    }
    Ok(changed)
}

/// The constraints in force that name any of the records, sorted.
pub(crate) fn read_constraints_on(
    conn: &Connection,
    records: &[RecordId],
) -> Result<Vec<Constraint>> {
    if records.is_empty() {
        return Ok(Vec::new());
    }
    constraints(
        conn,
        "SELECT DISTINCT c.record_a::VARCHAR, c.record_b::VARCHAR, c.kind
         FROM spool.constraint_link c
         JOIN arrow(?) a ON a.id::UUID IN (c.record_a, c.record_b)
         WHERE c.to_seq IS NULL",
        records,
    )
}

/// The constraints in force that name a current member of any of the
/// identities, sorted.
pub(crate) fn read_constraints_of_members(
    conn: &Connection,
    ids: &[IdentityId],
) -> Result<Vec<Constraint>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    constraints(
        conn,
        "SELECT DISTINCT c.record_a::VARCHAR, c.record_b::VARCHAR, c.kind
         FROM spool.constraint_link c
         JOIN spool.membership m
           ON m.to_seq IS NULL AND m.record_id IN (c.record_a, c.record_b)
         JOIN arrow(?) a ON a.id::UUID = m.identity_id
         WHERE c.to_seq IS NULL",
        ids,
    )
}

fn constraints<T: ToString>(conn: &Connection, sql: &str, ids: &[T]) -> Result<Vec<Constraint>> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query([&id_list(ids)?])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(constraint(row, 0)?);
    }
    found.sort_unstable();
    Ok(found)
}

/// Reads a constraint from three columns: the two records and the kind.
pub(crate) fn constraint(row: &duckdb::Row<'_>, at: usize) -> Result<Constraint> {
    Ok(Constraint::new(
        constraint_kind(row.get::<_, Option<String>>(at + 2)?.as_deref())?,
        parse(&row.get::<_, String>(at)?)?,
        parse(&row.get::<_, String>(at + 1)?)?,
    ))
}

pub(crate) fn read_left_at(
    conn: &Connection,
    identity: IdentityId,
    seq: Seq,
) -> Result<Vec<RecordId>> {
    let mut statement = conn.prepare(
        "SELECT record_id::VARCHAR FROM spool.membership
         WHERE identity_id = ?::UUID AND to_seq = ?",
    )?;
    let mut rows = statement.query(params![identity.to_string(), seq.0 as i64])?;
    let mut left = Vec::new();
    while let Some(row) = rows.next()? {
        left.push(parse(&row.get::<_, String>(0)?)?);
    }
    left.sort_unstable();
    Ok(left)
}

/// Reads an identity from six columns: its ID, entity type and creation, the
/// identity that stands for it, and the merge that made it inactive.
fn identity_entry(row: &duckdb::Row<'_>, at: usize) -> Result<IdentityEntry> {
    let merged_into: Option<String> = row.get(at + 3)?;
    let merged_seq: Option<i64> = row.get(at + 4)?;
    let merge_target: Option<String> = row.get(at + 5)?;
    Ok(IdentityEntry {
        id: parse(&row.get::<_, String>(at)?)?,
        entity_type: EntityType::new(row.get::<_, String>(at + 1)?).map_err(StateError::new)?,
        created: Seq(row.get::<_, i64>(at + 2)? as u64),
        status: match merged_into {
            Some(survivor) => IdentityStatus::MergedInto(parse(&survivor)?),
            None => IdentityStatus::Active,
        },
        merged: match (merge_target, merged_seq) {
            (Some(into), Some(seq)) => Some(Merged {
                into: parse(&into)?,
                seq: Seq(seq as u64),
            }),
            _ => None,
        },
    })
}
