//! What a recomputation is asked for, and what it reports: the counts of
//! what would change, the changes one by one, and what applying it did.

use serde::{Deserialize, Serialize};
use spool_core::{
    Actor, ChangesetId, Committed, ConstraintKind, EntityType, IdentityId, RecordId, ReviewItem,
    RunId, Seq,
};

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
