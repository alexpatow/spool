//! Recomputing identities: what the identities of an entity type would be
//! if every record were matched again, all at once, by the rules as they
//! are now, set against the identities there are.
//!
//! Identities stay as they are when rules change, so that an ID stored
//! elsewhere keeps meaning one thing. A recomputation is how a better rule
//! reaches records that are already placed: it is asked for, it shows what
//! would change first, and it changes nothing until it is applied.
//!
//! What the identities would be is what the default policy makes of the
//! type's records in an empty store, every record new, with the judgements
//! of the rules and what outranks them, strongest first:
//!
//! 1. a constraint in force: a `must_link` reads as `match` and a
//!    `cannot_link` as `no_match`;
//! 2. what people put together: the groups that a `must_link`, the versions
//!    of a row and the decisions of people and agents hold together, as a
//!    `possible_split` item reads them, and the records of a `possible_split`
//!    a reviewer dismissed. Each group stays together, and a rule's
//!    `no_match` inside one is not read;
//! 3. a version and the one it supersedes read as `match`;
//! 4. the judgements of the rules.
//!
//! A group people put together stays an identity even where the rules join
//! it with records that something says differ: only those records wait.
//!
//! Records whose source took them back have no part in it, and records a
//! reviewer dismissed from review that still have no identity keep having
//! none.
//!
//! Which identity a group of records becomes follows the rule a split
//! follows: an identity keeps its ID with the group that holds most of its
//! records, and where several identities have their most in one group, the
//! one with the most records there keeps its ID and the others are merged
//! into it, so that their IDs stand for it. A group in which no identity
//! has its most is a new identity.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use duckdb::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spool_core::{
    Actor, ChangesetId, Committed, ConstraintKind, DisjointSets, EntityType, EvidenceSummary,
    IdGenerator, IdentityId, MemoryState, NewPair, NewRecord, Op, Policy, RecordId, ReviewItem,
    RunId, SeededIds, Seq, Verdict, reconcile,
};

use crate::error::{Error, Result};
use crate::possible_split;
use crate::project::RuleSet;
use crate::retract::NOT_RETRACTED;
use crate::rules::{self, NOT_DISMISSED, SCOPE};
use crate::store::parse;

/// Where a preview keeps the candidate pairs and judgements of the rules.
/// They are temporary tables of a connection that reads, and are gone
/// when its transaction ends.
const CANDIDATES: &str = "spool_recompute_candidate";
const JUDGEMENTS: &str = "spool_recompute_judgement";

/// How many changes a preview lists when the caller does not say.
pub const DEFAULT_CHANGES: u64 = 10;

/// What a preview is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecomputeRequest {
    /// One entity type, or every type the manifest declares.
    pub entity_type: Option<EntityType>,
    /// The change to show whole, named by an identity in it.
    pub identity: Option<IdentityId>,
    /// How many changes to list for each type, largest first.
    pub limit: u64,
    /// How many of the largest changes to pass over first.
    pub offset: u64,
}

impl Default for RecomputeRequest {
    fn default() -> Self {
        Self {
            entity_type: None,
            identity: None,
            limit: DEFAULT_CHANGES,
            offset: 0,
        }
    }
}

/// What the identities would be under the rules as they are now, against
/// the identities there are. Nothing was changed to find out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecomputeReport {
    /// Names this result: applying it takes this, and refuses if what it
    /// finds then is not the same.
    pub plan: String,
    /// One entry per entity type, in the order the manifest declares them.
    pub types: Vec<TypeRecompute>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TypeRecompute {
    pub entity_type: EntityType,
    pub summary: RecomputeSummary,
    /// The decisions of people that shaped the result or stopped it.
    pub decisions: DecisionSummary,
    /// How many changes there are in all.
    pub changes_total: u64,
    /// The changes that were asked for, largest first.
    pub changes: Vec<Change>,
    /// The change that holds the identity that was asked about, whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<ChangeDetail>,
}

/// The counts of what would change for one entity type.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecomputeSummary {
    /// Records the recomputation placed: every record of the type that is
    /// not taken back, other than those a reviewer dismissed from review
    /// that have no identity.
    pub records: u64,
    /// Identities with records now.
    pub identities_now: u64,
    /// Identities with records after.
    pub identities_after: u64,
    /// Identities that would keep exactly the records they have.
    pub unchanged: u64,
    /// Identities whose records would go to more than one identity.
    pub splitting: u64,
    /// Identities after that would take records from more than one
    /// identity of now.
    pub merging: u64,
    /// Identities that would keep their ID and gain records.
    pub gaining: u64,
    /// Identities that would keep their ID and lose records.
    pub losing: u64,
    /// Identities that would be merged into another: their ID would stand
    /// for it.
    pub merged_away: u64,
    /// Identities that would be left with no records, because every one
    /// would wait for review.
    pub emptied: u64,
    /// Identities that would be made.
    pub new_identities: u64,
    /// Records that would be in another identity than now, by a move or a
    /// merge. Records that would gain or lose an identity are not counted.
    pub records_moved: u64,
    /// Records without an identity that would get one.
    pub records_placed: u64,
    /// Records with an identity that would wait for review without one.
    pub records_unplaced: u64,
    /// Records that would wait for review without an identity, in all.
    pub records_waiting: u64,
    /// The `conflict` review items those records would be left in.
    pub review_items: u64,
    /// Open review items whose records would all have an identity, and
    /// which would close.
    pub review_items_closed: u64,
    /// Pairs of records that share an identity now.
    pub pairs_now: u64,
    /// Pairs that would share one after.
    pub pairs_after: u64,
    /// Pairs that share one now and would after.
    pub pairs_kept: u64,
}

/// What people decided, as far as it reaches this result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DecisionSummary {
    /// `must_link` constraints in force between records of the type. Each
    /// keeps its two records in one identity.
    pub must_link: u64,
    /// `cannot_link` constraints in force between records of the type. Each
    /// keeps its two records apart.
    pub cannot_link: u64,
    /// Groups of records that people's decisions hold together, of more
    /// than one record. Each stays together whatever the rules say.
    pub held_together: u64,
    /// `possible_split` items a reviewer dismissed, whose records stay
    /// together.
    pub dismissed_splits: u64,
    /// Groups left waiting for review because a `cannot_link` contradicts
    /// what the rules join.
    pub blocked_by_constraints: u64,
    /// Groups left waiting for review because a `no_match` rule
    /// contradicts what the rules join.
    pub blocked_by_rules: u64,
    /// Changes that touch an identity a changeset by a person or an agent
    /// shaped. Such a change may undo what they did.
    pub changes_shaped_by_people: u64,
    /// Changes that touch records a draft placed that a reviewer approved.
    /// An approved draft binds no more than the run that drafted it.
    pub changes_with_approved_drafts: u64,
}

/// One connected part of what would change: the identities of now and
/// after that share records, with no record shared with any other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Change {
    /// Its place among the type's changes, the largest first, from 1.
    pub rank: u64,
    pub kind: ChangeKind,
    /// The records in it.
    pub records: u64,
    /// Those whose identity would be another than now, or none.
    pub moved: u64,
    /// The identities of now, the largest first. One without an ID stands
    /// for records that have no identity.
    pub before: Vec<Before>,
    /// What there would be after, the largest first.
    pub after: Vec<After>,
    /// Changesets by people or agents that shaped an identity in it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shaped_by: Vec<PersonDecision>,
    /// How many of its records a draft placed that a reviewer approved.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub approved_drafts: u64,
    /// What stops a group in it from being an identity: two of its records
    /// that something says differ.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<Contradiction>,
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Records without an identity would become a new one.
    New,
    /// An identity would gain records that have no identity now.
    Gain,
    /// An identity would lose records to review.
    Lose,
    /// An identity's records would go to more than one identity.
    Split,
    /// Identities would become one.
    Merge,
    /// Several identities would become several others.
    Regroup,
    /// Every record of the identities in it would wait for review.
    Review,
}

impl ChangeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ChangeKind::New => "new",
            ChangeKind::Gain => "gain",
            ChangeKind::Lose => "lose",
            ChangeKind::Split => "split",
            ChangeKind::Merge => "merge",
            ChangeKind::Regroup => "regroup",
            ChangeKind::Review => "review",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Before {
    /// `None` for the records in the change that have no identity.
    pub identity: Option<IdentityId>,
    /// The label most of its records have.
    pub label: Option<String>,
    pub records: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct After {
    pub fate: Fate,
    /// The identity that keeps its ID. `None` for a new identity, whose ID
    /// is minted when the result is applied, and for records that wait.
    pub identity: Option<IdentityId>,
    pub label: Option<String>,
    pub records: u64,
    /// Identities of now that would be merged into this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merged: Vec<IdentityId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Fate {
    /// An identity of now, with its ID.
    Kept,
    /// A new identity.
    New,
    /// Records that would wait for review without an identity.
    Waiting,
}

/// A changeset by a person or an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PersonDecision {
    pub seq: Seq,
    pub actor: Actor,
    pub reason: Option<String>,
}

/// Two records of a group that something says are not the same thing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Contradiction {
    pub records: [RecordId; 2],
    /// A `cannot_link` that someone recorded, if that is what says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<PersonDecision>,
    /// The `no_match` rules that hold for the pair, if those say so.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

/// A change with each of its records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ChangeDetail {
    pub change: Change,
    /// Ordered by where they would go, then by source and key.
    pub records: Vec<RecordMove>,
    /// The constraints in force between its records.
    #[serde(default)]
    pub constraints: Vec<ConstraintNote>,
    /// Its records that drafts placed which reviewers approved.
    #[serde(default)]
    pub approved_drafts: Vec<ApprovedPlacement>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecordMove {
    pub record: RecordId,
    pub source: String,
    pub key: Option<String>,
    pub label: Option<String>,
    /// Its identity now.
    pub from: Option<IdentityId>,
    /// Where it would go: its place in the change's `after`, from 0.
    pub to: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConstraintNote {
    pub kind: ConstraintKind,
    pub records: [RecordId; 2],
    pub decision: PersonDecision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApprovedPlacement {
    pub record: RecordId,
    /// The draft, by its changeset ID.
    pub draft: ChangesetId,
    /// Where it was committed in the log.
    pub seq: Seq,
}

/// What applying a recomputation did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecomputeApplied {
    pub plan: String,
    pub types: Vec<AppliedType>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppliedType {
    pub entity_type: EntityType,
    /// The match run that judged every record by the rules. Its judgements
    /// are kept as any run's are.
    pub match_run: RunId,
    /// The reconcile run that wrote the changeset.
    pub reconcile_run: RunId,
    pub summary: RecomputeSummary,
    /// The changeset, or `None` when nothing changed.
    pub committed: Option<Committed>,
    /// The review items the records that wait were left in.
    pub review: Vec<ReviewItem>,
}

/// Lists every record a recomputation of `entity_type` places in the
/// scope table: those loaded before `before_run`, other than those taken
/// back and those a reviewer dismissed from review that have no identity.
pub(crate) fn create_scope(
    conn: &Connection,
    entity_type: &EntityType,
    before_run: RunId,
) -> Result<()> {
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {SCOPE} AS
         SELECT record_id FROM spool.record
         WHERE entity_type = '{entity_type}' AND load_run < {before_run} AND {NOT_RETRACTED}
           AND ({NOT_DISMISSED} OR record_id IN (
             SELECT record_id FROM spool.membership WHERE to_seq IS NULL))"
    ))?;
    Ok(())
}

/// Judges every pair the block rules find among the type's records into
/// temporary tables, for a preview. Returns the relation of judgements.
pub(crate) fn judge_for_preview(
    conn: &Connection,
    entity_type: &EntityType,
    rules: &RuleSet,
) -> Result<String> {
    create_scope(conn, entity_type, RunId(i64::MAX as u64))?;
    // Declared column by column: a copy made with `CREATE TABLE .. AS
    // SELECT .. WHERE false` read back the IDs of a large insert as null.
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {CANDIDATES} (
           run_id BIGINT NOT NULL, left_id UUID NOT NULL, right_id UUID NOT NULL, rule VARCHAR);
         CREATE OR REPLACE TEMP TABLE {JUDGEMENTS} (
           run_id BIGINT NOT NULL, left_id UUID NOT NULL, right_id UUID NOT NULL,
           verdict VARCHAR NOT NULL, score DOUBLE, detail JSON);"
    ))?;
    rules::generate_candidates_into(conn, entity_type, RunId(0), &rules.block, CANDIDATES)?;
    rules::judge_candidates_into(
        conn,
        entity_type,
        RunId(0),
        &rules.no_match,
        &rules.match_,
        (CANDIDATES, JUDGEMENTS),
    )?;
    Ok(JUDGEMENTS.to_owned())
}

/// Where a record would be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Place {
    Group(usize),
    Waiting(usize),
}

/// A recomputation of one type, worked out and not applied.
pub(crate) struct Outcome {
    entity_type: EntityType,
    /// Every record placed, in order.
    records: Vec<RecordId>,
    now: HashMap<RecordId, IdentityId>,
    /// The groups that would be identities, each ordered, ordered by their
    /// first record.
    groups: Vec<Vec<RecordId>>,
    /// The groups that would wait for review.
    waiting: Vec<Vec<RecordId>>,
    /// For each waiting group, a record of the group people put together
    /// that was kept out of it, if one was.
    kept_out: Vec<Option<RecordId>>,
    place: HashMap<RecordId, Place>,
    /// For each group, the identity that keeps its ID with it.
    keeper: Vec<Option<IdentityId>>,
    /// Identities merged into another: into which.
    merged: BTreeMap<IdentityId, IdentityId>,
    /// The `no_match` pairs inside each waiting group, with what says so.
    blocked: Vec<Vec<Contradiction>>,
    /// Each record's label, with when it was loaded.
    labels: HashMap<RecordId, (String, i64)>,
    shaped: BTreeMap<IdentityId, Vec<PersonDecision>>,
    approved: BTreeMap<RecordId, (ChangesetId, Seq)>,
    constraints: Vec<ConstraintNote>,
    held_together: u64,
    dismissed_splits: u64,
    open_items: Vec<Vec<RecordId>>,
    /// Pairs of records that share an identity now, counting members
    /// outside the recomputation, which keep their place.
    pairs_now: u64,
}

/// Works out what the identities of `entity_type` would be, from the
/// records in the scope table and the judgements in `judgements`, a
/// relation with the columns of `spool.judgement`.
#[expect(clippy::too_many_lines)]
pub(crate) fn compute(
    conn: &Connection,
    entity_type: &EntityType,
    judgements: &str,
) -> Result<Outcome> {
    let records: Vec<RecordId> = strings(
        conn,
        &format!("SELECT record_id::VARCHAR FROM {SCOPE} ORDER BY record_id"),
    )?
    .iter()
    .map(|id| parse(id))
    .collect::<Result<_>>()?;

    let mut now: HashMap<RecordId, IdentityId> = HashMap::new();
    let mut created: HashMap<IdentityId, i64> = HashMap::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT m.record_id::VARCHAR, m.identity_id::VARCHAR, i.created_seq
             FROM {SCOPE} s
             JOIN spool.membership m ON m.record_id = s.record_id AND m.to_seq IS NULL
             JOIN spool.identity i ON i.identity_id = m.identity_id"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let identity: IdentityId = parse(&row.get::<_, String>(1)?)?;
            now.insert(parse(&row.get::<_, String>(0)?)?, identity);
            created.insert(identity, row.get(2)?);
        }
    }
    let identities: Vec<IdentityId> = {
        let mut ids: Vec<IdentityId> = created.keys().copied().collect();
        ids.sort();
        ids
    };
    let pairs_now: u64 = conn.query_row(
        &format!(
            "SELECT coalesce(sum(n * (n - 1) // 2), 0)::BIGINT FROM (
               SELECT count(*) AS n FROM spool.membership
               WHERE to_seq IS NULL AND identity_id IN (
                 SELECT m.identity_id FROM {SCOPE} s
                 JOIN spool.membership m ON m.record_id = s.record_id AND m.to_seq IS NULL)
               GROUP BY identity_id)"
        ),
        [],
        |row| row.get::<_, i64>(0),
    )? as u64;

    // The evidence, weakest first: what is read later wins.
    let mut verdicts: BTreeMap<(RecordId, RecordId), Verdict> = BTreeMap::new();
    let mut rule_names: HashMap<(RecordId, RecordId), Vec<String>> = HashMap::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT j.left_id::VARCHAR, j.right_id::VARCHAR, j.verdict,
                    coalesce(to_json(j.detail->'no_match')::VARCHAR, '[]')
             FROM {judgements} j
             WHERE j.verdict <> 'unsure'
               AND j.left_id IN (SELECT record_id FROM {SCOPE})
               AND j.right_id IN (SELECT record_id FROM {SCOPE})"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let pair = ordered(
                parse(&row.get::<_, String>(0)?)?,
                parse(&row.get::<_, String>(1)?)?,
            );
            let verdict: Verdict = row
                .get::<_, String>(2)?
                .parse()
                .map_err(|message: String| Error::Recompute(message))?;
            if verdict == Verdict::NoMatch {
                let names: Vec<String> =
                    serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default();
                rule_names.insert(pair, names);
            }
            verdicts.insert(pair, verdict);
        }
    }
    {
        let mut statement = conn.prepare(&format!(
            "SELECT record_id::VARCHAR, supersedes::VARCHAR FROM spool.record
             WHERE entity_type = '{entity_type}' AND supersedes IS NOT NULL
               AND record_id IN (SELECT record_id FROM {SCOPE})
               AND supersedes IN (SELECT record_id FROM {SCOPE})"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let pair = ordered(
                parse(&row.get::<_, String>(0)?)?,
                parse(&row.get::<_, String>(1)?)?,
            );
            verdicts.insert(pair, Verdict::Match);
        }
    }

    // What people put together stays together.
    let mut held = DisjointSets::new(records.iter().copied());
    let mut held_together = 0;
    for groups in possible_split::held_together(conn, &identities)?.into_values() {
        for group in groups.into_iter().filter(|group| group.len() > 1) {
            held_together += 1;
            for record in &group[1..] {
                held.join(group[0], *record);
            }
        }
    }
    let mut dismissed_splits = 0;
    for item in strings(
        conn,
        &format!(
            "SELECT to_json(record_ids)::VARCHAR FROM spool.review_item
             WHERE entity_type = '{entity_type}' AND kind = 'possible_split'
               AND status = 'dismissed'
             ORDER BY review_id"
        ),
    )? {
        let ids: Vec<RecordId> = serde_json::from_str::<Vec<String>>(&item)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect();
        // Those of its records that still share an identity.
        let mut by_identity: BTreeMap<IdentityId, Vec<RecordId>> = BTreeMap::new();
        for id in ids {
            if let Some(identity) = now.get(&id) {
                by_identity.entry(*identity).or_default().push(id);
            }
        }
        let mut counted = false;
        for group in by_identity.into_values().filter(|group| group.len() > 1) {
            counted = true;
            for record in &group[1..] {
                held.join(group[0], *record);
            }
        }
        dismissed_splits += u64::from(counted);
    }
    verdicts
        .retain(|(a, b), verdict| *verdict != Verdict::NoMatch || held.find(*a) != held.find(*b));
    for (root, members) in joined(&held) {
        for record in members.into_iter().filter(|record| *record != root) {
            verdicts.insert(ordered(root, record), Verdict::Match);
        }
    }

    let mut constraints = Vec::new();
    let mut by_constraint: HashMap<(RecordId, RecordId), PersonDecision> = HashMap::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT c.record_a::VARCHAR, c.record_b::VARCHAR, c.kind, c.from_seq,
                    ch.actor, ch.reason
             FROM spool.constraint_link c
             JOIN spool.changeset ch ON ch.seq = c.from_seq
             WHERE c.to_seq IS NULL
               AND c.record_a IN (SELECT record_id FROM {SCOPE})
               AND c.record_b IN (SELECT record_id FROM {SCOPE})
             ORDER BY c.record_a, c.record_b"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let pair = ordered(
                parse(&row.get::<_, String>(0)?)?,
                parse(&row.get::<_, String>(1)?)?,
            );
            let kind = match row.get::<_, String>(2)?.as_str() {
                "must_link" => ConstraintKind::MustLink,
                _ => ConstraintKind::CannotLink,
            };
            let decision = PersonDecision {
                seq: Seq(row.get::<_, i64>(3)? as u64),
                actor: parse(&row.get::<_, String>(4)?)?,
                reason: row.get(5)?,
            };
            verdicts.insert(
                pair,
                match kind {
                    ConstraintKind::MustLink => Verdict::Match,
                    ConstraintKind::CannotLink => Verdict::NoMatch,
                },
            );
            if kind == ConstraintKind::CannotLink {
                by_constraint.insert(pair, decision.clone());
            }
            constraints.push(ConstraintNote {
                kind,
                records: [pair.0, pair.1],
                decision,
            });
        }
    }

    // The default policy, in an empty store where every record is new.
    let mut state = MemoryState::new();
    for record in &records {
        state.add_record(*record, entity_type.clone());
    }
    let evidence = EvidenceSummary {
        records: records
            .iter()
            .map(|record| NewRecord {
                record: *record,
                entity_type: entity_type.clone(),
            })
            .collect(),
        pairs: verdicts
            .iter()
            .map(|((a, b), verdict)| NewPair {
                a: *a,
                b: *b,
                verdict: *verdict,
            })
            .collect(),
        links: Vec::new(),
    };
    let draft = reconcile(
        &evidence,
        &state,
        &Policy::default(),
        &mut SeededIds::new(0),
    )?;
    let mut by_identity: BTreeMap<IdentityId, Vec<RecordId>> = BTreeMap::new();
    for op in &draft.changeset.ops {
        if let Op::Assign {
            record,
            to: Some(to),
            ..
        } = op
        {
            by_identity.entry(*to).or_default().push(*record);
        }
    }
    let mut groups: Vec<Vec<RecordId>> = by_identity
        .into_values()
        .map(|mut group| {
            group.sort();
            group
        })
        .collect();
    let mut waiting: Vec<Vec<RecordId>> = draft
        .review
        .iter()
        .map(|item| {
            let mut records = match item {
                ReviewItem::PossibleMerge { records, .. }
                | ReviewItem::Conflict { records, .. } => records.clone(),
            };
            records.sort();
            records
        })
        .collect();
    // What people put together stays an identity where the rules contradict
    // themselves around it, and only the other records of such a group wait.
    // Left waiting, its records would lose their identity when the result is
    // applied, and with it what says that a person put them together.
    // The records of a group that is kept out are what the rest contradicts
    // itself about, so the item they wait in names its identity.
    let mut kept_out: Vec<Option<RecordId>> = vec![None; waiting.len()];
    for set in joined(&held).into_values() {
        let Some(at) = waiting
            .iter()
            .position(|group| group.binary_search(&set[0]).is_ok())
        else {
            continue;
        };
        if set
            .iter()
            .all(|record| waiting[at].binary_search(record).is_ok())
        {
            let taken: BTreeSet<RecordId> = set.iter().copied().collect();
            waiting[at].retain(|record| !taken.contains(record));
            kept_out[at].get_or_insert(set[0]);
            groups.push(set);
        }
    }
    let mut waiting_with: Vec<(Vec<RecordId>, Option<RecordId>)> = waiting
        .into_iter()
        .zip(kept_out)
        .filter(|(group, _)| !group.is_empty())
        .collect();
    waiting_with.sort();
    let (waiting, kept_out): (Vec<Vec<RecordId>>, Vec<Option<RecordId>>) =
        waiting_with.into_iter().unzip();
    groups.sort();

    let mut place: HashMap<RecordId, Place> = HashMap::new();
    for (at, group) in groups.iter().enumerate() {
        for record in group {
            place.insert(*record, Place::Group(at));
        }
    }
    for (at, group) in waiting.iter().enumerate() {
        for record in group {
            place.insert(*record, Place::Waiting(at));
        }
    }

    // The `no_match` pairs inside each waiting group, found from the
    // verdicts rather than from every pair of its records.
    let mut blocked: Vec<Vec<Contradiction>> = vec![Vec::new(); waiting.len()];
    for ((a, b), verdict) in &verdicts {
        if *verdict != Verdict::NoMatch {
            continue;
        }
        if let (Some(Place::Waiting(at)), Some(Place::Waiting(other))) =
            (place.get(a), place.get(b))
            && at == other
        {
            let pair = (*a, *b);
            blocked[*at].push(Contradiction {
                records: [*a, *b],
                constraint: by_constraint.get(&pair).cloned(),
                rules: match by_constraint.contains_key(&pair) {
                    true => Vec::new(),
                    false => rule_names.get(&pair).cloned().unwrap_or_default(),
                },
            });
        }
    }
    // A constraint is the decision of a person, so those come first.
    for found in &mut blocked {
        found.sort_by_key(|contradiction| contradiction.constraint.is_none());
    }

    // Which identity each group becomes. An identity's home is the group
    // that holds most of its records, on a tie the first group.
    let mut counts: BTreeMap<IdentityId, BTreeMap<usize, u64>> = BTreeMap::new();
    for (record, identity) in &now {
        if let Some(Place::Group(at)) = place.get(record) {
            *counts.entry(*identity).or_default().entry(*at).or_default() += 1;
        }
    }
    let mut claimants: BTreeMap<usize, Vec<(IdentityId, u64)>> = BTreeMap::new();
    for (identity, at) in &counts {
        let (home, most) = at
            .iter()
            .max_by(|(a, x), (b, y)| x.cmp(y).then_with(|| b.cmp(a)))
            .map(|(home, most)| (*home, *most))
            .expect("an identity counted here has a record in a group");
        claimants.entry(home).or_default().push((*identity, most));
    }
    let mut keeper: Vec<Option<IdentityId>> = vec![None; groups.len()];
    let mut merged: BTreeMap<IdentityId, IdentityId> = BTreeMap::new();
    for (home, mut identities) in claimants {
        // The most records there, then the oldest, then the smallest ID.
        identities.sort_by(|(a, x), (b, y)| {
            y.cmp(x)
                .then_with(|| created[a].cmp(&created[b]))
                .then_with(|| a.cmp(b))
        });
        let kept = identities[0].0;
        keeper[home] = Some(kept);
        for (identity, _) in &identities[1..] {
            merged.insert(*identity, kept);
        }
    }

    let labels = labels(conn)?;
    let shaped = shaped_by_people(conn)?;
    let approved = approved_placements(conn)?;
    let open_items: Vec<Vec<RecordId>> = strings(
        conn,
        &format!(
            "SELECT to_json(record_ids)::VARCHAR FROM spool.review_item
             WHERE entity_type = '{entity_type}' AND status = 'open'
               AND kind <> 'possible_split'
             ORDER BY review_id"
        ),
    )?
    .iter()
    .map(|ids| {
        serde_json::from_str::<Vec<String>>(ids)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect()
    })
    .collect();

    Ok(Outcome {
        entity_type: entity_type.clone(),
        records,
        now,
        groups,
        waiting,
        kept_out,
        place,
        keeper,
        merged,
        blocked,
        labels,
        shaped,
        approved,
        constraints,
        held_together,
        dismissed_splits,
        open_items,
        pairs_now,
    })
}

fn ordered(a: RecordId, b: RecordId) -> (RecordId, RecordId) {
    if a <= b { (a, b) } else { (b, a) }
}

fn strings(conn: &Connection, sql: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query([])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(row.get(0)?);
    }
    Ok(found)
}

fn labels(conn: &Connection) -> Result<HashMap<RecordId, (String, i64)>> {
    let mut statement = conn.prepare(&format!(
        "SELECT l.record_id::VARCHAR, l.label, epoch_us(o.loaded_at) FROM spool.record_label l
         JOIN spool.record o ON o.record_id = l.record_id
         WHERE l.label IS NOT NULL AND l.record_id IN (SELECT record_id FROM {SCOPE})"
    ))?;
    let mut rows = statement.query([])?;
    let mut found = HashMap::new();
    while let Some(row) = rows.next()? {
        found.insert(
            parse(&row.get::<_, String>(0)?)?,
            (row.get(1)?, row.get(2)?),
        );
    }
    Ok(found)
}

/// For each identity a record in scope is in, the changesets by people or
/// agents that named it or an identity merged into it, other than to
/// record or remove a constraint.
fn shaped_by_people(conn: &Connection) -> Result<BTreeMap<IdentityId, Vec<PersonDecision>>> {
    let mut statement = conn.prepare(&format!(
        "WITH here AS (
           SELECT DISTINCT m.identity_id FROM {SCOPE} s
           JOIN spool.membership m ON m.record_id = s.record_id AND m.to_seq IS NULL)
         SELECT DISTINCT coalesce(i.merged_into, i.identity_id)::VARCHAR, c.seq, c.actor, c.reason
         FROM spool.changeset_op op
         JOIN spool.changeset c ON c.seq = op.seq AND c.actor <> 'engine'
         JOIN spool.identity i ON i.identity_id IN (op.identity_id, op.from_identity_id)
         WHERE op.kind IN ('create_identity', 'assign', 'merge', 'reinstate')
           AND coalesce(i.merged_into, i.identity_id) IN (SELECT identity_id FROM here)
         ORDER BY 1, 2"
    ))?;
    let mut rows = statement.query([])?;
    let mut found: BTreeMap<IdentityId, Vec<PersonDecision>> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        found
            .entry(parse(&row.get::<_, String>(0)?)?)
            .or_default()
            .push(PersonDecision {
                seq: Seq(row.get::<_, i64>(1)? as u64),
                actor: parse(&row.get::<_, String>(2)?)?,
                reason: row.get(3)?,
            });
    }
    Ok(found)
}

/// The records in scope that drafts placed which reviewers approved, with
/// the last such draft.
fn approved_placements(conn: &Connection) -> Result<BTreeMap<RecordId, (ChangesetId, Seq)>> {
    let mut statement = conn.prepare(&format!(
        "SELECT op.record_id::VARCHAR, d.changeset_id::VARCHAR, op.seq
         FROM spool.changeset_op op
         JOIN spool.draft d ON d.closed_seq = op.seq AND d.status = 'committed'
         WHERE op.kind = 'assign' AND op.identity_id IS NOT NULL
           AND op.record_id IN (SELECT record_id FROM {SCOPE})
         ORDER BY op.seq"
    ))?;
    let mut rows = statement.query([])?;
    let mut found = BTreeMap::new();
    while let Some(row) = rows.next()? {
        found.insert(
            parse(&row.get::<_, String>(0)?)?,
            (
                parse(&row.get::<_, String>(1)?)?,
                Seq(row.get::<_, i64>(2)? as u64),
            ),
        );
    }
    Ok(found)
}

/// Each set of more than one record, by its first record, with its records
/// in order.
fn joined(held: &DisjointSets<RecordId>) -> BTreeMap<RecordId, Vec<RecordId>> {
    let mut sets = held.sets();
    sets.retain(|_, members| members.len() > 1);
    sets
}

/// A node of the graph whose connected parts are the changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Node {
    Identity(IdentityId),
    Unplaced(RecordId),
    Group(usize),
    Waiting(usize),
}

fn choose(n: u64) -> u64 {
    n * n.saturating_sub(1) / 2
}

impl Outcome {
    pub(crate) fn entity_type(&self) -> &EntityType {
        &self.entity_type
    }

    fn target(&self, record: RecordId) -> Option<Place> {
        self.place.get(&record).copied()
    }

    /// The identity a group becomes, named the way the plan names it: an
    /// identity that keeps its ID, or the group's first record.
    fn group_name(&self, at: usize) -> String {
        match self.keeper[at] {
            Some(identity) => identity.to_string(),
            None => format!("new:{}", self.groups[at][0]),
        }
    }

    /// The plan code of this type: a hash of where each record is and
    /// where it would go.
    pub(crate) fn plan(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"spool.recompute.v1\0");
        hash.update(self.entity_type.as_str().as_bytes());
        hash.update([0]);
        for record in &self.records {
            let now = self
                .now
                .get(record)
                .map_or_else(|| "-".to_owned(), ToString::to_string);
            let then = match self.target(*record) {
                Some(Place::Group(at)) => self.group_name(at),
                Some(Place::Waiting(at)) => format!("waiting:{}", self.waiting[at][0]),
                None => "-".to_owned(),
            };
            hash.update(format!("{record} {now} {then}\n").as_bytes());
        }
        hex::encode(hash.finalize())
    }

    /// The operations that make the identities what they would be, with
    /// `mint` for the IDs of new identities: the new identities, then
    /// each record that moves, then each merge. A record of an identity
    /// that is merged into the one it would be in moves with the merge.
    /// The identity of each group is returned with them.
    pub(crate) fn operations(
        &self,
        mut mint: impl FnMut() -> IdentityId,
    ) -> (Vec<Op>, Vec<IdentityId>) {
        let mut ops = Vec::new();
        let mut ids: Vec<Option<IdentityId>> = self.keeper.clone();
        for id in &mut ids {
            if id.is_none() {
                let identity = mint();
                ops.push(Op::CreateIdentity {
                    identity,
                    entity_type: self.entity_type.clone(),
                });
                *id = Some(identity);
            }
        }
        for record in &self.records {
            let from = self.now.get(record).copied();
            let to = match self.target(*record) {
                Some(Place::Group(at)) => ids[at],
                Some(Place::Waiting(_)) => None,
                None => continue,
            };
            if from == to {
                continue;
            }
            if let (Some(from), Some(to)) = (from, to)
                && self.merged.get(&from) == Some(&to)
            {
                continue;
            }
            ops.push(Op::Assign {
                record: *record,
                from,
                to,
            });
        }
        for (from, into) in &self.merged {
            ops.push(Op::Merge {
                from: *from,
                into: *into,
            });
        }
        (ops, ids.into_iter().flatten().collect())
    }

    /// The groups that would wait, as the review items they are left in.
    /// `ids` is the identity of each group, as [`operations`](Self::operations)
    /// returns them: an item names the identity of what people put together
    /// that was kept out of its group.
    pub(crate) fn review(&self, ids: &[IdentityId]) -> Vec<ReviewItem> {
        self.waiting
            .iter()
            .zip(&self.kept_out)
            .map(|(records, kept)| ReviewItem::Conflict {
                records: records.clone(),
                identity: kept.and_then(|record| match self.target(record) {
                    Some(Place::Group(at)) => ids.get(at).copied(),
                    _ => None,
                }),
            })
            .collect()
    }

    /// The label most of the records have; of labels as many have, the one
    /// loaded first.
    fn label_of<'a>(&self, records: impl Iterator<Item = &'a RecordId>) -> Option<String> {
        let mut counts: BTreeMap<&str, (u64, (i64, RecordId))> = BTreeMap::new();
        for record in records {
            if let Some((label, loaded)) = self.labels.get(record) {
                let first = (*loaded, *record);
                let entry = counts.entry(label.as_str()).or_insert((0, first));
                entry.0 += 1;
                entry.1 = entry.1.min(first);
            }
        }
        counts
            .into_iter()
            .max_by(|(_, (x, a)), (_, (y, b))| x.cmp(y).then_with(|| b.cmp(a)))
            .map(|(label, _)| label.to_owned())
    }

    /// Whether a record would be in another identity than now, or in none.
    fn changes_identity(&self, record: RecordId) -> bool {
        match (self.now.get(&record), self.target(record)) {
            (Some(identity), Some(Place::Group(at))) => self.keeper[at] != Some(*identity),
            (None, Some(Place::Group(_))) | (Some(_), Some(Place::Waiting(_))) => true,
            _ => false,
        }
    }

    /// Where the records of a change would go, each place with its records,
    /// the place with most first and then by first record. This is the
    /// order of a change's `after`.
    fn destinations(&self, records: &[RecordId]) -> Vec<(Place, Vec<RecordId>)> {
        let mut by_place: BTreeMap<Place, Vec<RecordId>> = BTreeMap::new();
        for record in records {
            if let Some(place) = self.target(*record) {
                by_place.entry(place).or_default().push(*record);
            }
        }
        let mut found: Vec<(Place, Vec<RecordId>)> = by_place
            .into_iter()
            .map(|(place, mut members)| {
                members.sort();
                (place, members)
            })
            .collect();
        found.sort_by(|(_, a), (_, b)| b.len().cmp(&a.len()).then_with(|| a[0].cmp(&b[0])));
        found
    }

    /// The changes, largest first, each with its records.
    #[expect(clippy::too_many_lines)]
    fn changes(&self) -> Vec<(Change, Vec<RecordId>)> {
        let mut nodes: Vec<Node> = Vec::new();
        let mut index: HashMap<Node, usize> = HashMap::new();
        let mut parent: Vec<usize> = Vec::new();
        fn find(parent: &mut [usize], mut node: usize) -> usize {
            while parent[node] != node {
                parent[node] = parent[parent[node]];
                node = parent[node];
            }
            node
        }
        let mut node = |node: Node, nodes: &mut Vec<Node>, parent: &mut Vec<usize>| -> usize {
            *index.entry(node).or_insert_with(|| {
                nodes.push(node);
                parent.push(parent.len());
                parent.len() - 1
            })
        };
        let mut merged_into: BTreeMap<IdentityId, Vec<IdentityId>> = BTreeMap::new();
        for (from, into) in &self.merged {
            merged_into.entry(*into).or_default().push(*from);
        }
        let mut edges: Vec<(RecordId, usize, usize)> = Vec::new();
        for record in &self.records {
            let Some(target) = self.target(*record) else {
                continue;
            };
            let source = match self.now.get(record) {
                Some(identity) => Node::Identity(*identity),
                None => Node::Unplaced(*record),
            };
            let target = match target {
                Place::Group(at) => Node::Group(at),
                Place::Waiting(at) => Node::Waiting(at),
            };
            let (a, b) = (
                node(source, &mut nodes, &mut parent),
                node(target, &mut nodes, &mut parent),
            );
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            parent[ra.max(rb)] = ra.min(rb);
            edges.push((*record, a, b));
        }
        let mut parts: BTreeMap<usize, (BTreeSet<Node>, Vec<RecordId>)> = BTreeMap::new();
        for (record, a, b) in edges {
            let root = find(&mut parent, a);
            let part = parts.entry(root).or_default();
            part.0.insert(nodes[a]);
            part.0.insert(nodes[b]);
            part.1.push(record);
        }

        let mut changes = Vec::new();
        for (nodes, records) in parts.into_values() {
            let olds: Vec<IdentityId> = nodes
                .iter()
                .filter_map(|node| match node {
                    Node::Identity(identity) => Some(*identity),
                    _ => None,
                })
                .collect();
            let groups: Vec<usize> = nodes
                .iter()
                .filter_map(|node| match node {
                    Node::Group(at) => Some(*at),
                    _ => None,
                })
                .collect();
            let waits: Vec<usize> = nodes
                .iter()
                .filter_map(|node| match node {
                    Node::Waiting(at) => Some(*at),
                    _ => None,
                })
                .collect();
            let unplaced = records
                .iter()
                .filter(|record| !self.now.contains_key(record))
                .count() as u64;
            // Nothing changes: one identity keeps exactly its records, or
            // records that wait go on waiting.
            if olds.is_empty() && groups.is_empty() {
                continue;
            }
            if olds.len() == 1 && groups.len() == 1 && waits.is_empty() && unplaced == 0 {
                continue;
            }
            let kind = match (olds.len(), groups.len(), waits.len()) {
                (0, _, _) => ChangeKind::New,
                (1, 0, _) => ChangeKind::Review,
                (1, 1, 0) => ChangeKind::Gain,
                (1, 1, _) => ChangeKind::Lose,
                (1, _, _) => ChangeKind::Split,
                (_, 0, _) => ChangeKind::Review,
                (_, 1, 0) => ChangeKind::Merge,
                _ => ChangeKind::Regroup,
            };

            let mut by_old: BTreeMap<Option<IdentityId>, Vec<RecordId>> = BTreeMap::new();
            for record in &records {
                by_old
                    .entry(self.now.get(record).copied())
                    .or_default()
                    .push(*record);
            }
            let mut before: Vec<Before> = by_old
                .iter()
                .map(|(identity, members)| Before {
                    identity: *identity,
                    label: self.label_of(members.iter()),
                    records: members.len() as u64,
                })
                .collect();
            before.sort_by(|a, b| {
                b.records
                    .cmp(&a.records)
                    .then_with(|| a.identity.is_none().cmp(&b.identity.is_none()))
                    .then_with(|| a.identity.cmp(&b.identity))
            });
            let after: Vec<After> = self
                .destinations(&records)
                .iter()
                .map(|(place, members)| {
                    let (fate, identity, merged) = match place {
                        Place::Group(at) => match self.keeper[*at] {
                            Some(kept) => (
                                Fate::Kept,
                                Some(kept),
                                merged_into.get(&kept).cloned().unwrap_or_default(),
                            ),
                            None => (Fate::New, None, Vec::new()),
                        },
                        Place::Waiting(_) => (Fate::Waiting, None, Vec::new()),
                    };
                    After {
                        fate,
                        identity,
                        label: self.label_of(members.iter()),
                        records: members.len() as u64,
                        merged,
                    }
                })
                .collect();
            let moved = records
                .iter()
                .filter(|record| self.changes_identity(**record))
                .count() as u64;
            let mut shaped_by: Vec<PersonDecision> = olds
                .iter()
                .flat_map(|identity| self.shaped.get(identity).into_iter().flatten())
                .cloned()
                .collect();
            shaped_by.sort_by_key(|decision| decision.seq);
            shaped_by.dedup();
            let approved_drafts = records
                .iter()
                .filter(|record| self.approved.contains_key(record))
                .count() as u64;
            let blocked_by: Vec<Contradiction> = waits
                .iter()
                .filter_map(|at| self.blocked[*at].first().cloned())
                .collect();
            changes.push((
                Change {
                    rank: 0,
                    kind,
                    records: records.len() as u64,
                    moved,
                    before,
                    after,
                    shaped_by,
                    approved_drafts,
                    blocked_by,
                },
                records,
            ));
        }
        changes.sort_by(|(a, x), (b, y)| {
            b.records
                .cmp(&a.records)
                .then_with(|| x.iter().min().cmp(&y.iter().min()))
        });
        for (rank, (change, _)) in changes.iter_mut().enumerate() {
            change.rank = rank as u64 + 1;
        }
        changes
    }

    pub(crate) fn summary(&self) -> RecomputeSummary {
        let mut summary = RecomputeSummary {
            records: self.records.len() as u64,
            pairs_now: self.pairs_now,
            ..RecomputeSummary::default()
        };
        let mut members: BTreeMap<IdentityId, Vec<RecordId>> = BTreeMap::new();
        for (record, identity) in &self.now {
            members.entry(*identity).or_default().push(*record);
        }
        summary.identities_now = members.len() as u64;
        summary.identities_after = self.groups.len() as u64;
        summary.new_identities = self.keeper.iter().filter(|kept| kept.is_none()).count() as u64;
        summary.merged_away = self.merged.len() as u64;
        let mut cells: HashMap<(IdentityId, usize), u64> = HashMap::new();
        for (identity, records) in &members {
            let places: BTreeSet<Place> = records
                .iter()
                .filter_map(|record| self.target(*record))
                .collect();
            let groups = places
                .iter()
                .filter(|place| matches!(place, Place::Group(_)))
                .count();
            if groups > 1 {
                summary.splitting += 1;
            }
            if groups == 0 {
                summary.emptied += 1;
            }
            for record in records {
                if let Some(Place::Group(at)) = self.target(*record) {
                    *cells.entry((*identity, at)).or_default() += 1;
                }
            }
        }
        for (at, group) in self.groups.iter().enumerate() {
            let from: BTreeSet<Option<IdentityId>> = group
                .iter()
                .map(|record| self.now.get(record).copied())
                .collect();
            if from.iter().flatten().count() > 1 {
                summary.merging += 1;
            }
            if let Some(kept) = self.keeper[at] {
                let had = &members[&kept];
                let stays = group
                    .iter()
                    .filter(|record| self.now.get(record) == Some(&kept))
                    .count();
                if stays == had.len() && group.len() == had.len() {
                    summary.unchanged += 1;
                }
                if group.len() > stays {
                    summary.gaining += 1;
                }
                if had.len() > stays {
                    summary.losing += 1;
                }
            }
            summary.pairs_after += choose(group.len() as u64);
        }
        summary.pairs_kept = cells.values().map(|n| choose(*n)).sum();
        for record in &self.records {
            let from = self.now.get(record).copied();
            match (from, self.target(*record)) {
                (None, Some(Place::Group(_))) => summary.records_placed += 1,
                (Some(_), Some(Place::Waiting(_))) => summary.records_unplaced += 1,
                (Some(identity), Some(Place::Group(at))) if self.keeper[at] != Some(identity) => {
                    summary.records_moved += 1;
                }
                _ => {}
            }
        }
        summary.records_waiting = self.waiting.iter().map(|group| group.len() as u64).sum();
        summary.review_items = self.waiting.len() as u64;
        summary.review_items_closed = self
            .open_items
            .iter()
            .filter(|records| {
                records
                    .iter()
                    .all(|record| matches!(self.target(*record), Some(Place::Group(_))))
            })
            .count() as u64;
        summary
    }

    pub(crate) fn report(
        &self,
        conn: &Connection,
        request: &RecomputeRequest,
        detail_of: Option<IdentityId>,
    ) -> Result<TypeRecompute> {
        let changes = self.changes();
        let mut decisions = DecisionSummary {
            held_together: self.held_together,
            dismissed_splits: self.dismissed_splits,
            ..DecisionSummary::default()
        };
        for constraint in &self.constraints {
            match constraint.kind {
                ConstraintKind::MustLink => decisions.must_link += 1,
                ConstraintKind::CannotLink => decisions.cannot_link += 1,
            }
        }
        for blocked in &self.blocked {
            match blocked.first() {
                Some(contradiction) if contradiction.constraint.is_some() => {
                    decisions.blocked_by_constraints += 1;
                }
                Some(_) => decisions.blocked_by_rules += 1,
                None => {}
            }
        }
        decisions.changes_shaped_by_people = changes
            .iter()
            .filter(|(change, _)| !change.shaped_by.is_empty())
            .count() as u64;
        decisions.changes_with_approved_drafts = changes
            .iter()
            .filter(|(change, _)| change.approved_drafts > 0)
            .count() as u64;

        let detail = match detail_of {
            Some(identity) => changes
                .iter()
                .find(|(change, _)| {
                    change
                        .before
                        .iter()
                        .any(|before| before.identity == Some(identity))
                        || change.after.iter().any(|after| {
                            after.identity == Some(identity) || after.merged.contains(&identity)
                        })
                })
                .map(|(change, records)| self.detail(conn, change, records))
                .transpose()?,
            None => None,
        };
        Ok(TypeRecompute {
            entity_type: self.entity_type.clone(),
            summary: self.summary(),
            decisions,
            changes_total: changes.len() as u64,
            changes: changes
                .into_iter()
                .skip(request.offset as usize)
                .take(request.limit as usize)
                .map(|(change, _)| change)
                .collect(),
            detail,
        })
    }

    fn detail(
        &self,
        conn: &Connection,
        change: &Change,
        records: &[RecordId],
    ) -> Result<ChangeDetail> {
        let places: Vec<Place> = self
            .destinations(records)
            .into_iter()
            .map(|(place, _)| place)
            .collect();
        let mut found = Vec::new();
        let mut sources: HashMap<RecordId, (String, Option<String>)> = HashMap::new();
        {
            let list: Vec<String> = records.iter().map(|id| format!("'{id}'")).collect();
            let mut statement = conn.prepare(&format!(
                "SELECT record_id::VARCHAR, source_id, source_key FROM spool.record
                 WHERE record_id IN (SELECT unnest([{}]::UUID[]))",
                list.join(", ")
            ))?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                sources.insert(
                    parse(&row.get::<_, String>(0)?)?,
                    (row.get(1)?, row.get(2)?),
                );
            }
        }
        for record in records {
            let to = self
                .target(*record)
                .and_then(|place| places.iter().position(|known| *known == place))
                .unwrap_or(0) as u64;
            let (source, key) = sources.get(record).cloned().unwrap_or_default();
            found.push(RecordMove {
                record: *record,
                source,
                key,
                label: self.labels.get(record).map(|(label, _)| label.clone()),
                from: self.now.get(record).copied(),
                to,
            });
        }
        found.sort_by(|a, b| {
            a.to.cmp(&b.to)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.key.cmp(&b.key))
                .then_with(|| a.record.cmp(&b.record))
        });
        let members: BTreeSet<RecordId> = records.iter().copied().collect();
        let constraints = self
            .constraints
            .iter()
            .filter(|note| members.contains(&note.records[0]) && members.contains(&note.records[1]))
            .cloned()
            .collect();
        let approved_drafts = records
            .iter()
            .filter_map(|record| {
                self.approved
                    .get(record)
                    .map(|(draft, seq)| ApprovedPlacement {
                        record: *record,
                        draft: *draft,
                        seq: *seq,
                    })
            })
            .collect();
        let mut change = change.clone();
        change.blocked_by = change_blocked(self, records);
        Ok(ChangeDetail {
            change,
            records: found,
            constraints,
            approved_drafts,
        })
    }
}

/// Every contradiction in the waiting groups of a change, for its detail.
fn change_blocked(outcome: &Outcome, records: &[RecordId]) -> Vec<Contradiction> {
    let waits: BTreeSet<usize> = records
        .iter()
        .filter_map(|record| match outcome.target(*record) {
            Some(Place::Waiting(at)) => Some(at),
            _ => None,
        })
        .collect();
    waits
        .into_iter()
        .flat_map(|at| outcome.blocked[at].iter().cloned())
        .collect()
}

/// The plan code of several types.
pub(crate) fn combined_plan(plans: &[String]) -> String {
    if let [only] = plans {
        return only.clone();
    }
    let mut hash = Sha256::new();
    hash.update(b"spool.recompute.v1.types\0");
    for plan in plans {
        hash.update(plan.as_bytes());
        hash.update([0]);
    }
    hex::encode(hash.finalize())
}

/// Mints the IDs of the new identities an outcome needs.
pub(crate) fn mint<'a>(ids: &'a mut dyn IdGenerator) -> impl FnMut() -> IdentityId + 'a {
    move || ids.identity_id()
}
