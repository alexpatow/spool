//! Cases about constraints: recording and removing them, and the states
//! they forbid.

use super::{
    Author, assert_replays, assign, cannot_link, commit, create, gadget, merge, must_link, obs,
    only_reason, records, three_widgets, unconstrain, widget,
};
use crate::changeset::ConstraintKind;
use crate::ids::{EntityType, RecordId, Seq};
use crate::state::{Constraint, ConstraintEntry, IdentityStore};
use crate::validate::Reason;

pub fn records_and_removes_constraints<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
            ConstraintEntry {
                constraint: together,
                from: Seq(2),
                to: None
            },
            ConstraintEntry {
                constraint: apart,
                from: Seq(2),
                to: Some(Seq(3))
            },
            ConstraintEntry {
                constraint: apart,
                from: Seq(4),
                to: None
            },
        ]
    );
    assert_replays(&store);
}

pub fn rejects_constraints_that_cannot_be_recorded<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
    let mut author = Author::new();
    three_widgets(&mut store, &mut author);
    commit(&mut store, &mut author, vec![cannot_link(1, 3)]);

    let (apart, together) = (ConstraintKind::CannotLink, ConstraintKind::MustLink);
    let cases = [
        (
            vec![cannot_link(1, 1)],
            0,
            Reason::SameRecord { record: obs(1) },
        ),
        (
            vec![cannot_link(1, 99)],
            0,
            Reason::UnknownRecord { record: obs(99) },
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
    // identity, because a record with no identity breaks nothing.
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
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&records());
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
