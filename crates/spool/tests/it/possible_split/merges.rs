//! A `possible_split` item through a merge and back.

use super::*;

#[test]
fn the_item_goes_with_its_records_into_a_merge_and_back() {
    let engine = engine();
    let (held, item) = asked(&engine);
    load(&engine, "yard", &[("1", Some("wrench"), None)]);
    resolve(&engine);
    let wrenches = identity(&engine, "yard", "1").unwrap();

    // A reviewer merges the identity into another. The question is the
    // same, and is about the identity that stands for both now.
    let merged = decide(
        &engine,
        vec![Op::Merge {
            from: held.identity,
            into: wrenches,
        }],
    );
    let [now] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    assert_eq!(now.id, item.id);
    assert_eq!(now.identities, [wrenches]);
    assert_eq!(
        groups(&engine, now),
        names(&[&["depot:1"], &["shop:1"], &["yard:1"]])
    );
    let case = engine.review_case(item.id).unwrap();
    assert_eq!(case.candidates[0].identity.id, wrenches);
    assert!(
        case.records
            .iter()
            .all(|record| record.identity == Some(wrenches))
    );
    let taken: Vec<RecordId> = case.taken_back.iter().map(|record| record.id).collect();
    assert_eq!(taken, [held.bridge]);
    engine.verify().unwrap();

    // Undoing the merge puts the records back, and the question with them.
    engine.undo(merged.seq, reviewer(), None, true).unwrap();
    let [back] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    assert_eq!(back.id, item.id);
    assert_eq!(back.identities, [held.identity]);
    assert_eq!(groups(&engine, back), names(&[&["depot:1"], &["shop:1"]]));
    engine.verify().unwrap();

    // Merged again, the identity the item names is one that can be split.
    // Without the mallet, what is left of the side that was merged is the
    // anvil, and the merge says the anvil and the wrench are one thing.
    decide(
        &engine,
        vec![Op::Merge {
            from: held.identity,
            into: wrenches,
        }],
    );
    let named = open(&engine)[0].identities[0];
    let split = engine
        .split(named, &[held.mallet], true, None, reviewer(), None)
        .unwrap();
    assert_eq!(open(&engine), []);
    let closed = self::item(&engine, item.id);
    assert_eq!(closed.status, ReviewStatus::Resolved);
    assert_eq!(closed.resolved_seq, Some(split.committed.seq));
    assert_eq!(
        members(&engine, wrenches),
        ["shop:1", "yard:1"].map(str::to_owned).into()
    );
    engine.verify().unwrap();
}

#[test]
fn identities_that_are_merged_have_one_item_between_them() {
    let engine = engine();
    let held = held_by_one(&engine);
    // A second identity that one row holds together.
    load(&engine, "shop", &[("5", Some("pliers"), None)]);
    load(&engine, "depot", &[("5", Some("tongs"), Some("K5"))]);
    load(&engine, "kiosk", &[("5", Some("pliers"), Some("K5"))]);
    resolve(&engine);
    let pliers = identity(&engine, "shop", "5").unwrap();
    let taken = retract(&engine, "kiosk", &["1", "5"]);
    assert_eq!(taken.possible_splits, 2);
    let before = open(&engine);
    let of = |identity: IdentityId| {
        before
            .iter()
            .find(|item| item.identities == [identity])
            .expect("an item for the identity")
            .clone()
    };
    let (kept, other) = (of(held.identity), of(pliers));

    // A reviewer merges the two. One item asks about the identity that
    // stands, and the other is closed by the merge.
    let merged = decide(
        &engine,
        vec![Op::Merge {
            from: pliers,
            into: held.identity,
        }],
    );
    let [now] = &open(&engine)[..] else {
        panic!("one open review item, not {:?}", open(&engine));
    };
    assert_eq!(now.id, kept.id);
    assert_eq!(now.identities, [held.identity]);
    assert_eq!(
        groups(&engine, now),
        names(&[&["depot:1"], &["depot:5"], &["shop:1"], &["shop:5"]])
    );
    let closed = item(&engine, other.id);
    assert_eq!(closed.status, ReviewStatus::Resolved);
    assert_eq!(closed.resolved_seq, Some(merged.seq));
    engine.verify().unwrap();

    // Undoing the merge leaves two identities with the question, and each
    // has an item again.
    engine.undo(merged.seq, reviewer(), None, true).unwrap();
    let after = open(&engine);
    assert_eq!(after.len(), 2, "{after:?}");
    let of = |identity: IdentityId| {
        after
            .iter()
            .find(|item| item.identities == [identity])
            .expect("an item for the identity")
    };
    assert_eq!(of(held.identity).id, kept.id);
    assert_eq!(
        groups(&engine, of(held.identity)),
        names(&[&["depot:1"], &["shop:1"]])
    );
    assert_ne!(of(pliers).id, other.id, "a closed item stays closed");
    assert_eq!(of(pliers).run, taken.run);
    assert_eq!(
        groups(&engine, of(pliers)),
        names(&[&["depot:5"], &["shop:5"]])
    );
    engine.verify().unwrap();

    // Dismissing the one item of an identity leaves no other that asks the
    // same.
    decide(
        &engine,
        vec![Op::Merge {
            from: pliers,
            into: held.identity,
        }],
    );
    let [one] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    engine.dismiss_review(one.id, &reviewer(), None).unwrap();
    assert_eq!(open(&engine), []);
    engine.verify().unwrap();
}
