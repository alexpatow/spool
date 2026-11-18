//! The earlier versions of a row go where the row goes.
//!
//! A row sent again under its key with other content is the same thing as
//! before, so its versions share an identity. A changeset that places the
//! latest version of a row, whether a match run or a reviewer wrote it,
//! takes its earlier versions along: each that is in another identity, or
//! in none, moves to where the latest goes. A version the changeset moves
//! itself stays where it puts it, and so does one whose identity it merges
//! or reinstates, which it has a say about already. A version its source
//! took back stays out, as every version of that row is.
//!
//! The same holds for what an extractor found: a mention found again at
//! its place with other attributes is the next version of that record.

use std::collections::{BTreeSet, HashSet};

use duckdb::Connection;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use spool_core::{IdentityId, Op, RecordId};

use crate::batch::{Column, batch};
use crate::error::Result;
use crate::store::parse;

/// The moves that take the earlier versions of each row `ops` places to
/// where `ops` places it, ordered by the row's operation and then by
/// version.
pub(crate) fn following(conn: &Connection, ops: &[Op]) -> Result<Vec<Op>> {
    let mut placed = Vec::new();
    let mut moved = HashSet::new();
    let mut decided = HashSet::new();
    for op in ops {
        match op {
            Op::Assign { record, to, .. } => {
                moved.insert(*record);
                if let Some(to) = to {
                    placed.push((*record, *to));
                }
            }
            Op::Merge { from, into } => {
                decided.extend([*from, *into]);
            }
            Op::Reinstate { identity } => {
                decided.insert(*identity);
            }
            Op::CreateIdentity { .. } | Op::Constrain { .. } | Op::Unconstrain { .. } => {}
        }
    }
    if placed.is_empty() {
        return Ok(Vec::new());
    }
    let staged = batch(vec![
        ("ord", Column::Integer((0..placed.len() as i64).collect())),
        (
            "record_id",
            Column::Text(
                placed
                    .iter()
                    .map(|(record, _)| Some(record.to_string()))
                    .collect(),
            ),
        ),
        (
            "identity_id",
            Column::Text(placed.iter().map(|(_, to)| Some(to.to_string())).collect()),
        ),
    ])?;
    let mut statement = conn.prepare(
        "SELECT earlier.record_id::VARCHAR, m.identity_id::VARCHAR, p.identity_id
         FROM arrow(?) p
         JOIN spool.record latest ON latest.record_id = p.record_id::UUID
         JOIN spool.record earlier
           ON earlier.entity_type = latest.entity_type
          AND earlier.source_id = latest.source_id
          AND earlier.source_key = latest.source_key
          AND earlier.record_id <> latest.record_id
         LEFT JOIN spool.membership m
           ON m.record_id = earlier.record_id AND m.to_seq IS NULL
         WHERE NOT EXISTS (
             SELECT 1 FROM spool.record later WHERE later.supersedes = latest.record_id)
           AND earlier.record_id NOT IN (
             SELECT record_id FROM spool.retraction WHERE restored_run IS NULL)
           AND (m.identity_id IS NULL OR m.identity_id <> p.identity_id::UUID)
         ORDER BY p.ord, earlier.loaded_at, earlier.record_id",
    )?;
    let mut rows = statement.query([&ArrowBatchRegistration::new(staged)])?;
    let mut ops = Vec::new();
    let mut taken = BTreeSet::new();
    while let Some(row) = rows.next()? {
        let record: RecordId = parse(&row.get::<_, String>(0)?)?;
        let from: Option<IdentityId> = row
            .get::<_, Option<String>>(1)?
            .as_deref()
            .map(parse)
            .transpose()?;
        let to: IdentityId = parse(&row.get::<_, String>(2)?)?;
        if moved.contains(&record)
            || from.is_some_and(|from| decided.contains(&from))
            || !taken.insert(record)
        {
            continue;
        }
        ops.push(Op::Assign {
            record,
            from,
            to: Some(to),
        });
    }
    Ok(ops)
}
