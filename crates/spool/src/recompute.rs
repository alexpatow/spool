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

use std::collections::{BTreeMap, HashMap};

use spool_core::{ChangesetId, EntityType, IdentityId, RecordId, Seq};

mod compute;
mod outcome;
mod types;

pub(crate) use compute::{compute, create_scope, judge_for_preview};
pub(crate) use outcome::{combined_plan, mint};
pub use types::{
    After, AppliedType, ApprovedPlacement, Before, Change, ChangeDetail, ChangeKind,
    ConstraintNote, Contradiction, DEFAULT_CHANGES, DecisionSummary, Fate, PersonDecision,
    RecomputeApplied, RecomputeReport, RecomputeRequest, RecomputeSummary, RecordMove,
    TypeRecompute,
};

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
