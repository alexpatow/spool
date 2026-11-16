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
use crate::state::{IdentityRecord, IdentityStatus, IdentityStore, Membership};
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
            rejects_operations_that_are_not_implemented,
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
                status: IdentityStatus::Active
            }),
            None,
            Some(IdentityRecord {
                id: a,
                entity_type: widget(),
                created: Seq(1),
                status: IdentityStatus::Active
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

pub fn rejects_operations_that_are_not_implemented<S: IdentityStore>(
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

    let later = [
        Op::Merge { from: a, into: b },
        Op::Constrain {
            kind: ConstraintKind::CannotLink,
            a: obs(1),
            b: obs(2),
        },
        Op::Unconstrain {
            kind: ConstraintKind::MustLink,
            a: obs(1),
            b: obs(2),
        },
        Op::Reinstate { identity: a },
    ];
    for op in later {
        let kind = op.kind().to_owned();
        let changeset = author.changeset(Seq(1), vec![op]);
        assert_eq!(
            only_reason(&store, changeset),
            (Some(0), Reason::NotImplemented { kind })
        );
    }
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
