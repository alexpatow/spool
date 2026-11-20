//! Cases about undoing a committed changeset.

use super::{
    Author, assert_replays, assign, cannot_link, commit, create, expect_rejections,
    fixture_records, merge, obs, read_identity_status, reinstate, three_widgets, unconstrain,
    widget,
};
use crate::changeset::{ConstraintKind, Op};
use crate::ids::{EntityType, RecordId, Seq};
use crate::state::{IdentityStatus, IdentityStore, Membership};
use crate::undo::inverse;
use crate::validate::Reason;

fn undo<S: IdentityStore>(store: &mut S, author: &mut Author, seq: Seq) -> Vec<Op> {
    let log = store.log().unwrap();
    let entry = log.iter().find(|entry| entry.seq == seq).unwrap();
    let ops = inverse(store, entry).unwrap();
    commit(store, author, ops.clone());
    ops
}

pub fn undoes_assignments_and_constraints<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&fixture_records());
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
    assert_eq!(
        read_identity_status(&store, d),
        (IdentityStatus::Active, None)
    );
    assert_eq!(store.members(&[d]).unwrap(), vec![]);

    // An undo that no longer fits is rejected like any changeset.
    let log = store.log().unwrap();
    let again = inverse(&store, &log[2]).unwrap();
    let changeset = author.changeset(store.head().unwrap(), again);
    assert!(matches!(
        expect_rejections(&store, changeset)[0].reason,
        Reason::ConstraintExists { .. }
    ));
    assert_replays(&store);
}

pub fn undoes_a_merge_and_the_undo<S: IdentityStore>(
    make: impl FnOnce(&[(RecordId, EntityType)]) -> S,
) {
    let mut store = make(&fixture_records());
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
    // what the assignment took. Record 3 was in `b` all along.
    let ops = undo(&mut store, &mut author, merged);
    assert_eq!(
        ops,
        vec![
            reinstate(a),
            assign(1, Some(b), Some(a)),
            assign(2, Some(c), Some(a)),
        ]
    );
    assert_eq!(
        read_identity_status(&store, a),
        (IdentityStatus::Active, None)
    );
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
    assert_eq!(
        read_identity_status(&store, a).0,
        IdentityStatus::MergedInto(b)
    );
    assert_eq!(
        store.members(&[a, b, c]).unwrap(),
        vec![(b, obs(1)), (b, obs(3)), (c, obs(2)), (c, obs(4))]
    );

    // A record that has left the surviving identity since the merge is
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
