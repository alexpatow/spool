//! Identity state: what a store must answer, and what it must be able to write.

use serde::{Deserialize, Serialize};

use crate::changeset::{Changeset, ConstraintKind, ContentHash};
use crate::ids::{ChangesetId, EntityType, IdentityId, ObservationId, Seq};
use crate::validate::Validated;

/// A failure of the store behind an [`IdentityState`], as opposed to a
/// changeset the rules reject.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct StateError(Box<dyn std::error::Error + Send + Sync>);

impl StateError {
    pub fn new(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self(source.into())
    }
}

pub type Result<T, E = StateError> = std::result::Result<T, E>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum IdentityStatus {
    Active,
    /// Merged, and the active identity that stands for it now. That is the
    /// identity it was merged into, unless that one was merged in turn.
    MergedInto(IdentityId),
}

/// The merge that made an identity inactive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Merged {
    /// The identity the merge named. Undoing the merge needs this one, and
    /// not the identity at the end of the chain.
    pub into: IdentityId,
    pub seq: Seq,
}

/// An identity has an ID, an entity type and a status. It has no attributes of
/// its own: what is known about it is the evidence of its member observations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityRecord {
    pub id: IdentityId,
    pub entity_type: EntityType,
    pub created: Seq,
    pub status: IdentityStatus,
    /// `None` while the identity is active.
    pub merged: Option<Merged>,
}

/// One interval of an observation's membership. `to` is `None` while the
/// observation is still a member.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Membership {
    pub observation: ObservationId,
    pub identity: IdentityId,
    pub from: Seq,
    pub to: Option<Seq>,
}

/// A decision that two observations must share an identity, or must not.
/// The pair has no direction: `a` is the smaller ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Constraint {
    pub a: ObservationId,
    pub b: ObservationId,
    pub kind: ConstraintKind,
}

impl Constraint {
    pub fn new(kind: ConstraintKind, a: ObservationId, b: ObservationId) -> Self {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        Self { a, b, kind }
    }

    /// Whether the constraint holds for the identities its two observations
    /// belong to. An observation without an identity breaks nothing: the
    /// constraint decides where it may go later.
    pub fn holds(&self, a: Option<IdentityId>, b: Option<IdentityId>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => match self.kind {
                ConstraintKind::MustLink => a == b,
                ConstraintKind::CannotLink => a != b,
            },
            _ => true,
        }
    }
}

/// One interval of a constraint. `to` is `None` while it is in force.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ConstraintRecord {
    #[serde(flatten)]
    pub constraint: Constraint,
    pub from: Seq,
    pub to: Option<Seq>,
}

/// The receipt for a committed changeset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Committed {
    pub seq: Seq,
    pub changeset: ChangesetId,
    pub content_hash: ContentHash,
}

/// A committed changeset as the log holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LogEntry {
    pub seq: Seq,
    pub changeset: Changeset,
    pub content_hash: ContentHash,
}

/// The whole of a store's identity state, in a fixed order, so two stores can
/// be compared for equality.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub head: Seq,
    pub identities: Vec<IdentityRecord>,
    pub memberships: Vec<Membership>,
    pub constraints: Vec<ConstraintRecord>,
}

/// Read access the validator and the reconciler need. Every method takes a
/// batch and answers in the order asked, so a SQL store can answer each call
/// with one query.
pub trait IdentityState {
    /// The position of the last committed changeset.
    fn head(&self) -> Result<Seq>;

    /// The log position of a changeset, if it has been committed.
    fn changeset_seq(&self, id: ChangesetId) -> Result<Option<Seq>>;

    /// The entity type of each observation, or `None` for one the store does
    /// not hold.
    fn observation_types(&self, observations: &[ObservationId]) -> Result<Vec<Option<EntityType>>>;

    fn identities(&self, ids: &[IdentityId]) -> Result<Vec<Option<IdentityRecord>>>;

    /// The identity each observation currently belongs to, if any.
    fn assignments(&self, observations: &[ObservationId]) -> Result<Vec<Option<IdentityId>>>;

    /// The current members of the given identities, sorted.
    fn members(&self, ids: &[IdentityId]) -> Result<Vec<(IdentityId, ObservationId)>>;

    /// For each identity, the log position that last changed which
    /// observations belong to it, or position zero if none has.
    fn membership_changed(&self, ids: &[IdentityId]) -> Result<Vec<Seq>>;

    /// The constraints in force that name any of the observations, sorted.
    fn constraints_on(&self, observations: &[ObservationId]) -> Result<Vec<Constraint>>;

    /// The constraints in force that name a current member of any of the
    /// identities, sorted.
    fn constraints_of_members(&self, ids: &[IdentityId]) -> Result<Vec<Constraint>>;

    /// The observations that the changeset at `seq` took out of `identity`,
    /// sorted.
    fn left_at(&self, identity: IdentityId, seq: Seq) -> Result<Vec<ObservationId>>;

    /// The identity that `identity` was last merged into before `seq`, if it
    /// had been merged by then.
    fn merged_before(&self, identity: IdentityId, seq: Seq) -> Result<Option<IdentityId>>;
}

/// The write side of a store. `apply` accepts only a [`Validated`] changeset,
/// which only [`validate`](crate::validate) produces.
///
/// `log` and `snapshot` read everything and exist for verification: replaying
/// `log` must reproduce `snapshot`.
pub trait IdentityStore: IdentityState {
    /// Appends the changeset to the log and updates current state, atomically.
    fn apply(&mut self, changeset: Validated) -> Result<Committed>;

    /// Every committed changeset, in log order.
    fn log(&self) -> Result<Vec<LogEntry>>;

    fn snapshot(&self) -> Result<Snapshot>;
}
