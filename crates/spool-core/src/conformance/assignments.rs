//! Cases about identities and the records assigned to them.

use super::{
    Author, assign, commit, create, expect_rejections, fixture_records, gadget, obs, only_reason,
    widget,
};
use crate::ids::{EntityType, RecordId, Seq};
use crate::state::{IdentityEntry, IdentityStatus, IdentityStore, Membership};
use crate::validate::{Reason, Rejection, validate};

pub fn creates_an_identity_and_attaches_records<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&fixture_records());
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
            Some(IdentityEntry {
                id: b,
                entity_type: gadget(),
                created: Seq(1),
                status: IdentityStatus::Active,
                merged: None,
            }),
            None,
            Some(IdentityEntry {
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&fixture_records());
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
    let interval = |record, identity, from, to: Option<u64>| Membership {
        record: obs(record),
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&fixture_records());
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

    let found = expect_rejections(&store, second);
    assert_eq!(
        found,
        vec![Rejection {
            op_index: Some(1),
            op: Some(assign(1, None, Some(b))),
            reason: Reason::StaleAssignment {
                record: obs(1),
                expected: None,
                actual: Some(a)
            },
        }]
    );
    assert!(found[0].to_string().contains("operation 1"));
    assert_eq!(store.assignments(&[obs(2)]).unwrap(), vec![None]);
}

pub fn rejects_a_reused_identity_id<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&fixture_records());
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

pub fn rejects_an_unknown_record<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let store = make(&fixture_records());
    let mut author = Author::new();
    let a = author.identity();
    let changeset = author.changeset(
        Seq::ZERO,
        vec![create(a, widget()), assign(99, None, Some(a))],
    );
    assert_eq!(
        only_reason(&store, changeset),
        (Some(1), Reason::UnknownRecord { record: obs(99) })
    );
}

pub fn rejects_an_unknown_identity<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let store = make(&fixture_records());
    let mut author = Author::new();
    let missing = author.identity();
    let changeset = author.changeset(Seq::ZERO, vec![assign(1, None, Some(missing))]);
    assert_eq!(
        only_reason(&store, changeset),
        (Some(0), Reason::UnknownIdentity { identity: missing })
    );
}

pub fn rejects_an_entity_type_mismatch<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&fixture_records());
    let mut author = Author::new();
    let a = author.identity();
    commit(&mut store, &mut author, vec![create(a, widget())]);

    let changeset = author.changeset(Seq(1), vec![assign(6, None, Some(a))]);
    assert_eq!(
        only_reason(&store, changeset),
        (
            Some(0),
            Reason::EntityTypeMismatch {
                record: obs(6),
                record_type: gadget(),
                identity: a,
                identity_type: widget(),
            }
        )
    );
}

pub fn rejects_assigning_an_record_twice_in_one_changeset<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let store = make(&fixture_records());
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
            Reason::RecordAssignedTwice {
                record: obs(1),
                first_op: 2
            }
        )
    );
}
