//! Review items: what a reconcile run would not decide, kept in the store.
//!
//! A review item names observations that were left without an identity and
//! says why. It is evidence, like a judgement: it changes no identity, and
//! nothing is committed on its account. It is open while any of its
//! observations has no identity, and it closes in one of three ways:
//!
//! - `resolved`: a changeset gave every observation an identity. Any
//!   changeset does this, whether a reviewer wrote it or a later run did.
//! - `superseded`: a later run examined the observations again and raised an
//!   item that differs, for example because they now match a third identity.
//! - `dismissed`: a reviewer closed it without placing its observations.
//!
//! The observations of open items are in the scope of every later match run
//! of their entity type, so an item is looked at again whenever state or
//! rules may have changed. A run that reaches the same conclusion leaves the
//! item as it is. The observations of a dismissed item are not looked at
//! again: they stay without an identity until a changeset places them.

use std::collections::BTreeSet;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{
    Actor, Constraint, EntityType, IdentityId, IdentityStatus, ObservationId, ReviewItem, RunId,
    Seq, StateError, Verdict,
};

use crate::error::{Error, Result};
use crate::lookup::{self, IdentitySummary, ObservationRecord};
use crate::store::{self, parse};
use crate::time::rfc3339;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReviewKind {
    /// The observations match more than one identity, so placing them would
    /// merge identities.
    PossibleMerge,
    /// The evidence about the observations contradicts itself.
    Conflict,
}

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
pub struct ReviewRecord {
    pub id: u64,
    /// The reconcile run that raised it.
    pub run: RunId,
    pub entity_type: EntityType,
    pub kind: ReviewKind,
    /// The observations left without an identity.
    pub observations: Vec<ObservationId>,
    /// The identities they matched. Empty when the conflict is among the
    /// observations themselves.
    pub identities: Vec<IdentityId>,
    pub status: ReviewStatus,
    /// Resolved: the changeset after which every observation had an identity.
    pub resolved_seq: Option<Seq>,
    /// Superseded: the reconcile run that raised it again in another form.
    pub superseded_by: Option<RunId>,
    /// Dismissed: who closed it, when and why.
    #[serde(default)]
    pub dismissed: Option<Dismissal>,
}

/// A reviewer's decision to close a review item without placing its
/// observations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Dismissal {
    pub by: Actor,
    pub at: String,
    pub reason: Option<String>,
}

/// A review item with what a reviewer reads to decide it: the observations
/// it holds back, the identities they matched, what was judged between
/// them, and what was already decided about them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReviewCase {
    pub item: ReviewRecord,
    /// The observations the item names, as they are now, in the item's
    /// order.
    pub observations: Vec<ObservationRecord>,
    /// The identities the item names, as they stand now, ordered by ID. One
    /// that was merged away since is shown as the identity that stands for
    /// it, so there may be fewer than the item names.
    pub candidates: Vec<Candidate>,
    /// What the latest run to judge each pair made of it, for every pair of
    /// one of the item's observations with another of them or with a member
    /// of a candidate. Only runs that were reconciled count.
    pub judgements: Vec<PairJudgement>,
    /// The constraints in force that bear on the decision, oldest first:
    /// those that name one of the item's observations, and those between
    /// members of two of the candidates. A changeset that would break one
    /// is refused, so a `cannot_link` between two candidates means they
    /// cannot be merged, and one between an observation and a member of a
    /// candidate means it cannot be placed there.
    pub constraints: Vec<CaseConstraint>,
}

/// A constraint in force, with where its two observations are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CaseConstraint {
    #[serde(flatten)]
    pub constraint: Constraint,
    /// The log position of the changeset that made it.
    pub from: Seq,
    /// The identities its observations belong to now, that of `a` first.
    /// `None` for one that has no identity.
    pub identities: [Option<IdentityId>; 2],
}

/// An identity that the observations of a review item matched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Candidate {
    pub identity: IdentitySummary,
    /// Its members: those a judgement of the case names before the others,
    /// and at most [`CASE_MEMBERS`] of them. `identity.members` says how
    /// many there are.
    pub members: Vec<ObservationRecord>,
}

/// The most members of a candidate that a case shows.
pub const CASE_MEMBERS: u64 = 25;

/// What a run judged about one of a review item's observations and another
/// observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PairJudgement {
    /// One of the item's observations.
    pub observation: ObservationId,
    /// What it was compared with.
    pub other: ObservationId,
    /// The identity `other` belongs to now: one of the candidates, unless
    /// `other` is another of the item's observations. A candidate's members
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
    /// Only the items that name this observation.
    pub observation: Option<ObservationId>,
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
pub(crate) fn list(conn: &Connection, filter: &ReviewFilter) -> Result<Vec<ReviewRecord>> {
    let mut statement = conn.prepare(
        "SELECT review_id, run_id, entity_type, kind,
                array_to_string(observation_ids::VARCHAR[], ','),
                array_to_string(identity_ids::VARCHAR[], ','),
                status, closed_seq, closed_run, closed_by, epoch_us(closed_at), closed_reason
         FROM spool.review_item
         WHERE ($1::VARCHAR IS NULL OR status = $1::VARCHAR)
           AND ($2::VARCHAR IS NULL OR entity_type = $2::VARCHAR)
           AND ($3::BIGINT IS NULL OR review_id = $3::BIGINT)
           AND ($4::VARCHAR IS NULL OR list_contains(observation_ids, $4::UUID))
         ORDER BY review_id",
    )?;
    let mut rows = statement.query(params![
        filter.status.map(ReviewStatus::as_str),
        filter.entity_type.as_ref().map(EntityType::as_str),
        filter.id.map(|id| id as i64),
        filter.observation.map(|id| id.to_string()),
    ])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let unknown =
            |what: &str, value: &str| StateError::new(format!("unknown review {what} {value:?}"));
        let kind: String = row.get(3)?;
        let status: String = row.get(6)?;
        items.push(ReviewRecord {
            id: row.get::<_, i64>(0)? as u64,
            run: RunId(row.get::<_, i64>(1)? as u64),
            entity_type: EntityType::new(row.get::<_, String>(2)?).map_err(StateError::new)?,
            kind: match kind.as_str() {
                "possible_merge" => ReviewKind::PossibleMerge,
                "conflict" => ReviewKind::Conflict,
                other => return Err(unknown("kind", other).into()),
            },
            observations: split(row.get(4)?)
                .iter()
                .map(|id| parse(id))
                .collect::<Result<_>>()?,
            identities: split(row.get(5)?)
                .iter()
                .map(|id| parse(id))
                .collect::<Result<_>>()?,
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

/// A review item with the records it is about.
pub(crate) fn case(conn: &Connection, id: u64) -> Result<ReviewCase> {
    let only = ReviewFilter {
        id: Some(id),
        ..ReviewFilter::default()
    };
    let item = list(conn, &only)?
        .pop()
        .ok_or(Error::UnknownReviewItem(id))?;
    let observations = lookup::observations_by_id(conn, &item.observations)?;

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

    let mut judgements = Vec::new();
    {
        // A pair is stored with the smaller ID on the left, so one of the
        // item's observations may be on either side.
        let mut statement = conn.prepare(&format!(
            "WITH held AS (SELECT unnest({held}) AS observation_id),
                  near AS (
                    SELECT observation_id FROM held
                    UNION
                    SELECT observation_id FROM spool.membership
                    WHERE to_seq IS NULL AND list_contains({candidates}, identity_id)),
                  judged AS (
                    SELECT j.*, j.left_id IN (SELECT observation_id FROM held) AS left_held
                    FROM spool.judgement j
                    JOIN spool.run r ON r.run_id = j.run_id AND r.status IN ('succeeded', 'drafted')
                    WHERE j.left_id IN (SELECT observation_id FROM near)
                      AND j.right_id IN (SELECT observation_id FROM near)
                      AND (j.left_id IN (SELECT observation_id FROM held)
                           OR j.right_id IN (SELECT observation_id FROM held))
                    QUALIFY j.run_id = max(j.run_id) OVER (PARTITION BY j.left_id, j.right_id))
             SELECT (CASE WHEN left_held THEN left_id ELSE right_id END)::VARCHAR,
                    (CASE WHEN left_held THEN right_id ELSE left_id END)::VARCHAR,
                    run_id, verdict, score, detail::VARCHAR, m.identity_id::VARCHAR
             FROM judged
             LEFT JOIN spool.membership m
               ON m.observation_id = (CASE WHEN left_held THEN right_id ELSE left_id END)
              AND m.to_seq IS NULL
             ORDER BY 1, 2, verdict, score",
            held = lookup::uuid_list(&item.observations),
            candidates = lookup::uuid_list(&current),
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            judgements.push(PairJudgement {
                observation: parse(&row.get::<_, String>(0)?)?,
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
            "SELECT c.observation_a::VARCHAR, c.observation_b::VARCHAR, c.kind, c.from_seq,
                    a.identity_id::VARCHAR, b.identity_id::VARCHAR
             FROM spool.constraint_link c
             LEFT JOIN spool.membership a
               ON a.observation_id = c.observation_a AND a.to_seq IS NULL
             LEFT JOIN spool.membership b
               ON b.observation_id = c.observation_b AND b.to_seq IS NULL
             WHERE c.to_seq IS NULL
               AND (list_contains({held}, c.observation_a)
                    OR list_contains({held}, c.observation_b)
                    OR (a.identity_id <> b.identity_id
                        AND list_contains({candidates}, a.identity_id)
                        AND list_contains({candidates}, b.identity_id)))
             ORDER BY c.from_seq, least(c.observation_a, c.observation_b),
                      greatest(c.observation_a, c.observation_b)",
            held = lookup::uuid_list(&item.observations),
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
            let stored_first: ObservationId = parse(&row.get::<_, String>(0)?)?;
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

    let judged: Vec<ObservationId> = judgements
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
        observations,
        candidates,
        judgements,
        constraints,
    })
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
                 VALUES (?, ?, ?, ?, {}, {}, 'open', NULL, NULL, NULL, NULL, NULL)",
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

/// Closes an open item as dismissed. Its observations keep having no
/// identity, and later runs no longer examine them on its account.
pub(crate) fn dismiss(
    conn: &Connection,
    id: u64,
    actor: &Actor,
    at_micros: i64,
    reason: Option<&str>,
) -> Result<ReviewRecord> {
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
