//! Identity state for Spool.
//!
//! This crate holds what an identity means and which changes to it are valid.
//! It knows nothing about how evidence is stored or matched, and depends on no
//! database.
//!
//! - [`Changeset`] and [`Op`]: the only way identity state changes.
//! - [`validate`]: the rules every changeset must pass. It returns a
//!   [`Validated`], which is the only thing an [`IdentityStore`] applies.
//! - [`reconcile`]: turns a strategy's judgements about new observations into
//!   a draft changeset.
//! - [`inverse`]: the operations that undo a committed changeset.
//! - [`MemoryState`]: an in-memory store, used as the reference model and as
//!   the target of log replay.
//! - [`conformance`]: the suite every store must pass.

mod changeset;
pub mod conformance;
mod generate;
mod ids;
mod memory;
mod reconcile;
mod state;
mod undo;
mod validate;

pub use changeset::{Changeset, ConstraintKind, ContentHash, Op};
pub use generate::{Clock, IdGenerator, RandomIds, SeededIds, StepClock, SystemClock};
pub use ids::{
    Actor, ChangesetId, DocumentId, EntityType, IdentityId, InvalidActor, InvalidEntityType,
    ObservationId, RunId, Seq,
};
pub use memory::{MemoryState, ReplayError};
pub use reconcile::{
    Draft, EvidenceSummary, IdentityLink, NewObservation, NewPair, Policy, ReviewItem, Verdict,
    reconcile,
};
pub use state::{
    Committed, Constraint, ConstraintRecord, IdentityRecord, IdentityState, IdentityStatus,
    IdentityStore, LogEntry, Membership, Merged, Snapshot, StateError,
};
pub use undo::inverse;
pub use validate::{Reason, Rejection, Validated, ValidationError, validate};
