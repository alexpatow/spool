//! Review items: what a reconcile run would not decide, kept in the store.
//!
//! A review item names observations that were left without an identity and
//! says why. It is evidence, like a judgement: it changes no identity, and
//! nothing is committed on its account. It is open while any of its
//! observations has no identity, and it closes in one of two ways:
//!
//! - `resolved`: a changeset gave every observation an identity. Any
//!   changeset does this, whether a reviewer wrote it or a later run did.
//! - `superseded`: a later run examined the observations again and raised an
//!   item that differs, for example because they now match a third identity.
//!
//! The observations of open items are in the scope of every later match run
//! of their entity type, so an item is looked at again whenever state or
//! rules may have changed. A run that reaches the same conclusion leaves the
//! item as it is.

use std::collections::BTreeSet;

use duckdb::{Connection, params};
use spool_core::{EntityType, ReviewItem, RunId, Seq};

use crate::error::Result;

/// A review item reduced to what identifies it: its kind, its observations
/// and its identities, each list in order.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shape {
    kind: &'static str,
    observations: Vec<String>,
    identities: Vec<String>,
}

impl Shape {
    fn of(item: &ReviewItem) -> Self {
        let (kind, observations, identities) = match item {
            ReviewItem::PossibleMerge {
                observations,
                identities,
            } => (
                "possible_merge",
                observations,
                identities.iter().map(ToString::to_string).collect(),
            ),
            ReviewItem::Conflict {
                observations,
                identity,
            } => (
                "conflict",
                observations,
                identity.iter().map(ToString::to_string).collect(),
            ),
        };
        Self {
            kind,
            observations: sorted(observations.iter().map(ToString::to_string).collect()),
            identities: sorted(identities),
        }
    }
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids.dedup();
    ids
}

/// A list of IDs as a SQL value. The IDs are UUIDs the engine printed, so
/// they are safe to splice.
fn uuid_list(ids: &[String]) -> String {
    let quoted: Vec<String> = ids.iter().map(|id| format!("'{id}'")).collect();
    format!("[{}]::UUID[]", quoted.join(", "))
}

fn split(joined: Option<String>) -> Vec<String> {
    sorted(
        joined
            .unwrap_or_default()
            .split(',')
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// The open review items of an entity type, oldest first.
pub(crate) fn open_ids(conn: &Connection, entity_type: &EntityType) -> Result<Vec<u64>> {
    let mut statement = conn.prepare(
        "SELECT review_id FROM spool.review_item
         WHERE status = 'open' AND entity_type = ? ORDER BY review_id",
    )?;
    let mut rows = statement.query([entity_type.as_str()])?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next()? {
        ids.push(row.get::<_, i64>(0)? as u64);
    }
    Ok(ids)
}

/// A SQL query for the observations of the given review items that still
/// have no identity, or `None` if there are no items.
pub(crate) fn unassigned_observations(review_items: &[u64]) -> Option<String> {
    if review_items.is_empty() {
        return None;
    }
    let ids: Vec<String> = review_items.iter().map(ToString::to_string).collect();
    Some(format!(
        "SELECT r.observation_id
         FROM (SELECT unnest(observation_ids) AS observation_id
               FROM spool.review_item WHERE review_id IN ({})) r
         WHERE NOT EXISTS (
           SELECT 1 FROM spool.membership m
           WHERE m.observation_id = r.observation_id AND m.to_seq IS NULL)",
        ids.join(", ")
    ))
}

/// Stores the review items a reconcile run raised. An item that is already
/// open in the same form is left alone. An open item that shares an
/// observation with a new, different item is closed as superseded.
pub(crate) fn record(
    conn: &Connection,
    entity_type: &EntityType,
    run: RunId,
    items: &[ReviewItem],
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let mut open: Vec<(i64, Shape)> = Vec::new();
    {
        let mut statement = conn.prepare(
            "SELECT review_id, kind,
                    array_to_string(observation_ids::VARCHAR[], ','),
                    array_to_string(identity_ids::VARCHAR[], ',')
             FROM spool.review_item
             WHERE status = 'open' AND entity_type = ? ORDER BY review_id",
        )?;
        let mut rows = statement.query([entity_type.as_str()])?;
        while let Some(row) = rows.next()? {
            let kind = match row.get::<_, String>(1)?.as_str() {
                "possible_merge" => "possible_merge",
                _ => "conflict",
            };
            open.push((
                row.get(0)?,
                Shape {
                    kind,
                    observations: split(row.get(2)?),
                    identities: split(row.get(3)?),
                },
            ));
        }
    }
    let mut next_id: i64 = conn.query_row(
        "SELECT coalesce(max(review_id), 0) + 1 FROM spool.review_item",
        [],
        |row| row.get(0),
    )?;

    let mut superseded = BTreeSet::new();
    for item in items {
        let shape = Shape::of(item);
        if open.iter().any(|(_, known)| *known == shape) {
            continue;
        }
        for (id, known) in &open {
            let overlaps = known
                .observations
                .iter()
                .any(|observation| shape.observations.contains(observation));
            if overlaps {
                superseded.insert(*id);
            }
        }
        conn.execute(
            &format!(
                "INSERT INTO spool.review_item
                 VALUES (?, ?, ?, ?, {}, {}, 'open', NULL, NULL)",
                uuid_list(&shape.observations),
                uuid_list(&shape.identities)
            ),
            params![next_id, run.0 as i64, entity_type.as_str(), shape.kind],
        )?;
        next_id += 1;
    }

    if !superseded.is_empty() {
        let ids: Vec<String> = superseded.iter().map(ToString::to_string).collect();
        conn.execute(
            &format!(
                "UPDATE spool.review_item SET status = 'superseded', closed_run = ?
                 WHERE review_id IN ({})",
                ids.join(", ")
            ),
            [run.0 as i64],
        )?;
    }
    Ok(())
}

/// Closes the open items whose observations all have an identity, as
/// resolved by the changeset at `seq`. Runs inside the transaction that
/// commits that changeset.
pub(crate) fn resolve_assigned(conn: &Connection, seq: Seq) -> Result<()> {
    conn.execute(
        "UPDATE spool.review_item SET status = 'resolved', closed_seq = ?
         WHERE status = 'open' AND review_id NOT IN (
           SELECT r.review_id
           FROM (SELECT review_id, unnest(observation_ids) AS observation_id
                 FROM spool.review_item WHERE status = 'open') r
           WHERE NOT EXISTS (
             SELECT 1 FROM spool.membership m
             WHERE m.observation_id = r.observation_id AND m.to_seq IS NULL))",
        [seq.0 as i64],
    )?;
    Ok(())
}
