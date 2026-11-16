//! Identity state for Spool.
//!
//! This crate holds what an identity means and which changes to it are valid.
//! It knows nothing about how evidence is stored or matched, and depends on no
//! database.

mod changeset;
mod generate;
mod ids;

pub use changeset::{Changeset, ConstraintKind, ContentHash, Op};
pub use generate::{Clock, IdGenerator, RandomIds, SeededIds, StepClock, SystemClock};
pub use ids::{
    Actor, ChangesetId, EntityType, IdentityId, InvalidActor, InvalidEntityType, ObservationId,
    RunId, Seq,
};
