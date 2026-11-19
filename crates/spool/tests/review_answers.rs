//! What each answer to a `possible_merge` does, as the studio's review
//! screen says it before the answer is given: where the records go, what
//! becomes of the identities and their IDs, what is recorded, what later
//! runs do, and what undoing the answer puts back.

mod common;

use common::*;
use spool::core::{Actor, Changeset, Committed, IdentityId, Op, RecordId, Seq};
use spool::{
    ConstraintFilter, Engine, ResolveOptions, ReviewEntry, ReviewFilter, ReviewKind, ReviewStatus,
};

fn reviewer() -> Actor {
    Actor::User("reviewer".to_owned())
}

fn resolve(engine: &Engine) {
    engine.resolve(&ResolveOptions::default()).unwrap();
}

fn rematch(engine: &Engine) {
    engine
        .resolve(&ResolveOptions {
            rematch: true,
            ..ResolveOptions::default()
        })
        .unwrap();
}

fn open(engine: &Engine) -> Vec<ReviewEntry> {
    engine.review_items(&ReviewFilter::open()).unwrap()
}

fn item(engine: &Engine, id: u64) -> ReviewEntry {
    engine
        .review_items(&ReviewFilter {
            id: Some(id),
            ..ReviewFilter::default()
        })
        .unwrap()
        .remove(0)
}

fn commit(engine: &Engine, ops: Vec<Op>) -> Committed {
    engine
        .commit(Changeset {
            id: engine.mint_ids(0, 1).changesets[0],
            base: engine.head().unwrap(),
            actor: reviewer(),
            reason: Some("a reviewer decided".to_owned()),
            run: None,
            ops,
        })
        .unwrap()
}

/// Two anvils that share an identity, a hammer of its own, and the depot's
/// anvil with the hammer's code, which matches both and waits in a
/// `possible_merge`.
struct Asked {
    anvils: IdentityId,
    hammer: IdentityId,
    waiting: RecordId,
    item: ReviewEntry,
}

fn asked() -> (Engine, Asked) {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", None, Some("H1")),
        ],
    );
    resolve(&engine);
    load(&engine, "depot", &[("9", Some("anvil"), Some("H1"))]);
    resolve(&engine);
    let [item] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    assert_eq!(item.kind, ReviewKind::PossibleMerge);
    let asked = Asked {
        anvils: identity(&engine, "shop", "1").unwrap(),
        hammer: identity(&engine, "shop", "3").unwrap(),
        waiting: record_id(&engine, "depot", "9"),
        item: item.clone(),
    };
    assert_eq!(asked.item.records, [asked.waiting]);
    assert_eq!(engine.identity_of(asked.waiting).unwrap(), None);
    (engine, asked)
}

fn size(engine: &Engine, identity: IdentityId) -> usize {
    members(engine, identity).len()
}

fn constraints_on(engine: &Engine, record: RecordId) -> usize {
    engine
        .constraints(&ConstraintFilter {
            record: Some(record),
            ..ConstraintFilter::default()
        })
        .unwrap()
        .len()
}

#[test]
fn an_item_nobody_answers_is_asked_again_by_each_run_and_stays_as_it_is() {
    let (engine, asked) = asked();
    resolve(&engine);
    rematch(&engine);
    assert_eq!(open(&engine), std::slice::from_ref(&asked.item));
    assert_eq!(engine.identity_of(asked.waiting).unwrap(), None);
}

#[test]
fn placing_the_records_in_one_identity_changes_no_other_and_records_nothing_else() {
    let (engine, asked) = asked();
    let placed = commit(
        &engine,
        vec![Op::Assign {
            record: asked.waiting,
            from: None,
            to: Some(asked.anvils),
        }],
    );

    assert_eq!(
        engine.identity_of(asked.waiting).unwrap(),
        Some(asked.anvils)
    );
    assert_eq!(
        (size(&engine, asked.anvils), size(&engine, asked.hammer)),
        (3, 1)
    );
    // Both keep their IDs, and no constraint is recorded.
    assert_eq!(engine.current_id(asked.anvils).unwrap(), asked.anvils);
    assert_eq!(engine.current_id(asked.hammer).unwrap(), asked.hammer);
    assert_eq!(constraints_on(&engine, asked.waiting), 0);
    let closed = item(&engine, asked.item.id);
    assert_eq!(closed.status, ReviewStatus::Resolved);
    assert_eq!(closed.resolved_seq, Some(placed.seq));

    // No later run moves it, a rematch included.
    resolve(&engine);
    rematch(&engine);
    assert_eq!(
        engine.identity_of(asked.waiting).unwrap(),
        Some(asked.anvils)
    );
    assert_eq!(open(&engine), []);

    // Nothing stops the question being asked of another record.
    load(&engine, "yard", &[("4", Some("anvil"), Some("H1"))]);
    resolve(&engine);
    let [again] = &open(&engine)[..] else {
        panic!("a new item for the new record");
    };
    assert_eq!(again.records, [record_id(&engine, "yard", "4")]);
}

#[test]
fn undoing_a_placement_leaves_the_records_waiting_and_no_run_asks_about_them_again() {
    let (engine, asked) = asked();
    let placed = commit(
        &engine,
        vec![Op::Assign {
            record: asked.waiting,
            from: None,
            to: Some(asked.anvils),
        }],
    );
    engine.undo(placed.seq, reviewer(), None, true).unwrap();

    assert_eq!(engine.identity_of(asked.waiting).unwrap(), None);
    assert_eq!(item(&engine, asked.item.id).status, ReviewStatus::Resolved);
    resolve(&engine);
    assert_eq!(engine.identity_of(asked.waiting).unwrap(), None);
    assert_eq!(open(&engine), []);
    // A rematch examines every record, and asks about it again.
    rematch(&engine);
    let [again] = &open(&engine)[..] else {
        panic!("a new item after a rematch");
    };
    assert_ne!(again.id, asked.item.id);
    assert_eq!(again.records, [asked.waiting]);
}

#[test]
fn a_new_identity_takes_the_records_and_leaves_the_others_as_they_are() {
    let (engine, asked) = asked();
    let new = engine.mint_ids(1, 0).identities[0];
    commit(
        &engine,
        vec![
            Op::CreateIdentity {
                identity: new,
                entity_type: spool::core::EntityType::new("thing").unwrap(),
            },
            Op::Assign {
                record: asked.waiting,
                from: None,
                to: Some(new),
            },
        ],
    );
    assert_eq!(engine.identity_of(asked.waiting).unwrap(), Some(new));
    assert_eq!(
        (size(&engine, asked.anvils), size(&engine, asked.hammer)),
        (2, 1)
    );
    assert_eq!(item(&engine, asked.item.id).status, ReviewStatus::Resolved);
}

#[test]
fn merging_keeps_one_id_for_all_and_undoing_it_gives_each_back_its_own() {
    let (engine, asked) = asked();
    let merged = commit(
        &engine,
        vec![
            Op::Merge {
                from: asked.hammer,
                into: asked.anvils,
            },
            Op::Assign {
                record: asked.waiting,
                from: None,
                to: Some(asked.anvils),
            },
        ],
    );
    // The ID that was merged away answers with the one that stands.
    assert_eq!(engine.current_id(asked.hammer).unwrap(), asked.anvils);
    assert_eq!(engine.identity(asked.hammer).unwrap().current, asked.anvils);
    assert_eq!(size(&engine, asked.anvils), 4);
    assert_eq!(constraints_on(&engine, asked.waiting), 0);
    assert_eq!(item(&engine, asked.item.id).status, ReviewStatus::Resolved);

    engine.undo(merged.seq, reviewer(), None, true).unwrap();
    assert_eq!(engine.current_id(asked.hammer).unwrap(), asked.hammer);
    assert_eq!(
        (size(&engine, asked.anvils), size(&engine, asked.hammer)),
        (2, 1)
    );
    assert_eq!(engine.identity_of(asked.waiting).unwrap(), None);
    assert!(engine.head().unwrap() > Seq(merged.seq.0));
}

#[test]
fn a_dismissed_record_waits_and_no_run_looks_at_it_again() {
    let (engine, asked) = asked();
    engine
        .dismiss_review(asked.item.id, &reviewer(), Some("not ours"))
        .unwrap();
    assert_eq!(item(&engine, asked.item.id).status, ReviewStatus::Dismissed);

    resolve(&engine);
    rematch(&engine);
    assert_eq!(engine.identity_of(asked.waiting).unwrap(), None);
    assert_eq!(open(&engine), []);

    // Another record like it is asked about, and the dismissed one is not.
    load(&engine, "yard", &[("4", Some("anvil"), Some("H1"))]);
    resolve(&engine);
    let [again] = &open(&engine)[..] else {
        panic!("a new item for the new record");
    };
    assert_eq!(again.records, [record_id(&engine, "yard", "4")]);

    // A changeset can still place it.
    commit(
        &engine,
        vec![Op::Assign {
            record: asked.waiting,
            from: None,
            to: Some(asked.hammer),
        }],
    );
    assert_eq!(
        engine.identity_of(asked.waiting).unwrap(),
        Some(asked.hammer)
    );
}

/// An anvil and a mallet that share an identity only through the kiosk's
/// row, which its source then takes back: a `possible_split` of two groups.
fn split_asked() -> (Engine, IdentityId, RecordId, RecordId, ReviewEntry) {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    load(&engine, "depot", &[("1", Some("mallet"), Some("K2"))]);
    load(&engine, "kiosk", &[("1", Some("anvil"), Some("K2"))]);
    resolve(&engine);
    let identity = identity(&engine, "shop", "1").unwrap();
    engine
        .retract("kiosk", "thing", &["1".to_owned()], reviewer(), None)
        .unwrap();
    let [item] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    assert_eq!(item.kind, ReviewKind::PossibleSplit);
    let (anvil, mallet) = (
        record_id(&engine, "shop", "1"),
        record_id(&engine, "depot", "1"),
    );
    (engine, identity, anvil, mallet, item.clone())
}

#[test]
fn a_split_keeps_the_id_for_the_larger_side_and_undoing_it_leaves_the_new_id_empty() {
    let (engine, identity, anvil, mallet, item) = split_asked();
    let split = engine
        .split(identity, &[mallet], true, None, reviewer(), None)
        .unwrap();
    assert_eq!(split.kept, identity);
    assert_eq!(engine.identity_of(anvil).unwrap(), Some(identity));
    assert_eq!(engine.identity_of(mallet).unwrap(), Some(split.identity));
    // Keeping them apart is one `cannot_link` between the sides.
    assert_eq!(constraints_on(&engine, mallet), 1);
    assert_eq!(self::item(&engine, item.id).status, ReviewStatus::Resolved);

    engine
        .undo(split.committed.seq, reviewer(), None, true)
        .unwrap();
    assert_eq!(engine.identity_of(mallet).unwrap(), Some(identity));
    assert_eq!(constraints_on(&engine, mallet), 0);
    // Creating an identity is not undone: its ID stays, with no records.
    assert_eq!(size(&engine, split.identity), 0);
    // A person put the records back, which holds them together: nothing is asked.
    assert_eq!(open(&engine), []);
}

#[test]
fn keeping_the_groups_as_one_records_a_must_link_and_undoing_it_asks_nothing_again() {
    let (engine, identity, anvil, mallet, item) = split_asked();
    let kept = commit(
        &engine,
        vec![Op::Constrain {
            kind: spool::core::ConstraintKind::MustLink,
            a: anvil,
            b: mallet,
        }],
    );
    assert_eq!(self::item(&engine, item.id).status, ReviewStatus::Resolved);
    assert_eq!(size(&engine, identity), 2);
    engine.undo(kept.seq, reviewer(), None, true).unwrap();
    assert_eq!(constraints_on(&engine, mallet), 0);
    assert_eq!(open(&engine), []);
}

#[test]
fn keeping_the_groups_as_one_holds_while_the_records_the_must_link_names_stay() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    load(&engine, "depot", &[("1", Some("mallet"), Some("K2"))]);
    load(&engine, "kiosk", &[("1", Some("anvil"), Some("K2"))]);
    resolve(&engine);
    engine
        .retract("kiosk", "thing", &["1".to_owned()], reviewer(), None)
        .unwrap();
    let [item] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    // The studio links the first record of one group with one of each other group.
    commit(
        &engine,
        vec![Op::Constrain {
            kind: spool::core::ConstraintKind::MustLink,
            a: record_id(&engine, "shop", "1"),
            b: record_id(&engine, "depot", "1"),
        }],
    );
    assert_eq!(self::item(&engine, item.id).status, ReviewStatus::Resolved);
    assert_eq!(open(&engine), []);

    // When a record the must_link names is taken back, what is left of its
    // group is held to the other by nothing, and the question is asked again.
    let report = engine
        .retract("shop", "thing", &["1".to_owned()], reviewer(), None)
        .unwrap();
    assert_eq!(report.possible_splits, 1);
    assert_eq!(open(&engine).len(), 1);
}

#[test]
fn a_merge_is_refused_after_its_identities_changed_and_a_placement_only_if_its_record_was_placed() {
    let (engine, asked) = asked();
    let read = engine.head().unwrap();
    // Someone else gives the anvils another record after the screen was read.
    load(&engine, "yard", &[("8", Some("hammer"), None)]);
    let stranger = record_id(&engine, "yard", "8");
    commit(
        &engine,
        vec![Op::Assign {
            record: stranger,
            from: None,
            to: Some(asked.anvils),
        }],
    );
    let at = |ops: Vec<Op>| {
        engine.commit(Changeset {
            id: engine.mint_ids(0, 1).changesets[0],
            base: read,
            actor: reviewer(),
            reason: None,
            run: None,
            ops,
        })
    };
    // A merge written against what was read is refused.
    assert!(
        at(vec![Op::Merge {
            from: asked.hammer,
            into: asked.anvils,
        }])
        .is_err()
    );
    // A placement in the identity that changed is made all the same.
    at(vec![Op::Assign {
        record: asked.waiting,
        from: None,
        to: Some(asked.anvils),
    }])
    .unwrap();
    // And one of a record that has an identity by then is refused.
    assert!(
        at(vec![Op::Assign {
            record: asked.waiting,
            from: None,
            to: Some(asked.hammer),
        }])
        .is_err()
    );
}
