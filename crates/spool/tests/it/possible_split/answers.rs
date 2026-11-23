//! What answers a `possible_split` item, and what leaves it open.

use super::*;

#[test]
fn a_changeset_that_does_not_answer_the_item_leaves_it_open() {
    let engine = engine();
    let (held, item) = asked(&engine);

    // A run that places an unrelated record.
    load(&engine, "yard", &[("7", Some("wrench"), None)]);
    resolve(&engine);
    assert_eq!(open(&engine), std::slice::from_ref(&item));

    // A reviewer's changeset about other records.
    let wrench = record_id(&engine, "yard", "7");
    decide(
        &engine,
        vec![Op::Constrain {
            kind: ConstraintKind::CannotLink,
            a: wrench,
            b: held.anvil,
        }],
    );
    assert_eq!(open(&engine), std::slice::from_ref(&item));

    // A retraction that names none of its records.
    retract(&engine, "yard", &["7"]);
    assert_eq!(open(&engine), [item]);
    engine.verify().unwrap();
}

#[test]
fn splitting_the_identity_resolves_the_item() {
    let engine = engine();
    let (held, item) = asked(&engine);

    let split = engine
        .split(
            held.identity,
            &[held.mallet],
            true,
            None,
            reviewer(),
            Some("a mallet is not an anvil".to_owned()),
        )
        .unwrap();
    let closed = self::item(&engine, item.id);
    assert_eq!(closed.status, ReviewStatus::Resolved);
    assert_eq!(closed.resolved_seq, Some(split.committed.seq));
    // The item keeps what it asked.
    assert_eq!(closed.groups, item.groups);
    assert_eq!(open(&engine), []);
    assert_ne!(
        engine.identity_of(held.anvil).unwrap(),
        engine.identity_of(held.mallet).unwrap()
    );
    engine.verify().unwrap();
}

#[test]
fn saying_the_groups_are_one_thing_resolves_the_item() {
    let engine = engine();
    let (held, item) = asked(&engine);

    let said = decide(
        &engine,
        vec![Op::Constrain {
            kind: ConstraintKind::MustLink,
            a: held.anvil,
            b: held.mallet,
        }],
    );
    let closed = self::item(&engine, item.id);
    assert_eq!(closed.status, ReviewStatus::Resolved);
    assert_eq!(closed.resolved_seq, Some(said.seq));
    assert_eq!(members(&engine, held.identity).len(), 2);

    // The decision is evidence from then on: the question is not raised
    // again when another record comes and goes.
    load(&engine, "yard", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    assert_eq!(members(&engine, held.identity).len(), 3);
    assert_eq!(retract(&engine, "yard", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), []);
    engine.verify().unwrap();
}

#[test]
fn the_row_coming_back_resolves_the_item() {
    let engine = engine();
    let (held, item) = asked(&engine);

    let back = load(&engine, "kiosk", &[("1", Some("anvil"), Some("K2"))]);
    assert_eq!(back.returned, 1);
    assert_eq!(
        engine.identity_of(held.bridge).unwrap(),
        Some(held.identity)
    );
    let closed = self::item(&engine, item.id);
    assert_eq!(closed.status, ReviewStatus::Resolved);
    assert_eq!(closed.resolved_seq, back.committed.map(|done| done.seq));
    assert_eq!(resolve(&engine).resolved, []);
    engine.verify().unwrap();
}

#[test]
fn a_new_record_that_matches_both_groups_resolves_the_item() {
    let engine = engine();
    let (held, item) = asked(&engine);

    // The yard has an anvil with the code K2 as well.
    load(&engine, "yard", &[("1", Some("anvil"), Some("K2"))]);
    let report = resolve(&engine);
    assert_eq!(
        identity(&engine, "yard", "1"),
        Some(held.identity),
        "it matches members of one identity, so it joins it"
    );
    let closed = self::item(&engine, item.id);
    assert_eq!(closed.status, ReviewStatus::Resolved);
    assert_eq!(
        closed.resolved_seq,
        report.resolved[0].committed.as_ref().map(|done| done.seq)
    );
    engine.verify().unwrap();
}

#[test]
fn a_run_that_judges_the_groups_a_match_supersedes_the_item() {
    let engine = engine();
    let (held, item) = asked(&engine);

    // The project learns that an anvil and a mallet in one place are one
    // thing: a rule that matches every pair of things. A rematch judges the
    // two, and commits nothing, because both have an identity.
    let lenient = EntityTypeSource {
        block: vec![SqlFile::new("any", "true")],
        no_match: Vec::new(),
        match_: vec![SqlFile::new("any", "true")],
        ..thing()
    };
    engine.apply(&manifest_of(vec![lenient])).unwrap();
    let head = engine.head().unwrap();
    let report = rematch(&engine);
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(report.resolved[0].judgements.match_, 1);

    let closed = self::item(&engine, item.id);
    assert_eq!(closed.status, ReviewStatus::Superseded);
    assert_eq!(closed.superseded_by, Some(report.resolved[0].reconcile_run));
    assert_eq!(members(&engine, held.identity).len(), 2);
    engine.verify().unwrap();
}

#[test]
fn another_retraction_keeps_the_item_with_the_groups_that_are_left() {
    let engine = engine();
    let held = held_by_one(&engine);
    // A second anvil, which matches the first.
    load(&engine, "yard", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    assert_eq!(identity(&engine, "yard", "1"), Some(held.identity));

    let first = retract(&engine, "kiosk", &["1"]);
    assert_eq!(first.possible_splits, 1);
    let before = open(&engine).remove(0);
    assert_eq!(
        groups(&engine, &before),
        names(&[&["depot:1"], &["shop:1", "yard:1"]])
    );
    // The larger group is first.
    assert_eq!(before.groups[0].len(), 2);

    // The same retraction again changes nothing.
    assert_eq!(retract(&engine, "kiosk", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), std::slice::from_ref(&before));

    // The yard's anvil goes too. The question stands, about fewer records:
    // it is the same item, in step with the identity.
    let second = retract(&engine, "yard", &["1"]);
    assert_eq!(second.possible_splits, 0, "no new item was raised");
    let [after] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    assert_eq!((after.id, after.run), (before.id, first.run));
    assert_eq!(groups(&engine, after), names(&[&["depot:1"], &["shop:1"]]));
    let mut left = vec![held.anvil, held.mallet];
    left.sort();
    assert_eq!(after.records, left);
    engine.verify().unwrap();

    // When a whole group goes, nothing is left to ask.
    let third = retract(&engine, "depot", &["1"]);
    assert_eq!(third.possible_splits, 0);
    let closed = item(&engine, after.id);
    assert_eq!(closed.status, ReviewStatus::Resolved);
    assert_eq!(closed.resolved_seq, third.committed.map(|done| done.seq));
    assert_eq!(open(&engine), []);
    engine.verify().unwrap();
}

#[test]
fn a_run_that_is_still_taking_judgements_does_not_answer_the_item() {
    let engine = engine();
    let (held, item) = asked(&engine);

    // A caller judges everything again by hand, and says the anvil and the
    // mallet are a match. Until the run is reconciled that is not evidence:
    // the run may be given up, and the item is closed when it is reconciled.
    let thing = spool::core::EntityType::new("thing").unwrap();
    let run = engine
        .begin_run(
            &StrategyRef::external("by-hand", None),
            &Scope::rematch(thing),
        )
        .unwrap();
    let (anvil, mallet) = (held.anvil.to_string(), held.mallet.to_string());
    let (left, right) = match anvil < mallet {
        true => (anvil, mallet),
        false => (mallet, anvil),
    };
    let rows = Rows::from_text(
        &["left_id", "right_id", "verdict"],
        &[vec![
            Some(left.as_str()),
            Some(right.as_str()),
            Some("match"),
        ]],
    )
    .unwrap();
    assert_eq!(engine.submit_judgements(run, rows).unwrap(), 1);
    assert_eq!(open(&engine), std::slice::from_ref(&item));
    engine.verify().unwrap();

    // A retraction while the run is open does not count its judgements
    // either: the same question is not raised twice, and is not dropped.
    assert_eq!(retract(&engine, "kiosk", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), std::slice::from_ref(&item));

    let resolved = engine
        .reconcile_with(run, &Policy::default(), Disposition::Commit)
        .unwrap();
    let closed = self::item(&engine, item.id);
    assert_eq!(closed.status, ReviewStatus::Superseded);
    assert_eq!(closed.superseded_by, Some(resolved.reconcile_run));
    engine.verify().unwrap();

    // A run that is given up leaves the item as it was.
    let engine = self::engine();
    let (held, item) = asked(&engine);
    let thing = spool::core::EntityType::new("thing").unwrap();
    let run = engine
        .begin_run(
            &StrategyRef::external("by-hand", None),
            &Scope::rematch(thing),
        )
        .unwrap();
    let (anvil, mallet) = (held.anvil.to_string(), held.mallet.to_string());
    let (left, right) = match anvil < mallet {
        true => (anvil, mallet),
        false => (mallet, anvil),
    };
    let rows = Rows::from_text(
        &["left_id", "right_id", "verdict"],
        &[vec![
            Some(left.as_str()),
            Some(right.as_str()),
            Some("match"),
        ]],
    )
    .unwrap();
    engine.submit_judgements(run, rows).unwrap();
    engine.cancel_run(run).unwrap();
    assert_eq!(open(&engine), [item]);
    engine.verify().unwrap();
}
