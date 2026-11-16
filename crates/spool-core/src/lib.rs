//! Identity state for Spool.
//!
//! This crate holds what an identity means and which changes to it are valid.
//! It knows nothing about how evidence is stored or matched, and depends on no
//! database.
//!
//! - [`Changeset`] and [`Op`]: the only way identity state changes.
//! - [`validate`]: the rules every changeset must pass. It returns a
//!   [`Validated`], which is the only thing an [`IdentityStore`] applies.
//! - [`MemoryState`]: an in-memory store, used as the reference model and as
//!   the target of log replay.
//! - [`conformance`]: the suite every store must pass.

mod changeset;
pub mod conformance;
mod generate;
mod ids;
mod memory;
mod state;
mod validate;

pub use changeset::{Changeset, ConstraintKind, ContentHash, Op};
pub use generate::{Clock, IdGenerator, RandomIds, SeededIds, StepClock, SystemClock};
pub use ids::{
    Actor, ChangesetId, EntityType, IdentityId, InvalidActor, InvalidEntityType, ObservationId,
    RunId, Seq,
};
pub use memory::{MemoryState, ReplayError};
pub use state::{
    Committed, IdentityRecord, IdentityState, IdentityStatus, IdentityStore, LogEntry, Membership,
    Snapshot, StateError,
};
pub use validate::{Reason, Rejection, Validated, ValidationError, validate};
