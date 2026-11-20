//! Cases about the log: where it starts, what a changeset needs to join
//! it, and that replaying it gives the state.

use super::{
    Author, assign, commit, create, gadget, obs, only_reason, records, rejections, widget,
};
use crate::ids::{Actor, EntityType, RecordId, Seq};
use crate::memory::MemoryState;
use crate::state::IdentityStore;
use crate::validate::{Reason, Rejection, validate};

pub fn starts_empty<S: IdentityStore>(make: impl FnOnce(&[(RecordId, EntityType)]) -> S) {
    let store = make(&records());
    assert_eq!(store.head().unwrap(), Seq::ZERO);
    assert!(store.log().unwrap().is_empty());
    let snapshot = store.snapshot().unwrap();
    assert!(snapshot.identities.is_empty() && snapshot.memberships.is_empty());
    assert_eq!(store.assignments(&[obs(1)]).unwrap(), vec![None]);
    assert_eq!(
        store.record_types(&[obs(6), obs(99), obs(1)]).unwrap(),
        vec![Some(gadget()), None, Some(widget())]
    );
}

pub fn rejects_an_empty_changeset<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let store = make(&records());
    let changeset = Author::new().changeset(Seq::ZERO, vec![]);
    assert_eq!(
        only_reason(&store, changeset),
        (None, Reason::EmptyChangeset)
    );
}

pub fn rejects_a_base_ahead_of_the_log<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let store = make(&records());
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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

pub fn accepts_a_stale_base_when_preconditions_hold<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
