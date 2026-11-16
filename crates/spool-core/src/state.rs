//! Identity state: what a store must answer, and what it must be able to write.

use serde::{Deserialize, Serialize};

use crate::changeset::{Changeset, ContentHash};
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
    MergedInto(IdentityId),
}

/// An identity has an ID, an entity type and a status. It has no attributes of
/// its own: what is known about it is the evidence of its member observations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityRecord {
    pub id: IdentityId,
    pub entity_type: EntityType,
    pub created: Seq,
    pub status: IdentityStatus,
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
