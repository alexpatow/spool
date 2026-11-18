//! Review items: what the engine would not decide, kept in the store.
//!
//! Most review items come from a reconcile run. Such an item names records
//! that were left without an identity and says why. It is evidence, like a
//! judgement: it changes no identity, and nothing is committed on its
//! account. It is open while any of its records waits for an identity, and
//! it closes in one of three ways:
//!
//! - `resolved`: a changeset gave every record an identity. Any
//!   changeset does this, whether a reviewer wrote it or a later run did.
//! - `superseded`: a later run examined the records again and raised an
//!   item that differs, for example because they now match a third identity.
//!   Or their source took back the records that still waited, which
//!   leaves nothing to ask.
//! - `dismissed`: a reviewer closed it without placing its records.
//!
//! The records of open items are in the scope of every later match run
//! of their entity type, so an item is looked at again whenever state or
//! rules may have changed. A run that reaches the same conclusion leaves the
//! item as it is. The records of a dismissed item are not looked at
//! again: they stay without an identity until a changeset places them.
//!
//! A record its source took back does not wait for an identity. An item
//! that names one stays open for the sake of its other records, and
//! is resolved when those have an identity.
//!
//! A `possible_split` is the other way round: its records have an identity,
//! and the question is whether they should share it. A retraction raises
//! one, and so does a record that comes back. The `possible_split` module
//! says when, and how it closes. None
//! of the above applies to it: its records wait for nothing, so no match
//! run examines them on its account.

use std::collections::BTreeSet;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{
    Actor, Constraint, EntityType, IdentityId, IdentityStatus, RecordId, ReviewItem, RunId, Seq,
    StateError, Verdict,
};

use crate::error::{Error, Result};
use crate::lookup::{self, IdentitySummary, RecordEntry};
use crate::possible_split;
use crate::retract;
use crate::store::{self, parse};
use crate::time::rfc3339;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReviewKind {
    /// The records match more than one identity, so placing them would
    /// merge identities.
    PossibleMerge,
    /// The evidence about the records contradicts itself.
    Conflict,
    /// The records share an identity, and nothing the store knows holds
    /// them together any more: what did was a record its source took back.
    /// They may be more than one thing.
    PossibleSplit,
}

impl ReviewKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ReviewKind::PossibleMerge => "possible_merge",
            ReviewKind::Conflict => "conflict",
            ReviewKind::PossibleSplit => "possible_split",
        }
    }
}

/// A condition on a row of `spool.review_item` that holds for the kinds that
/// are about records without an identity.
pub(crate) const ABOUT_WAITING: &str = "kind <> 'possible_split'";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Open,
    Resolved,
    Superseded,
    Dismissed,
}

impl ReviewStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            ReviewStatus::Open => "open",
            ReviewStatus::Resolved => "resolved",
            ReviewStatus::Superseded => "superseded",
            ReviewStatus::Dismissed => "dismissed",
        }
    }
}

/// A review item as the store keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReviewEntry {
    pub id: u64,
    /// The run that raised it: a reconcile run. For a `possible_split`, the
    /// retract run that took back what held the records together, or the
    /// load run that brought a record back to where what held it is
    /// still gone.
    pub run: RunId,
    pub entity_type: EntityType,
    pub kind: ReviewKind,
    /// The records left without an identity. For a `possible_split`, the
    /// members of the identity that nothing holds together: every record
    /// in `groups`.
    pub records: Vec<RecordId>,
    /// The identities they matched. Empty when the conflict is among the
    /// records themselves. For a `possible_split`, the one identity the
    /// records are in. While the item is open that is the identity as it is
    /// now: when the identity is merged into another, the item names the
    /// one that stands for both.
    pub identities: Vec<IdentityId>,
    /// Only a `possible_split` has these: the records of its identity as
    /// the groups that are each still held together. Nothing the store
    /// knows says that two of the groups are one thing. While the item is
    /// open they are the identity's records as they are now; once it is
    /// closed, as they were last. The largest group is first.
    #[serde(default)]
    pub groups: Vec<Vec<RecordId>>,
    pub status: ReviewStatus,
    /// Resolved: the changeset after which every record had an identity.
    /// For a `possible_split`, the changeset after which nothing was left
    /// to ask: it took the groups apart, or it added what holds them
    /// together, or it merged the identity with one whose item asks for
    /// both.
    pub resolved_seq: Option<Seq>,
    /// Superseded: the reconcile run that raised it again in another form,
    /// or the retract run that took back the records it asked about. For a
    /// `possible_split`, the reconcile run whose judgements hold the
    /// groups together.
    pub superseded_by: Option<RunId>,
    /// Dismissed: who closed it, when and why.
    #[serde(default)]
    pub dismissed: Option<Dismissal>,
}

/// A reviewer's decision to close a review item without placing its
/// records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Dismissal {
    pub by: Actor,
    pub at: String,
    pub reason: Option<String>,
}

/// A review item with what a reviewer reads to decide it: the records
/// it holds back, the identities they matched, what was judged between
/// them, and what was already decided about them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReviewCase {
    pub item: ReviewEntry,
    /// The records the item names, as they are now, in the item's
    /// order. One with `retracted` set was taken back by its source: it
    /// does not wait for a decision, and a changeset that gives it an
    /// identity is refused. The decision is about the others.
    pub records: Vec<RecordEntry>,
    /// The identities the item names, as they stand now, ordered by ID. One
    /// that was merged away since is shown as the identity that stands for
    /// it, so there may be fewer than the item names.
    pub candidates: Vec<Candidate>,
    /// What the latest run to judge each pair made of it, for every pair of
    /// one of the item's records with another of them, with a member
    /// of a candidate, or with a record in `taken_back`. Only runs that
    /// were reconciled count.
    pub judgements: Vec<PairJudgement>,
    /// Only a `possible_split` has these: the records that were taken out
    /// of the identity when their source took them back, and have not come
    /// back. What held the groups together went through them, and
    /// `judgements` has what was judged between them and the item's
    /// records.
    #[serde(default)]
    pub taken_back: Vec<RecordEntry>,
    /// The constraints in force that bear on the decision, oldest first:
    /// those that name one of the item's records, and those between
    /// members of two of the candidates. A changeset that would break one
    /// is refused, so a `cannot_link` between two candidates means they
    /// cannot be merged, and one between a record and a member of a
    /// candidate means it cannot be placed there.
    pub constraints: Vec<CaseConstraint>,
}

/// A constraint in force, with where its two records are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CaseConstraint {
    #[serde(flatten)]
    pub constraint: Constraint,
    /// The log position of the changeset that made it.
    pub from: Seq,
    /// The identities its records belong to now, that of `a` first.
    /// `None` for one that has no identity.
    pub identities: [Option<IdentityId>; 2],
}

/// An identity that the records of a review item matched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Candidate {
    pub identity: IdentitySummary,
    /// Its members: those a judgement of the case names before the others,
    /// and at most [`CASE_MEMBERS`] of them. `identity.members` says how
    /// many there are.
    pub members: Vec<RecordEntry>,
}

/// The most members of a candidate that a case shows.
pub const CASE_MEMBERS: u64 = 25;

/// What a run judged about one of a review item's records and another
/// record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PairJudgement {
    /// One of the item's records.
    pub record: RecordId,
    /// What it was compared with.
    pub other: RecordId,
    /// The identity `other` belongs to now: one of the candidates, unless
    /// `other` is another of the item's records, or a record that was
    /// taken back, which has none. A candidate's members
    /// are not all in the case, so this, not the members, says which
    /// candidate a judgement is about.
    pub identity: Option<IdentityId>,
    /// The match run that judged.
    pub run: RunId,
    pub verdict: Verdict,
    /// On the strategy's own scale.
    pub score: Option<f64>,
    /// What the strategy kept with the judgement. The built-in `rules`
    /// keeps the names of the rules that held.
    pub detail: Option<serde_json::Value>,
}

/// Which review items to list. The default lists every item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewFilter {
    pub status: Option<ReviewStatus>,
    pub entity_type: Option<EntityType>,
    /// Only the item with this ID.
    pub id: Option<u64>,
    /// Only the items that name this record.
    pub record: Option<RecordId>,
}

impl ReviewFilter {
    /// The items that wait for a decision.
    pub fn open() -> Self {
        Self {
            status: Some(ReviewStatus::Open),
            ..Self::default()
        }
    }
}

/// Review items, oldest first.
pub(crate) fn list(conn: &Connection, filter: &ReviewFilter) -> Result<Vec<ReviewEntry>> {
    let mut statement = conn.prepare(
        "SELECT review_id, run_id, entity_type, kind,
                array_to_string(record_ids::VARCHAR[], ','),
                array_to_string(identity_ids::VARCHAR[], ','),
                status, closed_seq, closed_run, closed_by, epoch_us(closed_at), closed_reason,
                to_json(groups)::VARCHAR
         FROM spool.review_item
         WHERE ($1::VARCHAR IS NULL OR status = $1::VARCHAR)
           AND ($2::VARCHAR IS NULL OR entity_type = $2::VARCHAR)
           AND ($3::BIGINT IS NULL OR review_id = $3::BIGINT)
           AND ($4::VARCHAR IS NULL OR list_contains(record_ids, $4::UUID))
         ORDER BY review_id",
    )?;
    let mut rows = statement.query(params![
        filter.status.map(ReviewStatus::as_str),
        filter.entity_type.as_ref().map(EntityType::as_str),
        filter.id.map(|id| id as i64),
        filter.record.map(|id| id.to_string()),
    ])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let unknown =
            |what: &str, value: &str| StateError::new(format!("unknown review {what} {value:?}"));
        let kind: String = row.get(3)?;
        let status: String = row.get(6)?;
        items.push(ReviewEntry {
            id: row.get::<_, i64>(0)? as u64,
            run: RunId(row.get::<_, i64>(1)? as u64),
            entity_type: EntityType::new(row.get::<_, String>(2)?).map_err(StateError::new)?,
            kind: match kind.as_str() {
                "possible_merge" => ReviewKind::PossibleMerge,
                "conflict" => ReviewKind::Conflict,
                "possible_split" => ReviewKind::PossibleSplit,
                other => return Err(unknown("kind", other).into()),
            },
            records: split(row.get(4)?)
                .iter()
                .map(|id| parse(id))
                .collect::<Result<_>>()?,
            identities: split(row.get(5)?)
                .iter()
                .map(|id| parse(id))
                .collect::<Result<_>>()?,
            groups: match row.get::<_, Option<String>>(12)? {
                Some(groups) => serde_json::from_str(&groups).map_err(StateError::new)?,
                None => Vec::new(),
            },
            status: match status.as_str() {
                "open" => ReviewStatus::Open,
                "resolved" => ReviewStatus::Resolved,
                "superseded" => ReviewStatus::Superseded,
                "dismissed" => ReviewStatus::Dismissed,
                other => return Err(unknown("status", other).into()),
            },
            resolved_seq: row.get::<_, Option<i64>>(7)?.map(|seq| Seq(seq as u64)),
            superseded_by: row.get::<_, Option<i64>>(8)?.map(|run| RunId(run as u64)),
            dismissed: match row.get::<_, Option<String>>(9)? {
                Some(by) => Some(Dismissal {
                    by: parse(&by)?,
                    at: rfc3339(row.get::<_, Option<i64>>(10)?.unwrap_or_default()),
                    reason: row.get(11)?,
                }),
                None => None,
            },
        });
    }
    Ok(items)
}

/// A review item reduced to what identifies it: its kind, its records
/// and its identities, each list in order.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shape {
    kind: &'static str,
    records: Vec<String>,
    identities: Vec<String>,
}

impl Shape {
    fn of(item: &ReviewItem) -> Self {
        let (kind, records, identities) = match item {
            ReviewItem::PossibleMerge {
                records,
                identities,
            } => (
                "possible_merge",
                records,
                identities.iter().map(ToString::to_string).collect(),
            ),
            ReviewItem::Conflict { records, identity } => (
                "conflict",
                records,
                identity.iter().map(ToString::to_string).collect(),
            ),
        };
        Self {
            kind,
            records: sorted(records.iter().map(ToString::to_string).collect()),
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

/// A review item with the records it is about.
pub(crate) fn case(conn: &Connection, id: u64) -> Result<ReviewCase> {
    let only = ReviewFilter {
        id: Some(id),
        ..ReviewFilter::default()
    };
    let item = list(conn, &only)?
        .pop()
        .ok_or(Error::UnknownReviewItem(id))?;
    let records = lookup::records_by_id(conn, &item.records)?;

    let mut current: BTreeSet<IdentityId> = BTreeSet::new();
    for record in store::read_identities(conn, &item.identities)?
        .into_iter()
        .flatten()
    {
        current.insert(match record.status {
            IdentityStatus::Active => record.id,
            IdentityStatus::MergedInto(survivor) => survivor,
        });
    }
    let current: Vec<IdentityId> = current.into_iter().collect();
    let identities = lookup::identities_by_id(conn, &current)?;

    let taken_back = match item.kind {
        ReviewKind::PossibleSplit => possible_split::taken_back(conn, &current)?,
        ReviewKind::PossibleMerge | ReviewKind::Conflict => Vec::new(),
    };

    let mut judgements = Vec::new();
    {
        // A pair is stored with the smaller ID on the left, so one of the
        // item's records may be on either side.
        let mut statement = conn.prepare(&format!(
            "WITH held AS (SELECT unnest({held}) AS record_id),
                  near AS (
                    SELECT record_id FROM held
                    UNION
                    SELECT unnest({taken_back})
                    UNION
                    SELECT record_id FROM spool.membership
                    WHERE to_seq IS NULL AND list_contains({candidates}, identity_id)),
                  judged AS (
                    SELECT j.*, j.left_id IN (SELECT record_id FROM held) AS left_held
                    FROM spool.judgement j
                    JOIN spool.run r ON r.run_id = j.run_id AND r.status IN ('succeeded', 'drafted')
                    WHERE j.left_id IN (SELECT record_id FROM near)
                      AND j.right_id IN (SELECT record_id FROM near)
                      AND (j.left_id IN (SELECT record_id FROM held)
                           OR j.right_id IN (SELECT record_id FROM held))
                    QUALIFY j.run_id = max(j.run_id) OVER (PARTITION BY j.left_id, j.right_id))
             SELECT (CASE WHEN left_held THEN left_id ELSE right_id END)::VARCHAR,
                    (CASE WHEN left_held THEN right_id ELSE left_id END)::VARCHAR,
                    run_id, verdict, score, detail::VARCHAR, m.identity_id::VARCHAR
             FROM judged
             LEFT JOIN spool.membership m
               ON m.record_id = (CASE WHEN left_held THEN right_id ELSE left_id END)
              AND m.to_seq IS NULL
             ORDER BY 1, 2, verdict, score",
            held = lookup::uuid_list(&item.records),
            taken_back = lookup::uuid_list(&taken_back),
            candidates = lookup::uuid_list(&current),
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            judgements.push(PairJudgement {
                record: parse(&row.get::<_, String>(0)?)?,
                other: parse(&row.get::<_, String>(1)?)?,
                identity: row
                    .get::<_, Option<String>>(6)?
                    .map(|identity| parse(&identity))
                    .transpose()?,
                run: RunId(row.get::<_, i64>(2)? as u64),
                verdict: row
                    .get::<_, String>(3)?
                    .parse()
                    .map_err(|message: String| StateError::new(message))?,
                score: row.get(4)?,
                detail: row
                    .get::<_, Option<String>>(5)?
                    .map(|detail| serde_json::from_str(&detail).map_err(StateError::new))
                    .transpose()?,
            });
        }
    }

    let mut constraints = Vec::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT c.record_a::VARCHAR, c.record_b::VARCHAR, c.kind, c.from_seq,
                    a.identity_id::VARCHAR, b.identity_id::VARCHAR
             FROM spool.constraint_link c
             LEFT JOIN spool.membership a
               ON a.record_id = c.record_a AND a.to_seq IS NULL
             LEFT JOIN spool.membership b
               ON b.record_id = c.record_b AND b.to_seq IS NULL
             WHERE c.to_seq IS NULL
               AND (list_contains({held}, c.record_a)
                    OR list_contains({held}, c.record_b)
                    OR (a.identity_id <> b.identity_id
                        AND list_contains({candidates}, a.identity_id)
                        AND list_contains({candidates}, b.identity_id)))
             ORDER BY c.from_seq, least(c.record_a, c.record_b),
                      greatest(c.record_a, c.record_b)",
            held = lookup::uuid_list(&item.records),
            candidates = lookup::uuid_list(&current),
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let place = |column: usize| -> Result<Option<IdentityId>> {
                row.get::<_, Option<String>>(column)?
                    .map(|id| parse(&id))
                    .transpose()
            };
            let constraint = store::constraint(row, 0)?;
            // A constraint names the smaller ID first, however it is stored.
            let stored_first: RecordId = parse(&row.get::<_, String>(0)?)?;
            let identities = if constraint.a == stored_first {
                [place(4)?, place(5)?]
            } else {
                [place(5)?, place(4)?]
            };
            constraints.push(CaseConstraint {
                constraint,
                from: Seq(row.get::<_, i64>(3)? as u64),
                identities,
            });
        }
    }

    let judged: Vec<RecordId> = judgements
        .iter()
        .map(|judgement| judgement.other)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut candidates = Vec::new();
    for identity in identities {
        let members = lookup::members(conn, identity.id, &judged, CASE_MEMBERS)?;
        candidates.push(Candidate { identity, members });
    }
    Ok(ReviewCase {
        item,
        records,
        candidates,
        judgements,
        taken_back: lookup::records_by_id(conn, &taken_back)?,
        constraints,
    })
}

/// The open review items of an entity type that have records waiting for
/// an identity, oldest first. A match run examines those records again. A
/// `possible_split` is not among them: its records have an identity.
pub(crate) fn open_ids(conn: &Connection, entity_type: &EntityType) -> Result<Vec<u64>> {
    let mut statement = conn.prepare(&format!(
        "SELECT review_id FROM spool.review_item
         WHERE status = 'open' AND entity_type = ? AND {ABOUT_WAITING}
         ORDER BY review_id"
    ))?;
    let mut rows = statement.query([entity_type.as_str()])?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next()? {
        ids.push(row.get::<_, i64>(0)? as u64);
    }
    Ok(ids)
}

/// A SQL query for the records of the given review items that still
/// have no identity, or `None` if there are no items.
pub(crate) fn unassigned_records(review_items: &[u64]) -> Option<String> {
    if review_items.is_empty() {
        return None;
    }
    let ids: Vec<String> = review_items.iter().map(ToString::to_string).collect();
    Some(format!(
        "SELECT r.record_id
         FROM (SELECT unnest(record_ids) AS record_id
               FROM spool.review_item WHERE review_id IN ({})) r
         WHERE NOT EXISTS (
           SELECT 1 FROM spool.membership m
           WHERE m.record_id = r.record_id AND m.to_seq IS NULL)",
        ids.join(", ")
    ))
}

/// Stores the review items a reconcile run raised. An item that is already
/// open in the same form is left alone. An open item that shares an
/// record with a new, different item is closed as superseded.
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
        let mut statement = conn.prepare(&format!(
            "SELECT review_id, kind,
                    array_to_string(record_ids::VARCHAR[], ','),
                    array_to_string(identity_ids::VARCHAR[], ',')
             FROM spool.review_item
             WHERE status = 'open' AND entity_type = ? AND {ABOUT_WAITING}
             ORDER BY review_id"
        ))?;
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
                    records: split(row.get(2)?),
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
                .records
                .iter()
                .any(|record| shape.records.contains(record));
            if overlaps {
                superseded.insert(*id);
            }
        }
        conn.execute(
            &format!(
                "INSERT INTO spool.review_item
                 VALUES (?, ?, ?, ?, {}, {}, 'open', NULL, NULL, NULL, NULL, NULL, NULL)",
                uuid_list(&shape.records),
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

/// Closes the open items none of whose records waits for an identity
/// any longer, as resolved by the changeset at `seq`. Runs inside the
/// transaction that commits that changeset. A `possible_split` is not
/// about records that wait, and closes by its own rule.
pub(crate) fn resolve_assigned(conn: &Connection, seq: Seq) -> Result<()> {
    conn.execute(
        &format!(
            "UPDATE spool.review_item SET status = 'resolved', closed_seq = ?
             WHERE status = 'open' AND {ABOUT_WAITING} AND review_id NOT IN ({})",
            retract::WAITING
        ),
        [seq.0 as i64],
    )?;
    Ok(())
}

/// Closes an open item as dismissed. Its records keep having no
/// identity, and later runs no longer examine them on its account.
pub(crate) fn dismiss(
    conn: &Connection,
    id: u64,
    actor: &Actor,
    at_micros: i64,
    reason: Option<&str>,
) -> Result<ReviewEntry> {
    let only = ReviewFilter {
        id: Some(id),
        ..ReviewFilter::default()
    };
    let item = list(conn, &only)?
        .pop()
        .ok_or(Error::UnknownReviewItem(id))?;
    if item.status != ReviewStatus::Open {
        return Err(Error::Review {
            id,
            message: format!("it is {} and cannot be dismissed", item.status.as_str()),
        });
    }
    conn.execute(
        "UPDATE spool.review_item
         SET status = 'dismissed', closed_by = ?, closed_at = make_timestamptz(?),
             closed_reason = ?
         WHERE review_id = ?",
        params![actor.to_string(), at_micros, reason, id as i64],
    )?;
    Ok(list(conn, &only)?.remove(0))
}
