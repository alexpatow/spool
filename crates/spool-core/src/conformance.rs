//! The suite every [`IdentityStore`] must pass.
//!
//! Each case takes a function that builds an empty store holding the given
//! observations. [`conformance_tests!`](crate::conformance_tests) turns the
//! cases into `#[test]` functions:
//!
//! ```
//! # use spool_core::{conformance_tests, EntityType, MemoryState, ObservationId};
//! fn store(observations: &[(ObservationId, EntityType)]) -> MemoryState {
//!     let mut state = MemoryState::new();
//!     for (id, entity_type) in observations {
//!         state.observe(*id, entity_type.clone());
//!     }
//!     state
//! }
//!
//! conformance_tests!(store);
//! ```

use crate::changeset::{Changeset, ConstraintKind, Op};
use crate::generate::{IdGenerator, SeededIds};
use crate::ids::{Actor, EntityType, IdentityId, ObservationId, Seq};
use crate::memory::MemoryState;
use crate::state::{
    Constraint, ConstraintRecord, IdentityRecord, IdentityStatus, IdentityStore, Membership, Merged,
};
use crate::undo::inverse;
use crate::validate::{Reason, Rejection, validate};

/// Generates one `#[test]` per conformance case for the store that `$make`
/// builds. `$make` is called as `$make(&[(ObservationId, EntityType)])`.
#[macro_export]
macro_rules! conformance_tests {
    ($make:expr) => {
        $crate::conformance_tests!(@cases $make;
            starts_empty,
            creates_an_identity_and_attaches_observations,
            moves_and_detaches_keeping_history,
            rejects_a_stale_precondition_and_names_the_operation,
            rejects_a_reused_identity_id,
            rejects_an_unknown_observation,
            rejects_an_unknown_identity,
            rejects_an_entity_type_mismatch,
            rejects_assigning_an_observation_twice_in_one_changeset,
            rejects_an_empty_changeset,
            rejects_a_base_ahead_of_the_log,
            rejects_a_changeset_committed_twice,
            merges_identities_and_keeps_the_merged_id,
            a_chain_of_merges_points_at_the_active_identity,
            rejects_merges_that_cannot_be_made,
            rejects_a_merge_whose_members_changed,
            rejects_moving_an_observation_twice_through_a_merge,
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

fn obs(n: u128) -> ObservationId {
    ObservationId::from_u128(n)
}

/// Five widgets (1 to 5) and one gadget (6).
fn observations() -> Vec<(ObservationId, EntityType)> {
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

fn assign(observation: u128, from: Option<IdentityId>, to: Option<IdentityId>) -> Op {
    Op::Assign {
        observation: obs(observation),
        from,
        to,
    }
}

fn commit<S: IdentityStore>(store: &mut S, author: &mut Author, ops: Vec<Op>) -> Seq {
    let changeset = author.changeset(store.head().unwrap(), ops);
    let validated = validate(store, changeset).expect("the changeset should be valid");
    store.apply(validated).expect("apply should succeed").seq
}

fn rejections<S: IdentityStore>(store: &S, changeset: Changeset) -> Vec<Rejection> {
    match validate(store, changeset) {
        Ok(_) => panic!("the changeset should have been rejected"),
        Err(error) => error
            .rejections()
            .expect("the store should not fail")
            .to_vec(),
    }
}

fn only_reason<S: IdentityStore>(store: &S, changeset: Changeset) -> (Option<usize>, Reason) {
    let mut found = rejections(store, changeset);
    assert_eq!(found.len(), 1, "expected one rejection, found {found:?}");
    let rejection = found.remove(0);
    (rejection.op_index, rejection.reason)
}

pub fn starts_empty<S: IdentityStore>(make: impl FnOnce(&[(ObservationId, EntityType)]) -> S) {
    let store = make(&observations());
    assert_eq!(store.head().unwrap(), Seq::ZERO);
    assert!(store.log().unwrap().is_empty());
    let snapshot = store.snapshot().unwrap();
    assert!(snapshot.identities.is_empty() && snapshot.memberships.is_empty());
    assert_eq!(store.assignments(&[obs(1)]).unwrap(), vec![None]);
    assert_eq!(
        store.observation_types(&[obs(6), obs(99), obs(1)]).unwrap(),
        vec![Some(gadget()), None, Some(widget())]
    );
}

pub fn creates_an_identity_and_attaches_observations<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, unused) = (author.identity(), author.identity(), author.identity());

    let changeset = author.changeset(
        Seq::ZERO,
        vec![
            create(a, widget()),
            assign(2, None, Some(a)),
            assign(1, None, Some(a)),
            create(b, gadget()),
            assign(6, None, Some(b)),
        ],
    );
    let expected_hash = changeset.content_hash();
    let changeset_id = changeset.id;
    let committed = store.apply(validate(&store, changeset).unwrap()).unwrap();

    assert_eq!(committed.seq, Seq(1));
    assert_eq!(committed.changeset, changeset_id);
    assert_eq!(committed.content_hash, expected_hash);
    assert_eq!(store.head().unwrap(), Seq(1));
    assert_eq!(store.changeset_seq(changeset_id).unwrap(), Some(Seq(1)));

    assert_eq!(
        store.identities(&[b, unused, a]).unwrap(),
        vec![
            Some(IdentityRecord {
                id: b,
                entity_type: gadget(),
                created: Seq(1),
                status: IdentityStatus::Active,
                merged: None,
            }),
            None,
            Some(IdentityRecord {
                id: a,
                entity_type: widget(),
                created: Seq(1),
                status: IdentityStatus::Active,
                merged: None,
            }),
        ]
    );
    assert_eq!(
        store
            .assignments(&[obs(6), obs(3), obs(1), obs(2)])
            .unwrap(),
        vec![Some(b), None, Some(a), Some(a)]
    );
    assert_eq!(
        store.members(&[a, b]).unwrap(),
        vec![(a, obs(1)), (a, obs(2)), (b, obs(6))]
    );
    assert_eq!(store.members(&[unused]).unwrap(), vec![]);
}

pub fn moves_and_detaches_keeping_history<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b) = (author.identity(), author.identity());

    commit(
        &mut store,
        &mut author,
        vec![
            create(a, widget()),
            assign(1, None, Some(a)),
            assign(2, None, Some(a)),
        ],
    );
    commit(
        &mut store,
        &mut author,
        vec![create(b, widget()), assign(2, Some(a), Some(b))],
    );
    commit(&mut store, &mut author, vec![assign(1, Some(a), None)]);
    commit(&mut store, &mut author, vec![assign(1, None, Some(b))]);

    assert_eq!(
        store.assignments(&[obs(1), obs(2)]).unwrap(),
        vec![Some(b), Some(b)]
    );
    assert_eq!(store.members(&[a]).unwrap(), vec![]);
    let interval = |observation, identity, from, to: Option<u64>| Membership {
        observation: obs(observation),
        identity,
        from: Seq(from),
        to: to.map(Seq),
    };
    assert_eq!(
        store.snapshot().unwrap().memberships,
        vec![
            interval(1, a, 1, Some(3)),
            interval(1, b, 4, None),
            interval(2, a, 1, Some(2)),
            interval(2, b, 2, None),
        ]
    );
    assert_eq!(
        store.identities(&[a]).unwrap()[0]
            .as_ref()
            .map(|record| record.status),
        Some(IdentityStatus::Active),
        "an identity with no members still exists"
    );
}

pub fn rejects_a_stale_precondition_and_names_the_operation<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b) = (author.identity(), author.identity());
    commit(
        &mut store,
        &mut author,
        vec![create(a, widget()), create(b, widget())],
    );

    // Two drafts are written against the same state. The first to commit wins.
    let base = store.head().unwrap();
    let first = author.changeset(base, vec![assign(1, None, Some(a))]);
    let second = author.changeset(
        base,
        vec![assign(2, None, Some(b)), assign(1, None, Some(b))],
    );
    store.apply(validate(&store, first).unwrap()).unwrap();

    let found = rejections(&store, second);
    assert_eq!(
        found,
        vec![Rejection {
            op_index: Some(1),
            op: Some(assign(1, None, Some(b))),
            reason: Reason::StaleAssignment {
                observation: obs(1),
                expected: None,
                actual: Some(a)
            },
        }]
    );
    assert!(found[0].to_string().contains("operation 1"));
    assert_eq!(store.assignments(&[obs(2)]).unwrap(), vec![None]);
}

pub fn rejects_a_reused_identity_id<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b) = (author.identity(), author.identity());
    commit(&mut store, &mut author, vec![create(a, widget())]);

    let again = author.changeset(Seq(1), vec![create(a, gadget())]);
    assert_eq!(
        only_reason(&store, again),
        (Some(0), Reason::IdentityExists { identity: a })
    );
    let twice = author.changeset(Seq(1), vec![create(b, widget()), create(b, widget())]);
    assert_eq!(
        only_reason(&store, twice),
        (Some(1), Reason::IdentityExists { identity: b })
    );
}

pub fn rejects_an_unknown_observation<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let store = make(&observations());
    let mut author = Author::new();
    let a = author.identity();
    let changeset = author.changeset(
        Seq::ZERO,
        vec![create(a, widget()), assign(99, None, Some(a))],
    );
    assert_eq!(
        only_reason(&store, changeset),
        (
            Some(1),
            Reason::UnknownObservation {
                observation: obs(99)
            }
        )
    );
}

pub fn rejects_an_unknown_identity<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let store = make(&observations());
    let mut author = Author::new();
    let missing = author.identity();
    let changeset = author.changeset(Seq::ZERO, vec![assign(1, None, Some(missing))]);
    assert_eq!(
        only_reason(&store, changeset),
        (Some(0), Reason::UnknownIdentity { identity: missing })
    );
}

pub fn rejects_an_entity_type_mismatch<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let a = author.identity();
    commit(&mut store, &mut author, vec![create(a, widget())]);

    let changeset = author.changeset(Seq(1), vec![assign(6, None, Some(a))]);
    assert_eq!(
        only_reason(&store, changeset),
        (
            Some(0),
            Reason::EntityTypeMismatch {
                observation: obs(6),
                observation_type: gadget(),
                identity: a,
                identity_type: widget(),
            }
        )
    );
}

pub fn rejects_assigning_an_observation_twice_in_one_changeset<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let store = make(&observations());
    let mut author = Author::new();
    let (a, b) = (author.identity(), author.identity());
    let changeset = author.changeset(
        Seq::ZERO,
        vec![
            create(a, widget()),
            create(b, widget()),
            assign(1, None, Some(a)),
            assign(1, Some(a), Some(b)),
        ],
    );
    assert_eq!(
        only_reason(&store, changeset),
        (
            Some(3),
            Reason::ObservationAssignedTwice {
                observation: obs(1),
                first_op: 2
            }
        )
    );
}

pub fn rejects_an_empty_changeset<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let store = make(&observations());
    let changeset = Author::new().changeset(Seq::ZERO, vec![]);
    assert_eq!(
        only_reason(&store, changeset),
        (None, Reason::EmptyChangeset)
    );
}

pub fn rejects_a_base_ahead_of_the_log<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let store = make(&observations());
    let mut author = Author::new();
    let a = author.identity();
    let changeset = author.changeset(Seq(3), vec![create(a, widget())]);
    assert_eq!(
        only_reason(&store, changeset),
        (
            None,
            Reason::BaseAhead {
                base: Seq(3),
                head: Seq::ZERO
            }
        )
    );
}

pub fn rejects_a_changeset_committed_twice<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let a = author.identity();
    let changeset = author.changeset(Seq::ZERO, vec![create(a, widget())]);
    store
        .apply(validate(&store, changeset.clone()).unwrap())
        .unwrap();

    let found = rejections(&store, changeset);
    assert!(
        found.contains(&Rejection {
            op_index: None,
            op: None,
            reason: Reason::AlreadyCommitted { seq: Seq(1) }
        }),
        "{found:?}"
    );
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

fn status<S: IdentityStore>(store: &S, identity: IdentityId) -> (IdentityStatus, Option<Merged>) {
    let record = store.identities(&[identity]).unwrap().remove(0).unwrap();
    (record.status, record.merged)
}

/// Three widget identities: `a` holds 1 and 2, `b` holds 3, `c` holds 4.
/// Observation 5 has no identity. Committed at position 1.
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

pub fn merges_identities_and_keeps_the_merged_id<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, c) = three_widgets(&mut store, &mut author);

    commit(&mut store, &mut author, vec![merge(a, b)]);

    assert_eq!(
        store.assignments(&[obs(1), obs(2), obs(3)]).unwrap(),
        vec![Some(b), Some(b), Some(b)]
    );
    assert_eq!(
        store.members(&[a, b]).unwrap(),
        vec![(b, obs(1)), (b, obs(2)), (b, obs(3))]
    );
    assert_eq!(
        status(&store, a),
        (
            IdentityStatus::MergedInto(b),
            Some(Merged {
                into: b,
                seq: Seq(2)
            })
        )
    );
    assert_eq!(status(&store, b), (IdentityStatus::Active, None));
    assert_eq!(
        store.membership_changed(&[a, b, c]).unwrap(),
        vec![Seq(2), Seq(2), Seq(1)]
    );
    assert_eq!(store.left_at(a, Seq(2)).unwrap(), vec![obs(1), obs(2)]);

    // The merged ID stays: it has its history, and nothing joins it.
    let history: Vec<_> = store
        .snapshot()
        .unwrap()
        .memberships
        .into_iter()
        .filter(|row| row.observation == obs(1))
        .collect();
    assert_eq!(
        history,
        vec![
            Membership {
                observation: obs(1),
                identity: a,
                from: Seq(1),
                to: Some(Seq(2))
            },
            Membership {
                observation: obs(1),
                identity: b,
                from: Seq(2),
                to: None
            },
        ]
    );
    let joins = author.changeset(Seq(2), vec![assign(5, None, Some(a))]);
    assert_eq!(
        only_reason(&store, joins),
        (
            Some(0),
            Reason::IdentityNotActive {
                identity: a,
                merged_into: b
            }
        )
    );
    assert_replays(&store);
}

pub fn a_chain_of_merges_points_at_the_active_identity<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, c) = three_widgets(&mut store, &mut author);

    commit(&mut store, &mut author, vec![merge(a, b)]);
    commit(&mut store, &mut author, vec![merge(b, c)]);

    // `a` was merged into `b`, and `c` is the identity that stands for it.
    assert_eq!(
        status(&store, a),
        (
            IdentityStatus::MergedInto(c),
            Some(Merged {
                into: b,
                seq: Seq(2)
            })
        )
    );
    assert_eq!(
        status(&store, b),
        (
            IdentityStatus::MergedInto(c),
            Some(Merged {
                into: c,
                seq: Seq(3)
            })
        )
    );
    assert_eq!(
        store.members(&[a, b, c]).unwrap(),
        vec![(c, obs(1)), (c, obs(2)), (c, obs(3)), (c, obs(4))]
    );

    // With `b` active again, `a` stands for `b` once more.
    commit(&mut store, &mut author, vec![reinstate(b)]);
    assert_eq!(status(&store, a).0, IdentityStatus::MergedInto(b));
    assert_eq!(status(&store, b), (IdentityStatus::Active, None));
    assert_eq!(store.merged_before(b, Seq(4)).unwrap(), Some(c));
    assert_eq!(store.merged_before(b, Seq(3)).unwrap(), None);
    assert_replays(&store);
}

pub fn rejects_merges_that_cannot_be_made<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, c) = three_widgets(&mut store, &mut author);
    let (g, unknown) = (author.identity(), author.identity());
    commit(
        &mut store,
        &mut author,
        vec![create(g, gadget()), merge(c, b)],
    );
    let head = store.head().unwrap();

    let cases = [
        (merge(a, a), Reason::MergeIntoItself { identity: a }),
        (
            merge(a, unknown),
            Reason::UnknownIdentity { identity: unknown },
        ),
        (
            merge(unknown, a),
            Reason::UnknownIdentity { identity: unknown },
        ),
        (
            merge(a, c),
            Reason::IdentityNotActive {
                identity: c,
                merged_into: b,
            },
        ),
        (
            merge(c, a),
            Reason::IdentityNotActive {
                identity: c,
                merged_into: b,
            },
        ),
        (
            merge(a, g),
            Reason::IdentityTypeMismatch {
                from: a,
                from_type: widget(),
                into: g,
                into_type: gadget(),
            },
        ),
    ];
    for (op, reason) in cases {
        let changeset = author.changeset(head, vec![op]);
        assert_eq!(only_reason(&store, changeset), (Some(0), reason));
    }

    // A changeset has one position in history, so it merges no chain.
    let d = author.identity();
    let chain = author.changeset(head, vec![create(d, widget()), merge(a, d), merge(d, b)]);
    assert_eq!(
        only_reason(&store, chain),
        (
            Some(2),
            Reason::MembersMovedTwice {
                identity: d,
                first_op: 1
            }
        )
    );
    let twice = author.changeset(head, vec![merge(a, b), merge(a, b)]);
    assert_eq!(
        only_reason(&store, twice),
        (
            Some(1),
            Reason::IdentityNotActive {
                identity: a,
                merged_into: b
            }
        )
    );
}

pub fn rejects_a_merge_whose_members_changed<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, c) = three_widgets(&mut store, &mut author);

    // Two merges are written against position 1. Then `a` gains a member the
    // author of neither merge has seen.
    let stale_from = author.changeset(Seq(1), vec![merge(a, c)]);
    let stale_into = author.changeset(Seq(1), vec![merge(c, a)]);
    let untouched = author.changeset(Seq(1), vec![merge(b, c)]);
    commit(&mut store, &mut author, vec![assign(5, None, Some(a))]);

    for changeset in [stale_from, stale_into] {
        assert_eq!(
            only_reason(&store, changeset),
            (
                Some(0),
                Reason::StaleMerge {
                    identity: a,
                    changed: Seq(2),
                    base: Seq(1)
                }
            )
        );
    }
    // A merge of identities nothing touched still commits from an old base.
    store.apply(validate(&store, untouched).unwrap()).unwrap();
    assert_eq!(store.assignments(&[obs(3)]).unwrap(), vec![Some(c)]);
}

pub fn rejects_moving_an_observation_twice_through_a_merge<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, c) = three_widgets(&mut store, &mut author);

    // The merge moved observation 1 to `b`. Moving it on is a second move.
    let onward = author.changeset(Seq(1), vec![merge(a, b), assign(1, Some(b), Some(c))]);
    assert_eq!(
        only_reason(&store, onward),
        (
            Some(1),
            Reason::ObservationAssignedTwice {
                observation: obs(1),
                first_op: 0
            }
        )
    );
    let behind = author.changeset(Seq(1), vec![merge(a, b), assign(1, Some(a), Some(c))]);
    assert_eq!(
        only_reason(&store, behind),
        (
            Some(1),
            Reason::StaleAssignment {
                observation: obs(1),
                expected: Some(a),
                actual: Some(b)
            }
        )
    );
    // An observation that joins an identity cannot be merged away with it.
    let joined = author.changeset(Seq(1), vec![assign(5, None, Some(a)), merge(a, b)]);
    assert_eq!(
        only_reason(&store, joined),
        (
            Some(1),
            Reason::MembersMovedTwice {
                identity: a,
                first_op: 0
            }
        )
    );
}

pub fn reinstates_a_merged_identity<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, _) = three_widgets(&mut store, &mut author);
    let unknown = author.identity();
    commit(&mut store, &mut author, vec![merge(a, b)]);

    let cases = [
        (
            vec![reinstate(b)],
            0,
            Reason::IdentityIsActive { identity: b },
        ),
        (
            vec![reinstate(unknown)],
            0,
            Reason::UnknownIdentity { identity: unknown },
        ),
        (
            vec![reinstate(a), reinstate(a)],
            1,
            Reason::IdentityChangedTwice {
                identity: a,
                first_op: 0,
            },
        ),
        (
            vec![reinstate(a), merge(a, b)],
            1,
            Reason::IdentityChangedTwice {
                identity: a,
                first_op: 0,
            },
        ),
    ];
    for (ops, index, reason) in cases {
        let changeset = author.changeset(Seq(2), ops);
        assert_eq!(only_reason(&store, changeset), (Some(index), reason));
    }

    // A reinstated identity is active and empty. Members come back by
    // assignment, here in the same changeset.
    commit(
        &mut store,
        &mut author,
        vec![reinstate(a), assign(2, Some(b), Some(a))],
    );
    assert_eq!(status(&store, a), (IdentityStatus::Active, None));
    assert_eq!(
        store.members(&[a, b]).unwrap(),
        vec![(a, obs(2)), (b, obs(1)), (b, obs(3))]
    );
    assert_replays(&store);
}

pub fn records_and_removes_constraints<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, _, _) = three_widgets(&mut store, &mut author);

    // The pair has no direction: it is recorded smaller ID first.
    commit(
        &mut store,
        &mut author,
        vec![cannot_link(3, 1), must_link(2, 1)],
    );
    let apart = Constraint::new(ConstraintKind::CannotLink, obs(1), obs(3));
    let together = Constraint::new(ConstraintKind::MustLink, obs(1), obs(2));
    assert_eq!((apart.a, apart.b), (obs(1), obs(3)));
    assert_eq!(
        store.constraints_on(&[obs(1)]).unwrap(),
        vec![together, apart]
    );
    assert_eq!(
        store.constraints_on(&[obs(3), obs(5)]).unwrap(),
        vec![apart]
    );
    assert_eq!(store.constraints_on(&[obs(4)]).unwrap(), vec![]);
    assert_eq!(
        store.constraints_of_members(&[a]).unwrap(),
        vec![together, apart]
    );

    commit(
        &mut store,
        &mut author,
        vec![unconstrain(ConstraintKind::CannotLink, 1, 3)],
    );
    assert_eq!(store.constraints_on(&[obs(3)]).unwrap(), vec![]);
    // The same constraint can be recorded again. Each time is an interval.
    commit(&mut store, &mut author, vec![cannot_link(1, 3)]);
    assert_eq!(
        store.snapshot().unwrap().constraints,
        vec![
            ConstraintRecord {
                constraint: together,
                from: Seq(2),
                to: None
            },
            ConstraintRecord {
                constraint: apart,
                from: Seq(2),
                to: Some(Seq(3))
            },
            ConstraintRecord {
                constraint: apart,
                from: Seq(4),
                to: None
            },
        ]
    );
    assert_replays(&store);
}

pub fn rejects_constraints_that_cannot_be_recorded<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    three_widgets(&mut store, &mut author);
    commit(&mut store, &mut author, vec![cannot_link(1, 3)]);

    let (apart, together) = (ConstraintKind::CannotLink, ConstraintKind::MustLink);
    let cases = [
        (
            vec![cannot_link(1, 1)],
            0,
            Reason::SameObservation {
                observation: obs(1),
            },
        ),
        (
            vec![cannot_link(1, 99)],
            0,
            Reason::UnknownObservation {
                observation: obs(99),
            },
        ),
        (
            vec![must_link(1, 6)],
            0,
            Reason::ConstraintAcrossTypes {
                a: obs(1),
                a_type: widget(),
                b: obs(6),
                b_type: gadget(),
            },
        ),
        (
            vec![cannot_link(3, 1)],
            0,
            Reason::ConstraintExists {
                kind: apart,
                a: obs(3),
                b: obs(1),
            },
        ),
        // A pair has one constraint, whichever kind came first.
        (
            vec![must_link(1, 3)],
            0,
            Reason::ConstraintExists {
                kind: apart,
                a: obs(1),
                b: obs(3),
            },
        ),
        (
            vec![unconstrain(together, 1, 3)],
            0,
            Reason::UnknownConstraint {
                kind: together,
                a: obs(1),
                b: obs(3),
            },
        ),
        (
            vec![unconstrain(apart, 1, 4)],
            0,
            Reason::UnknownConstraint {
                kind: apart,
                a: obs(1),
                b: obs(4),
            },
        ),
        (
            vec![unconstrain(apart, 1, 3), cannot_link(1, 3)],
            1,
            Reason::ConstraintChangedTwice {
                kind: apart,
                a: obs(1),
                b: obs(3),
                first_op: 0,
            },
        ),
    ];
    for (ops, index, reason) in cases {
        let changeset = author.changeset(Seq(2), ops);
        assert_eq!(only_reason(&store, changeset), (Some(index), reason));
    }
}

pub fn rejects_a_state_that_breaks_a_constraint<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, c) = three_widgets(&mut store, &mut author);
    // 1 (in `a`) and 3 (in `b`) are different things. 4 (in `c`) and 5 (no
    // identity yet) are the same thing.
    commit(
        &mut store,
        &mut author,
        vec![cannot_link(1, 3), must_link(4, 5)],
    );
    let (apart, together) = (ConstraintKind::CannotLink, ConstraintKind::MustLink);
    let broken = |kind, a, b, identity_a, identity_b| Reason::ConstraintViolated {
        kind,
        a: obs(a),
        b: obs(b),
        identity_a: Some(identity_a),
        identity_b: Some(identity_b),
    };

    let cases = [
        // An assignment that brings the two together.
        (
            vec![assign(3, Some(b), Some(a))],
            0,
            broken(apart, 1, 3, a, a),
        ),
        // A merge that does, in either direction.
        (vec![merge(a, b)], 0, broken(apart, 1, 3, b, b)),
        (vec![merge(b, a)], 0, broken(apart, 1, 3, a, a)),
        // Both moved elsewhere: the later operation is the one named.
        (
            vec![assign(1, Some(a), Some(c)), assign(3, Some(b), Some(c))],
            1,
            broken(apart, 1, 3, c, c),
        ),
        // An assignment that parts two that must stay together.
        (
            vec![assign(5, None, Some(a))],
            0,
            broken(together, 4, 5, c, a),
        ),
        // A constraint the state already contradicts.
        (vec![cannot_link(1, 2)], 0, broken(apart, 1, 2, a, a)),
        (vec![must_link(2, 3)], 0, broken(together, 2, 3, a, b)),
    ];
    for (ops, index, reason) in cases {
        let changeset = author.changeset(Seq(2), ops);
        assert_eq!(only_reason(&store, changeset), (Some(index), reason));
    }

    // What the constraints allow commits: 5 joins 4, and 4 can leave its
    // identity, because an observation with no identity breaks nothing.
    commit(&mut store, &mut author, vec![assign(5, None, Some(c))]);
    commit(&mut store, &mut author, vec![assign(4, Some(c), None)]);
    // Once the constraint is removed, so is what it forbade, in one changeset.
    commit(
        &mut store,
        &mut author,
        vec![unconstrain(apart, 1, 3), merge(a, b)],
    );
    assert_eq!(
        store.assignments(&[obs(1), obs(3)]).unwrap(),
        vec![Some(b), Some(b)]
    );
    assert_replays(&store);
}

pub fn splits_with_a_constraint_in_one_changeset<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, _, _) = three_widgets(&mut store, &mut author);

    // A split is not an operation of its own: it is a new identity, a move
    // and, usually, a constraint that keeps the two sides apart.
    let d = author.identity();
    commit(
        &mut store,
        &mut author,
        vec![
            cannot_link(1, 2),
            create(d, widget()),
            assign(2, Some(a), Some(d)),
        ],
    );
    assert_eq!(
        store.members(&[a, d]).unwrap(),
        vec![(a, obs(1)), (d, obs(2))]
    );

    let back = author.changeset(Seq(2), vec![merge(d, a)]);
    assert!(matches!(
        only_reason(&store, back),
        (Some(0), Reason::ConstraintViolated { .. })
    ));
    assert_replays(&store);
}

pub fn applies_mixed_operations_as_written<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, c) = three_widgets(&mut store, &mut author);
    commit(&mut store, &mut author, vec![merge(a, b)]);
    commit(&mut store, &mut author, vec![merge(b, c)]);

    // One changeset that reinstates, merges, moves and constrains. A store
    // may apply the kinds of operation in any order it likes, and has to end
    // where applying them one by one ends.
    let d = author.identity();
    commit(
        &mut store,
        &mut author,
        vec![
            reinstate(b),
            create(d, widget()),
            assign(5, None, Some(d)),
            assign(3, Some(c), Some(b)),
            cannot_link(2, 4),
            assign(2, Some(c), Some(b)),
            merge(c, d),
        ],
    );
    assert_eq!(
        store.members(&[a, b, c, d]).unwrap(),
        vec![
            (b, obs(2)),
            (b, obs(3)),
            (d, obs(1)),
            (d, obs(4)),
            (d, obs(5))
        ]
    );
    assert_eq!(status(&store, a).0, IdentityStatus::MergedInto(b));
    assert_eq!(status(&store, b).0, IdentityStatus::Active);
    assert_eq!(status(&store, c).0, IdentityStatus::MergedInto(d));

    // Reinstating an identity and merging into it the one that stood for it
    // turns the pair around.
    commit(&mut store, &mut author, vec![reinstate(c), merge(d, c)]);
    assert_eq!(status(&store, c), (IdentityStatus::Active, None));
    assert_eq!(status(&store, d).0, IdentityStatus::MergedInto(c));
    assert_eq!(
        store.members(&[c, d]).unwrap(),
        vec![(c, obs(1)), (c, obs(4)), (c, obs(5))]
    );

    let together = author.changeset(store.head().unwrap(), vec![merge(b, c)]);
    assert!(matches!(
        only_reason(&store, together),
        (Some(0), Reason::ConstraintViolated { .. })
    ));
    assert_replays(&store);
}

fn undo<S: IdentityStore>(store: &mut S, author: &mut Author, seq: Seq) -> Vec<Op> {
    let log = store.log().unwrap();
    let entry = log.iter().find(|entry| entry.seq == seq).unwrap();
    let ops = inverse(store, entry).unwrap();
    commit(store, author, ops.clone());
    ops
}

pub fn undoes_assignments_and_constraints<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, _) = three_widgets(&mut store, &mut author);
    commit(&mut store, &mut author, vec![cannot_link(1, 4)]);
    let before = store.snapshot().unwrap();

    let d = author.identity();
    let seq = commit(
        &mut store,
        &mut author,
        vec![
            create(d, widget()),
            assign(2, Some(a), Some(d)),
            assign(5, None, Some(b)),
            assign(3, Some(b), None),
            cannot_link(1, 2),
            unconstrain(ConstraintKind::CannotLink, 1, 4),
        ],
    );

    // Last operation first. The identity that was created stays.
    let ops = undo(&mut store, &mut author, seq);
    assert_eq!(
        ops,
        vec![
            cannot_link(1, 4),
            unconstrain(ConstraintKind::CannotLink, 1, 2),
            assign(3, None, Some(b)),
            assign(5, Some(b), None),
            assign(2, Some(d), Some(a)),
        ]
    );
    let after = store.snapshot().unwrap();
    let current = |snapshot: &crate::state::Snapshot| -> Vec<Membership> {
        snapshot
            .memberships
            .iter()
            .filter(|row| row.to.is_none())
            .map(|row| Membership {
                from: Seq::ZERO,
                ..*row
            })
            .collect()
    };
    assert_eq!(current(&after), current(&before));
    assert_eq!(store.constraints_on(&[obs(1)]).unwrap().len(), 1);
    assert_eq!(status(&store, d), (IdentityStatus::Active, None));
    assert_eq!(store.members(&[d]).unwrap(), vec![]);

    // An undo that no longer fits is rejected like any changeset.
    let log = store.log().unwrap();
    let again = inverse(&store, &log[2]).unwrap();
    let changeset = author.changeset(store.head().unwrap(), again);
    assert!(matches!(
        rejections(&store, changeset)[0].reason,
        Reason::ConstraintExists { .. }
    ));
    assert_replays(&store);
}

pub fn undoes_a_merge_and_the_undo<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, c) = three_widgets(&mut store, &mut author);

    // The merge is in a changeset that also moves a member out of `a` first.
    let merged = commit(
        &mut store,
        &mut author,
        vec![assign(2, Some(a), Some(c)), merge(a, b)],
    );
    assert_eq!(
        store.members(&[a, b, c]).unwrap(),
        vec![(b, obs(1)), (b, obs(3)), (c, obs(2)), (c, obs(4))]
    );

    // Undoing it reinstates `a` and gives it back what the merge took and
    // what the assignment took. Observation 3 was in `b` all along.
    let ops = undo(&mut store, &mut author, merged);
    assert_eq!(
        ops,
        vec![
            reinstate(a),
            assign(1, Some(b), Some(a)),
            assign(2, Some(c), Some(a)),
        ]
    );
    assert_eq!(status(&store, a), (IdentityStatus::Active, None));
    assert_eq!(
        store.members(&[a, b, c]).unwrap(),
        vec![(a, obs(1)), (a, obs(2)), (b, obs(3)), (c, obs(4))]
    );

    // Undoing the undo merges `a` into `b` again.
    let undone = store.head().unwrap();
    let ops = undo(&mut store, &mut author, undone);
    assert_eq!(
        ops,
        vec![
            assign(2, Some(a), Some(c)),
            assign(1, Some(a), Some(b)),
            merge(a, b),
        ]
    );
    assert_eq!(status(&store, a).0, IdentityStatus::MergedInto(b));
    assert_eq!(
        store.members(&[a, b, c]).unwrap(),
        vec![(b, obs(1)), (b, obs(3)), (c, obs(2)), (c, obs(4))]
    );

    // An observation that has left the surviving identity since the merge is
    // left where it is.
    let undone = store.head().unwrap();
    undo(&mut store, &mut author, undone);
    let merged = commit(&mut store, &mut author, vec![merge(a, b)]);
    let d = author.identity();
    commit(
        &mut store,
        &mut author,
        vec![create(d, widget()), assign(1, Some(b), Some(d))],
    );
    let ops = undo(&mut store, &mut author, merged);
    assert_eq!(ops, vec![reinstate(a), assign(2, Some(b), Some(a))]);
    assert_eq!(
        store.members(&[a, b, d]).unwrap(),
        vec![(a, obs(2)), (b, obs(3)), (d, obs(1))]
    );
    assert_replays(&store);
}

pub fn accepts_a_stale_base_when_preconditions_hold<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b) = (author.identity(), author.identity());

    // A draft written against the empty log waits while another commits.
    let waiting = author.changeset(
        Seq::ZERO,
        vec![create(a, widget()), assign(1, None, Some(a))],
    );
    commit(
        &mut store,
        &mut author,
        vec![create(b, widget()), assign(2, None, Some(b))],
    );

    let committed = store.apply(validate(&store, waiting).unwrap()).unwrap();
    assert_eq!(committed.seq, Seq(2));
    assert_eq!(
        store.assignments(&[obs(1), obs(2)]).unwrap(),
        vec![Some(a), Some(b)]
    );
    assert_eq!(store.log().unwrap()[1].changeset.base, Seq::ZERO);
}

pub fn a_rejected_changeset_changes_nothing<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b) = (author.identity(), author.identity());
    commit(
        &mut store,
        &mut author,
        vec![create(a, widget()), assign(1, None, Some(a))],
    );
    let before = store.snapshot().unwrap();
    let log_before = store.log().unwrap();

    // The first two operations are fine. The third is not.
    let changeset = author.changeset(
        Seq(1),
        vec![
            create(b, widget()),
            assign(2, None, Some(b)),
            assign(1, None, Some(b)),
        ],
    );
    assert_eq!(rejections(&store, changeset).len(), 1);
    assert_eq!(store.snapshot().unwrap(), before);
    assert_eq!(store.log().unwrap(), log_before);
}

pub fn replaying_the_log_reproduces_the_state<S: IdentityStore>(
    make: impl FnOnce(&[(ObservationId, EntityType)]) -> S,
) {
    let mut store = make(&observations());
    let mut author = Author::new();
    let (a, b, c) = (author.identity(), author.identity(), author.identity());
    commit(
        &mut store,
        &mut author,
        vec![
            create(a, widget()),
            assign(1, None, Some(a)),
            assign(2, None, Some(a)),
            assign(3, None, Some(a)),
        ],
    );
    commit(
        &mut store,
        &mut author,
        vec![
            create(b, widget()),
            assign(3, Some(a), Some(b)),
            create(c, gadget()),
            assign(6, None, Some(c)),
        ],
    );
    commit(&mut store, &mut author, vec![assign(2, Some(a), None)]);

    let log = store.log().unwrap();
    assert_eq!(
        log.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![Seq(1), Seq(2), Seq(3)],
        "log positions are dense"
    );
    for entry in &log {
        assert_eq!(entry.content_hash, entry.changeset.content_hash());
        assert_eq!(entry.changeset.actor, Actor::User("conformance".to_owned()));
        assert_eq!(entry.changeset.reason.as_deref(), Some("conformance suite"));
    }
    assert_eq!(log[1].changeset.ops.len(), 4);

    let replayed = MemoryState::replay(&log).unwrap();
    assert_eq!(replayed.snapshot().unwrap(), store.snapshot().unwrap());
}
