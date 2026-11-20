//! The suite every [`IdentityStore`] must pass.
//!
//! Each case takes a function that builds an empty store holding the given
//! records. [`conformance_tests!`](crate::conformance_tests) turns the
//! cases into `#[test]` functions:
//!
//! ```
//! # use spool_core::{conformance_tests, EntityType, MemoryState, RecordId};
//! fn store(records: &[(RecordId, EntityType)]) -> MemoryState {
//!     let mut state = MemoryState::new();
//!     for (id, entity_type) in records {
//!         state.add_record(*id, entity_type.clone());
//!     }
//!     state
//! }
//!
//! conformance_tests!(store);
//! ```

use crate::changeset::{Changeset, ConstraintKind, Op};
use crate::generate::{IdGenerator, SeededIds};
use crate::ids::{Actor, EntityType, IdentityId, RecordId, Seq};
use crate::memory::MemoryState;
use crate::state::{IdentityStatus, IdentityStore, Merged};
use crate::validate::{Reason, Rejection, validate};

mod assignments;
mod constraints;
mod log;
mod merges;
mod undo;

pub use assignments::{
    creates_an_identity_and_attaches_records, moves_and_detaches_keeping_history,
    rejects_a_reused_identity_id, rejects_a_stale_precondition_and_names_the_operation,
    rejects_an_entity_type_mismatch, rejects_an_unknown_identity, rejects_an_unknown_record,
    rejects_assigning_an_record_twice_in_one_changeset,
};
pub use constraints::{
    records_and_removes_constraints, rejects_a_state_that_breaks_a_constraint,
    rejects_constraints_that_cannot_be_recorded, splits_with_a_constraint_in_one_changeset,
};
pub use log::{
    a_rejected_changeset_changes_nothing, accepts_a_stale_base_when_preconditions_hold,
    rejects_a_base_ahead_of_the_log, rejects_a_changeset_committed_twice,
    rejects_an_empty_changeset, replaying_the_log_reproduces_the_state, starts_empty,
};
pub use merges::{
    a_chain_of_merges_points_at_the_active_identity, applies_mixed_operations_as_written,
    merges_identities_and_keeps_the_merged_id, reinstates_a_merged_identity,
    rejects_a_merge_whose_members_changed, rejects_merges_that_cannot_be_made,
    rejects_moving_an_record_twice_through_a_merge,
};
pub use undo::{undoes_a_merge_and_the_undo, undoes_assignments_and_constraints};

/// Generates one `#[test]` per conformance case for the store that `$make`
/// builds. `$make` is called as `$make(&[(RecordId, EntityType)])`.
#[macro_export]
macro_rules! conformance_tests {
    ($make:expr) => {
        $crate::conformance_tests!(@cases $make;
            starts_empty,
            creates_an_identity_and_attaches_records,
            moves_and_detaches_keeping_history,
            rejects_a_stale_precondition_and_names_the_operation,
            rejects_a_reused_identity_id,
            rejects_an_unknown_record,
            rejects_an_unknown_identity,
            rejects_an_entity_type_mismatch,
            rejects_assigning_an_record_twice_in_one_changeset,
            rejects_an_empty_changeset,
            rejects_a_base_ahead_of_the_log,
            rejects_a_changeset_committed_twice,
            merges_identities_and_keeps_the_merged_id,
            a_chain_of_merges_points_at_the_active_identity,
            rejects_merges_that_cannot_be_made,
            rejects_a_merge_whose_members_changed,
            rejects_moving_an_record_twice_through_a_merge,
            reinstates_a_merged_identity,
            records_and_removes_constraints,
            rejects_constraints_that_cannot_be_recorded,
            rejects_a_state_that_breaks_a_constraint,
            splits_with_a_constraint_in_one_changeset,
            applies_mixed_operations_as_written,
            undoes_assignments_and_constraints,
            undoes_a_merge_and_the_undo,
            accepts_a_stale_base_when_preconditions_hold,
            a_rejected_changeset_changes_nothing,
            replaying_the_log_reproduces_the_state,
        );
    };
    (@cases $make:expr; $($case:ident),* $(,)?) => {
        $(
            #[test]
            fn $case() {
                $crate::conformance::$case($make);
            }
        )*
    };
}

fn widget() -> EntityType {
    EntityType::new("widget").unwrap()
}

fn gadget() -> EntityType {
    EntityType::new("gadget").unwrap()
}

fn obs(n: u128) -> RecordId {
    RecordId::from_u128(n)
}

/// Five widgets (1 to 5) and one gadget (6).
fn fixture_records() -> Vec<(RecordId, EntityType)> {
    let mut all: Vec<_> = (1..=5).map(|n| (obs(n), widget())).collect();
    all.push((obs(6), gadget()));
    all
}

struct Author {
    ids: SeededIds,
}

impl Author {
    fn new() -> Self {
        Self {
            ids: SeededIds::new(42),
        }
    }

    fn identity(&mut self) -> IdentityId {
        self.ids.identity_id()
    }

    fn changeset(&mut self, base: Seq, ops: Vec<Op>) -> Changeset {
        Changeset {
            id: self.ids.changeset_id(),
            base,
            actor: Actor::User("conformance".to_owned()),
            reason: Some("conformance suite".to_owned()),
            run: None,
            ops,
        }
    }
}

fn create(identity: IdentityId, entity_type: EntityType) -> Op {
    Op::CreateIdentity {
        identity,
        entity_type,
    }
}

fn assign(record: u128, from: Option<IdentityId>, to: Option<IdentityId>) -> Op {
    Op::Assign {
        record: obs(record),
        from,
        to,
    }
}

fn commit<S: IdentityStore>(store: &mut S, author: &mut Author, ops: Vec<Op>) -> Seq {
    let changeset = author.changeset(store.head().unwrap(), ops);
    let validated = validate(store, changeset).expect("the changeset should be valid");
    store.apply(validated).expect("apply should succeed").seq
}

fn expect_rejections<S: IdentityStore>(store: &S, changeset: Changeset) -> Vec<Rejection> {
    match validate(store, changeset) {
        Ok(_) => panic!("the changeset should have been rejected"),
        Err(error) => error
            .rejections()
            .expect("the store should not fail")
            .to_vec(),
    }
}

fn only_reason<S: IdentityStore>(store: &S, changeset: Changeset) -> (Option<usize>, Reason) {
    let mut found = expect_rejections(store, changeset);
    assert_eq!(found.len(), 1, "expected one rejection, found {found:?}");
    let rejection = found.remove(0);
    (rejection.op_index, rejection.reason)
}

fn merge(from: IdentityId, into: IdentityId) -> Op {
    Op::Merge { from, into }
}

fn reinstate(identity: IdentityId) -> Op {
    Op::Reinstate { identity }
}

fn cannot_link(a: u128, b: u128) -> Op {
    Op::Constrain {
        kind: ConstraintKind::CannotLink,
        a: obs(a),
        b: obs(b),
    }
}

fn must_link(a: u128, b: u128) -> Op {
    Op::Constrain {
        kind: ConstraintKind::MustLink,
        a: obs(a),
        b: obs(b),
    }
}

fn unconstrain(kind: ConstraintKind, a: u128, b: u128) -> Op {
    Op::Unconstrain {
        kind,
        a: obs(a),
        b: obs(b),
    }
}

fn read_identity_status<S: IdentityStore>(
    store: &S,
    identity: IdentityId,
) -> (IdentityStatus, Option<Merged>) {
    let record = store.identities(&[identity]).unwrap().remove(0).unwrap();
    (record.status, record.merged)
}

/// Three widget identities: `a` holds 1 and 2, `b` holds 3, `c` holds 4.
/// Record 5 has no identity. Committed at position 1.
fn three_widgets<S: IdentityStore>(
    store: &mut S,
    author: &mut Author,
) -> (IdentityId, IdentityId, IdentityId) {
    let (a, b, c) = (author.identity(), author.identity(), author.identity());
    commit(
        store,
        author,
        vec![
            create(a, widget()),
            create(b, widget()),
            create(c, widget()),
            assign(1, None, Some(a)),
            assign(2, None, Some(a)),
            assign(3, None, Some(b)),
            assign(4, None, Some(c)),
        ],
    );
    (a, b, c)
}

/// Checks that the log alone rebuilds the state, as every store promises.
fn assert_replays<S: IdentityStore>(store: &S) {
    let replayed = MemoryState::replay(&store.log().unwrap()).unwrap();
    assert_eq!(replayed.snapshot().unwrap(), store.snapshot().unwrap());
}
