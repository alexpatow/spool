//! What is left of an identity when a source takes back the record that held
//! it together: the records stay where they are, and a `possible_split`
//! review item asks a reviewer whether they are one thing.

mod common;

use common::*;
use spool::core::{Actor, ConstraintKind, IdentityId, Op, RecordId, Verdict};
use spool::{
    Engine, EntityTypeSource, Error, Options, RecordFilter, ResolveOptions, ResolveReport,
    RetractReport, ReviewEntry, ReviewFilter, ReviewKind, ReviewStatus, SqlFile,
};

fn resolve(engine: &Engine) -> ResolveReport {
    engine.resolve(&ResolveOptions::default()).unwrap()
}

fn rematch(engine: &Engine) -> ResolveReport {
    engine
        .resolve(&ResolveOptions {
            rematch: true,
            ..ResolveOptions::default()
        })
        .unwrap()
}

fn feed() -> Actor {
    Actor::Agent("feed".to_owned())
}

fn reviewer() -> Actor {
    Actor::User("reviewer".to_owned())
}

fn retract(engine: &Engine, source: &str, keys: &[&str]) -> RetractReport {
    let keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
    engine
        .retract(source, "thing", &keys, feed(), None)
        .unwrap()
}

fn decide(engine: &Engine, ops: Vec<Op>) -> spool::core::Committed {
    engine
        .author(reviewer(), None, ops)
        .unwrap()
        .committed
        .expect("the changeset was committed")
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

/// The groups of an item as `source:key`, each in order, the groups in
/// order.
fn groups(engine: &Engine, item: &ReviewEntry) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = item
        .groups
        .iter()
        .map(|group| {
            let mut names: Vec<String> = group
                .iter()
                .map(|record| {
                    let entry = engine.record(*record).unwrap().unwrap();
                    format!("{}:{}", entry.source, entry.key.unwrap())
                })
                .collect();
            names.sort();
            names
        })
        .collect();
    groups.sort();
    groups
}

fn names(groups: &[&[&str]]) -> Vec<Vec<String>> {
    groups
        .iter()
        .map(|group| group.iter().map(|name| (*name).to_owned()).collect())
        .collect()
}

/// An anvil without a code, a mallet with the code K2, and the kiosk's row
/// that says they are one thing: an anvil with the code K2. The rules
/// match on the name or the code, so the three share an identity, and only
/// the kiosk's row holds the other two together.
struct Held {
    identity: IdentityId,
    anvil: RecordId,
    mallet: RecordId,
    bridge: RecordId,
}

fn held_by_one(engine: &Engine) -> Held {
    ingest(engine, "shop", &[("1", Some("anvil"), None)]);
    ingest(engine, "depot", &[("1", Some("mallet"), Some("K2"))]);
    ingest(engine, "kiosk", &[("1", Some("anvil"), Some("K2"))]);
    resolve(engine);
    let identity = identity(engine, "shop", "1").unwrap();
    assert_eq!(
        members(engine, identity).len(),
        3,
        "the three share an identity"
    );
    Held {
        identity,
        anvil: record_id(engine, "shop", "1"),
        mallet: record_id(engine, "depot", "1"),
        bridge: record_id(engine, "kiosk", "1"),
    }
}

/// The same, after the kiosk took its row back: the one open item.
fn asked(engine: &Engine) -> (Held, ReviewEntry) {
    let held = held_by_one(engine);
    let report = retract(engine, "kiosk", &["1"]);
    assert_eq!(report.possible_splits, 1);
    let [item] = &open(engine)[..] else {
        panic!("one open review item");
    };
    (held, item.clone())
}

#[test]
fn what_only_a_row_that_was_taken_back_held_together_is_put_to_a_reviewer() {
    let engine = engine();
    let held = held_by_one(&engine);
    assert_eq!(open(&engine), []);

    let report = retract(&engine, "kiosk", &["1"]);
    assert_eq!((report.detached, report.possible_splits), (1, 1));

    // Nothing was split: the two are where they were.
    assert_eq!(
        members(&engine, held.identity),
        ["depot:1", "shop:1"].map(str::to_owned).into()
    );
    // The question is kept, and says what it is about.
    let [item] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    assert_eq!(item.kind, ReviewKind::PossibleSplit);
    assert_eq!(item.run, report.run);
    assert_eq!(item.identities, [held.identity]);
    let mut records = [held.anvil, held.mallet];
    records.sort();
    assert_eq!(item.records, records);
    assert_eq!(groups(&engine, item), names(&[&["depot:1"], &["shop:1"]]));
    assert_eq!(engine.info().unwrap().entity_types[0].open_review_items, 1);
    // The run says so when it is read back.
    let run = engine.run(report.run).unwrap();
    assert_eq!(run.retraction, Some(report));
    engine.verify().unwrap();
}

#[test]
fn nothing_is_asked_while_what_is_left_holds_together() {
    // Three anvils match each other, so any two of them still do.
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    ingest(&engine, "depot", &[("1", Some("anvil"), None)]);
    ingest(&engine, "kiosk", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    assert_eq!(retract(&engine, "kiosk", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), []);

    // One record that is left is not a question either.
    assert_eq!(retract(&engine, "depot", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), []);
    engine.verify().unwrap();
}

#[test]
fn a_constraint_and_a_version_of_the_same_row_hold_records_together() {
    // A reviewer said the anvil and the mallet are one thing.
    let engine = engine();
    let held = held_by_one(&engine);
    decide(
        &engine,
        vec![Op::Constrain {
            kind: ConstraintKind::MustLink,
            a: held.anvil,
            b: held.mallet,
        }],
    );
    assert_eq!(retract(&engine, "kiosk", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), []);

    // The depot sent its row again with another name and no code. The new
    // version matches nothing, and is with the old one because it is the
    // same row.
    let engine = self::engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    ingest(&engine, "depot", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    ingest(&engine, "depot", &[("1", Some("mallet"), None)]);
    resolve(&engine);
    ingest(&engine, "kiosk", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvils = identity(&engine, "shop", "1").unwrap();
    assert_eq!(members(&engine, anvils).len(), 3);
    assert_eq!(
        engine
            .identities_of(&records(&engine, "depot", "1"))
            .unwrap(),
        [Some(anvils); 2]
    );
    assert_eq!(retract(&engine, "kiosk", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), []);
    engine.verify().unwrap();
}

#[test]
fn a_record_a_reviewer_placed_stays_with_the_members_that_were_there() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    ingest(&engine, "kiosk", &[("1", Some("anvil"), None)]);
    ingest(&engine, "depot", &[("1", Some("mallet"), None)]);
    resolve(&engine);
    let anvils = identity(&engine, "shop", "1").unwrap();
    let mallets = identity(&engine, "depot", "1").unwrap();
    assert_ne!(anvils, mallets);
    // No rule says so, and the reviewer knows better.
    let mallet = record_id(&engine, "depot", "1");
    decide(
        &engine,
        vec![Op::Assign {
            record: mallet,
            from: Some(mallets),
            to: Some(anvils),
        }],
    );

    // The decision was about the mallet and the identity, not about the
    // kiosk's row.
    assert_eq!(retract(&engine, "kiosk", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), []);
    assert_eq!(
        members(&engine, anvils),
        ["depot:1", "shop:1"].map(str::to_owned).into()
    );

    // That holds when the mallet itself goes and comes back: the engine
    // puts it back, and what put it there first still counts.
    ingest(&engine, "yard", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    retract(&engine, "depot", &["1"]);
    let back = ingest(&engine, "depot", &[("1", Some("mallet"), None)]);
    assert_eq!(back.returned, 1);
    assert_eq!(retract(&engine, "yard", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), []);
    engine.verify().unwrap();
}

#[test]
fn identities_a_reviewer_merged_stay_together_and_a_side_that_came_apart_is_asked_about() {
    let engine = engine();
    let held = held_by_one(&engine);
    ingest(
        &engine,
        "yard",
        &[("1", Some("wrench"), None), ("2", Some("wrench"), None)],
    );
    resolve(&engine);
    let wrenches = identity(&engine, "yard", "1").unwrap();
    decide(
        &engine,
        vec![Op::Merge {
            from: wrenches,
            into: held.identity,
        }],
    );
    assert_eq!(members(&engine, held.identity).len(), 5);

    // Both sides of the merge are whole when one wrench goes, so the merge
    // still says they are one thing.
    assert_eq!(retract(&engine, "yard", &["2"]).possible_splits, 0);
    assert_eq!(open(&engine), []);

    // Without the kiosk's row, the side of the anvil and the mallet is not
    // whole. The merge does not say which of the two the wrench is.
    assert_eq!(retract(&engine, "kiosk", &["1"]).possible_splits, 1);
    let [item] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    assert_eq!(
        groups(&engine, item),
        names(&[&["depot:1"], &["shop:1"], &["yard:1"]])
    );
    engine.verify().unwrap();
}

#[test]
fn undoing_a_merge_says_nothing_about_the_records_it_puts_back() {
    let engine = engine();
    let held = held_by_one(&engine);
    ingest(&engine, "yard", &[("1", Some("wrench"), None)]);
    resolve(&engine);
    let wrenches = identity(&engine, "yard", "1").unwrap();
    let merged = decide(
        &engine,
        vec![Op::Merge {
            from: held.identity,
            into: wrenches,
        }],
    );
    engine.undo(merged.seq, reviewer(), None, true).unwrap();
    assert_eq!(members(&engine, held.identity).len(), 3);

    // The undo placed the three by a reviewer's changeset. It put back what
    // was there, and did not decide that the anvil and the mallet are one.
    assert_eq!(retract(&engine, "kiosk", &["1"]).possible_splits, 1);
    assert_eq!(
        groups(&engine, &open(&engine)[0]),
        names(&[&["depot:1"], &["shop:1"]])
    );
    engine.verify().unwrap();
}

#[test]
fn the_item_makes_no_work_for_a_match_run() {
    let engine = engine();
    let (held, item) = asked(&engine);

    // Its records have an identity and wait for nothing.
    assert_eq!(resolve(&engine).resolved, []);
    assert_eq!(
        engine
            .records(&RecordFilter {
                unassigned: true,
                ..RecordFilter::default()
            })
            .unwrap(),
        []
    );

    // A run that has work does not examine them on the item's account.
    ingest(&engine, "yard", &[("7", Some("wrench"), None)]);
    let report = resolve(&engine);
    assert_eq!(
        (
            report.resolved[0].new_records,
            report.resolved[0].reexamined
        ),
        (1, 0)
    );
    assert_eq!(open(&engine), [item]);
    assert_eq!(members(&engine, held.identity).len(), 2);
    engine.verify().unwrap();
}

#[test]
fn a_changeset_that_does_not_answer_the_item_leaves_it_open() {
    let engine = engine();
    let (held, item) = asked(&engine);

    // A run that places an unrelated record.
    ingest(&engine, "yard", &[("7", Some("wrench"), None)]);
    resolve(&engine);
    assert_eq!(open(&engine), [item.clone()]);

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
    assert_eq!(open(&engine), [item.clone()]);

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
    ingest(&engine, "yard", &[("1", Some("anvil"), None)]);
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

    let back = ingest(&engine, "kiosk", &[("1", Some("anvil"), Some("K2"))]);
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
    ingest(&engine, "yard", &[("1", Some("anvil"), Some("K2"))]);
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
fn another_retraction_raises_the_item_again_in_the_groups_that_are_left() {
    let engine = engine();
    let held = held_by_one(&engine);
    // A second anvil, which matches the first.
    ingest(&engine, "yard", &[("1", Some("anvil"), None)]);
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
    assert_eq!(open(&engine), [before.clone()]);

    // The yard's anvil goes too. The question stands, about fewer records.
    let second = retract(&engine, "yard", &["1"]);
    assert_eq!(second.possible_splits, 1);
    let closed = item(&engine, before.id);
    assert_eq!(closed.status, ReviewStatus::Superseded);
    assert_eq!(closed.superseded_by, Some(second.run));
    let [after] = &open(&engine)[..] else {
        panic!("one open review item");
    };
    assert_eq!(after.run, second.run);
    assert_eq!(groups(&engine, after), names(&[&["depot:1"], &["shop:1"]]));

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
fn a_dismissed_item_stays_closed_and_does_not_keep_its_records_from_later_runs() {
    let engine = engine();
    let (held, item) = asked(&engine);

    let dismissed = engine
        .dismiss_review(item.id, &reviewer(), Some("one thing, two names"))
        .unwrap();
    assert_eq!(dismissed.status, ReviewStatus::Dismissed);
    assert_eq!(members(&engine, held.identity).len(), 2);
    assert_eq!(open(&engine), []);

    // The same retraction again does not ask again.
    assert_eq!(retract(&engine, "kiosk", &["1"]).possible_splits, 0);
    assert_eq!(open(&engine), []);

    // The records of a dismissed item of another kind are never examined
    // again. These were not waiting for anything, so when one of them later
    // has no identity, a run places it like any record.
    decide(
        &engine,
        vec![Op::Assign {
            record: held.mallet,
            from: Some(held.identity),
            to: None,
        }],
    );
    rematch(&engine);
    let placed = engine.identity_of(held.mallet).unwrap();
    assert!(placed.is_some(), "the mallet was examined and placed");
    assert_ne!(placed, Some(held.identity));
    engine.verify().unwrap();
}

#[test]
fn the_case_shows_the_groups_and_what_held_them_together() {
    let engine = engine();
    let (held, item) = asked(&engine);

    let case = engine.review_case(item.id).unwrap();
    assert_eq!(case.item, item);
    // The records are where they were, in the identity the item names.
    assert_eq!(case.records.len(), 2);
    assert!(
        case.records
            .iter()
            .all(|record| record.identity == Some(held.identity) && record.retracted.is_none())
    );
    let [candidate] = &case.candidates[..] else {
        panic!("one identity");
    };
    assert_eq!(candidate.identity.id, held.identity);
    assert_eq!(candidate.identity.members, 2);

    // What joined them: each matched the kiosk's row, which was taken back.
    let [taken] = &case.taken_back[..] else {
        panic!("one record taken back");
    };
    assert_eq!(taken.id, held.bridge);
    assert!(taken.retracted.is_some());
    let mut judged: Vec<(RecordId, RecordId, Verdict)> = case
        .judgements
        .iter()
        .map(|judgement| (judgement.record, judgement.other, judgement.verdict))
        .collect();
    judged.sort_by_key(|(record, _, _)| *record);
    let mut expected = vec![
        (held.anvil, held.bridge, Verdict::Match),
        (held.mallet, held.bridge, Verdict::Match),
    ];
    expected.sort_by_key(|(record, _, _)| *record);
    assert_eq!(judged, expected);
    assert!(
        case.judgements
            .iter()
            .all(|judgement| judgement.identity.is_none())
    );

    // A case of another kind has neither.
    let engine = self::engine();
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", None, Some("K7"))],
    );
    resolve(&engine);
    ingest(&engine, "depot", &[("9", Some("anvil"), Some("K7"))]);
    resolve(&engine);
    let other = engine.review_case(open(&engine)[0].id).unwrap();
    assert_eq!(other.item.groups, Vec::<Vec<RecordId>>::new());
    assert_eq!(other.taken_back, []);
}

#[test]
fn an_explanation_of_two_of_its_records_names_the_item() {
    let engine = engine();
    let (held, item) = asked(&engine);

    let explanation = engine.explain(held.anvil, held.mallet).unwrap();
    assert_eq!(explanation.review_items, [item.clone()]);
    let said = explanation.summary.join("\n");
    assert!(
        said.contains(&format!(
            "\nReview item {} (`possible_split`) is open and names `depot:1`, `shop:1`: they share an identity, and what held them together went through a record that its source took back. Nothing the store knows says they are one thing now. Whether to split the identity is left to a reviewer.\n",
            item.id
        )),
        "{said}"
    );
    // The same question in the summary's own words, a line earlier.
    assert!(said.contains(
        "No chain of matches, constraints or versions joins them within the identity as it is now."
    ));

    engine.dismiss_review(item.id, &reviewer(), None).unwrap();
    let said = engine
        .explain(held.anvil, held.mallet)
        .unwrap()
        .summary
        .join("\n");
    assert!(
        said.contains("was dismissed by `user:reviewer`: it asked whether to split their identity, and the reviewer left it as it is."),
        "{said}"
    );
    assert!(!said.contains("No run looks at"), "{said}");
}

#[test]
fn a_store_that_keeps_an_item_open_with_nothing_to_ask_does_not_verify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    let (resolved, dismissed) = {
        let engine = Engine::open(&path, Options::seeded(1)).unwrap();
        engine.apply(&things()).unwrap();
        let (held, item) = asked(&engine);
        engine.verify().unwrap();
        decide(
            &engine,
            vec![Op::Constrain {
                kind: ConstraintKind::MustLink,
                a: held.anvil,
                b: held.mallet,
            }],
        );
        // A second identity with the same question, which is dismissed.
        ingest(&engine, "shop", &[("5", Some("pliers"), None)]);
        ingest(&engine, "depot", &[("5", Some("tongs"), Some("K5"))]);
        ingest(&engine, "kiosk", &[("5", Some("pliers"), Some("K5"))]);
        resolve(&engine);
        assert_eq!(retract(&engine, "kiosk", &["5"]).possible_splits, 1);
        let other = open(&engine).remove(0);
        engine.dismiss_review(other.id, &reviewer(), None).unwrap();
        engine.verify().unwrap();
        (item.id, other.id)
    };
    let tamper = |sql: &str| {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!("ATTACH '{}' AS db; {sql}", path.display()))
            .unwrap();
    };
    let problems = || {
        let engine = Engine::open(&path, Options::seeded(2)).unwrap();
        match engine.verify() {
            Err(Error::Verify(problems)) => problems,
            other => panic!("expected problems, got {other:?}"),
        }
    };

    // Open again, though a constraint holds its groups together.
    tamper(&format!(
        "UPDATE db.spool.review_item SET status = 'open', closed_seq = NULL
         WHERE review_id = {resolved}"
    ));
    assert_eq!(
        problems(),
        ["1 open `possible_split` review items with nothing left to ask"]
    );
    tamper(&format!(
        "UPDATE db.spool.review_item SET status = 'resolved', closed_seq = 1
         WHERE review_id = {resolved}"
    ));
    Engine::open(&path, Options::seeded(2))
        .unwrap()
        .verify()
        .unwrap();

    // Open, and not in the form the kind has.
    tamper(&format!(
        "UPDATE db.spool.review_item
         SET status = 'open', closed_by = NULL, closed_at = NULL, groups = NULL
         WHERE review_id = {dismissed}"
    ));
    assert_eq!(
        problems(),
        [
            "1 `possible_split` review items that do not name one identity and two groups of its records, or review items of another kind that have groups",
            "1 open `possible_split` review items with nothing left to ask",
        ]
    );
}
