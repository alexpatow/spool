//! Cases about merging identities and reinstating a merged one.

use super::{
    Author, assert_replays, assign, cannot_link, commit, create, gadget, merge, obs, only_reason,
    records, reinstate, status, three_widgets, widget,
};
use crate::ids::{EntityType, RecordId, Seq};
use crate::state::{IdentityStatus, IdentityStore, Membership, Merged};
use crate::validate::{Reason, validate};

pub fn merges_identities_and_keeps_the_merged_id<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
        .filter(|row| row.record == obs(1))
        .collect();
    assert_eq!(
        history,
        vec![
            Membership {
                record: obs(1),
                identity: a,
                from: Seq(1),
                to: Some(Seq(2))
            },
            Membership {
                record: obs(1),
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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

pub fn rejects_moving_an_record_twice_through_a_merge<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
    let mut author = Author::new();
    let (a, b, c) = three_widgets(&mut store, &mut author);

    // The merge moved record 1 to `b`. Moving it on is a second move.
    let onward = author.changeset(Seq(1), vec![merge(a, b), assign(1, Some(b), Some(c))]);
    assert_eq!(
        only_reason(&store, onward),
        (
            Some(1),
            Reason::RecordAssignedTwice {
                record: obs(1),
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
                record: obs(1),
                expected: Some(a),
                actual: Some(b)
            }
        )
    );
    // A record that joins an identity cannot be merged away with it.
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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

pub fn applies_mixed_operations_as_written<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
