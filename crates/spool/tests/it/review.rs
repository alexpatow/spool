//! Review items: kept in the store, and examined again by later runs.

use spool::core::{Actor, Changeset, IdGenerator, IdentityId, Op, ReviewItem, SeededIds};
use spool::{Engine, Options, ResolveOptions, ResolveReport};
use spool_testing::*;

fn resolve(engine: &Engine) -> ResolveReport {
    engine.resolve(&ResolveOptions::default()).unwrap()
}

/// One row of `spool.review_item`, with lists joined by commas.
#[derive(Debug, PartialEq, Eq)]
struct Stored {
    review_id: String,
    run_id: String,
    kind: String,
    records: String,
    identities: String,
    status: String,
    closed_seq: Option<String>,
    closed_run: Option<String>,
}

fn stored(engine: &Engine) -> Vec<Stored> {
    select(
        engine,
        "SELECT review_id::VARCHAR, run_id::VARCHAR, kind,
                array_to_string(record_ids::VARCHAR[], ','),
                array_to_string(identity_ids::VARCHAR[], ','),
                status, closed_seq::VARCHAR, closed_run::VARCHAR
         FROM spool.review_item ORDER BY review_id",
    )
    .into_iter()
    .map(|mut row| Stored {
        closed_run: row.pop().unwrap(),
        closed_seq: row.pop().unwrap(),
        status: row.pop().unwrap().unwrap(),
        identities: row.pop().unwrap().unwrap(),
        records: row.pop().unwrap().unwrap(),
        kind: row.pop().unwrap().unwrap(),
        run_id: row.pop().unwrap().unwrap(),
        review_id: row.pop().unwrap().unwrap(),
    })
    .collect()
}

fn open_reviews(engine: &Engine) -> u64 {
    count(engine, "SELECT count(*) FROM spool.review_open")
}

fn joined(mut ids: Vec<String>) -> String {
    ids.sort();
    ids.join(",")
}

/// Two identities, `by_name` and `by_code`, and a record `depot:9`
/// that matches both and is left for review.
fn ambiguous(engine: &Engine) -> (IdentityId, IdentityId) {
    load(
        engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", None, Some("K7"))],
    );
    resolve(engine);
    load(engine, "depot", &[("9", Some("anvil"), Some("K7"))]);
    resolve(engine);
    (
        identity(engine, "shop", "1").unwrap(),
        identity(engine, "shop", "2").unwrap(),
    )
}

fn commit(engine: &Engine, seed: u64, ops: Vec<Op>) {
    engine
        .commit(Changeset {
            id: SeededIds::new(seed).changeset_id(),
            base: engine.head().unwrap(),
            actor: Actor::User("reviewer".to_owned()),
            reason: Some("reviewed".to_owned()),
            run: None,
            ops,
        })
        .unwrap();
}

#[test]
fn a_review_item_is_stored_and_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    let expected = {
        let engine = Engine::open(&path, Options::seeded(1)).unwrap();
        engine.apply(&things()).unwrap();
        let (by_name, by_code) = ambiguous(&engine);

        let items = stored(&engine);
        let [item] = &items[..] else {
            panic!("one review item is stored: {items:?}");
        };
        assert_eq!(item.review_id, "1");
        assert_eq!(item.kind, "possible_merge");
        assert_eq!(item.records, record_id(&engine, "depot", "9").to_string());
        assert_eq!(
            item.identities,
            joined(vec![by_name.to_string(), by_code.to_string()])
        );
        assert_eq!(item.status, "open");
        assert_eq!((&item.closed_seq, &item.closed_run), (&None, &None));
        // It names the reconcile run that raised it.
        assert_eq!(
            select(
                &engine,
                &format!("SELECT kind FROM spool.run WHERE run_id = {}", item.run_id)
            ),
            [[Some("reconcile".to_owned())]]
        );
        items
    };

    let engine = Engine::open(&path, Options::seeded(2)).unwrap();
    assert_eq!(stored(&engine), expected);
    assert_eq!(open_reviews(&engine), 1);

    // It is still acted on: the next run looks at the record again.
    engine.apply(&things()).unwrap();
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].reexamined, 1);
    assert_eq!(stored(&engine), expected);
    engine.verify().unwrap();
}

#[test]
fn an_record_left_for_review_is_examined_again_by_later_runs() {
    let engine = engine();
    let (by_name, by_code) = ambiguous(&engine);
    let waiting = record_id(&engine, "depot", "9");
    let first = stored(&engine);

    // A later batch has nothing to say about it. The run reaches the same
    // conclusion, and the item stays as it was.
    load(&engine, "shop", &[("3", Some("chisel"), None)]);
    let report = resolve(&engine);
    let resolved = &report.resolved[0];
    assert_eq!((resolved.new_records, resolved.reexamined), (2, 1));
    assert_eq!(resolved.identities_created, 1);
    assert_eq!(resolved.records_unassigned, 1);
    assert!(matches!(
        &resolved.review[..],
        [ReviewItem::PossibleMerge { records, .. }] if records == &[waiting]
    ));
    assert_eq!(stored(&engine), first);

    // A reviewer decides the two identities are one thing and moves the
    // member of one into the other. The item stays open: the record it
    // names still has no identity.
    commit(
        &engine,
        50,
        vec![Op::Assign {
            record: record_id(&engine, "shop", "2"),
            from: Some(by_code),
            to: Some(by_name),
        }],
    );
    assert_eq!(open_reviews(&engine), 1);

    // Nothing new was loaded, and the next run still examines it. It now
    // matches one identity, so it is assigned and the item is resolved.
    let report = resolve(&engine);
    let resolved = &report.resolved[0];
    assert_eq!((resolved.new_records, resolved.reexamined), (1, 1));
    assert_eq!(resolved.records_assigned, 1);
    assert!(resolved.review.is_empty());
    assert_eq!(engine.identity_of(waiting).unwrap(), Some(by_name));

    let head = engine.head().unwrap();
    let after = stored(&engine);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].status, "resolved");
    assert_eq!(after[0].closed_seq, Some(head.to_string()));
    assert_eq!(after[0].run_id, first[0].run_id);
    assert_eq!(open_reviews(&engine), 0);

    // With nothing loaded and nothing under review, there is nothing to do.
    assert!(resolve(&engine).resolved.is_empty());
    assert_eq!(engine.head().unwrap(), head);
    engine.verify().unwrap();
}

#[test]
fn a_changeset_that_places_the_record_resolves_its_review_item() {
    let engine = engine();
    let (by_name, _) = ambiguous(&engine);
    let waiting = record_id(&engine, "depot", "9");

    commit(
        &engine,
        50,
        vec![Op::Assign {
            record: waiting,
            from: None,
            to: Some(by_name),
        }],
    );

    let items = stored(&engine);
    assert_eq!(items[0].status, "resolved");
    assert_eq!(
        items[0].closed_seq,
        Some(engine.head().unwrap().to_string())
    );
    assert_eq!(
        select(
            &engine,
            &format!(
                "SELECT actor FROM spool.changeset WHERE seq = {}",
                items[0].closed_seq.as_deref().unwrap()
            )
        ),
        [[Some("user:reviewer".to_owned())]]
    );
    assert!(resolve(&engine).resolved.is_empty());
    engine.verify().unwrap();
}

#[test]
fn an_item_raised_again_in_another_form_supersedes_the_old_one() {
    let engine = engine();
    ambiguous(&engine);
    let waiting = record_id(&engine, "depot", "9");

    // A second record that matches the first, and the same two identities.
    load(&engine, "yard", &[("4", Some("anvil"), Some("K7"))]);
    let report = resolve(&engine);
    let also_waiting = record_id(&engine, "yard", "4");
    assert_eq!(report.resolved[0].records_unassigned, 2);

    let items = stored(&engine);
    let [old, new] = &items[..] else {
        panic!("two review items are stored: {items:?}");
    };
    assert_eq!(old.status, "superseded");
    assert_eq!(old.closed_run.as_ref(), Some(&new.run_id));
    assert_eq!(old.closed_seq, None);
    assert_eq!(new.status, "open");
    assert_eq!(new.kind, "possible_merge");
    assert_eq!(
        new.records,
        joined(vec![waiting.to_string(), also_waiting.to_string()])
    );
    assert_eq!(new.identities, old.identities);
    assert_eq!(open_reviews(&engine), 1);

    // Placing one of the two does not close the item. The next run raises
    // it again for the one that is left.
    commit(
        &engine,
        50,
        vec![Op::Assign {
            record: waiting,
            from: None,
            to: Some(identity(&engine, "shop", "1").unwrap()),
        }],
    );
    assert_eq!(open_reviews(&engine), 1);
    resolve(&engine);
    let items = stored(&engine);
    assert_eq!(
        items
            .iter()
            .map(|item| item.status.as_str())
            .collect::<Vec<_>>(),
        ["superseded", "superseded", "open"]
    );
    assert_eq!(items[2].records, also_waiting.to_string());
    engine.verify().unwrap();
}

#[test]
fn two_new_records_a_rule_keeps_apart_do_not_join_the_same_identity() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();

    // Both match shop:1 by name, which has no code. Their codes differ, so
    // the `different_code` rule says they are not the same thing.
    load(
        &engine,
        "depot",
        &[
            ("1", Some("anvil"), Some("A")),
            ("2", Some("anvil"), Some("B")),
        ],
    );
    let report = resolve(&engine);
    let (first, second) = (
        record_id(&engine, "depot", "1"),
        record_id(&engine, "depot", "2"),
    );
    assert_eq!(
        count(
            &engine,
            &format!(
                "SELECT count(*) FROM spool.judgement
                 WHERE verdict = 'no_match'
                   AND left_id = least('{first}'::UUID, '{second}'::UUID)
                   AND right_id = greatest('{first}'::UUID, '{second}'::UUID)"
            )
        ),
        1
    );

    // Neither is attached. One item names both and the identity they match.
    assert_eq!(members(&engine, anvil), ["shop:1".to_owned()].into());
    assert_eq!(engine.identity_of(first).unwrap(), None);
    assert_eq!(engine.identity_of(second).unwrap(), None);
    let resolved = &report.resolved[0];
    assert_eq!(resolved.records_assigned, 0);
    assert_eq!(resolved.records_unassigned, 2);
    let items = stored(&engine);
    let [item] = &items[..] else {
        panic!("one review item is stored: {items:?}");
    };
    assert_eq!(item.kind, "conflict");
    assert_eq!(
        item.records,
        joined(vec![first.to_string(), second.to_string()])
    );
    assert_eq!(item.identities, anvil.to_string());
    assert_eq!(item.status, "open");

    // A reviewer places one of them. The other still cannot follow it.
    commit(
        &engine,
        50,
        vec![Op::Assign {
            record: first,
            from: None,
            to: Some(anvil),
        }],
    );
    resolve(&engine);
    assert_eq!(engine.identity_of(second).unwrap(), None);
    let items = stored(&engine);
    assert_eq!(
        items
            .iter()
            .map(|item| item.status.as_str())
            .collect::<Vec<_>>(),
        ["superseded", "open"]
    );
    assert_eq!(items[1].kind, "conflict");
    assert_eq!(items[1].records, second.to_string());
    assert_eq!(items[1].identities, anvil.to_string());
    engine.verify().unwrap();
}

#[test]
fn a_conflict_is_stored_with_the_identity_it_concerns() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), Some("A")), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    // Matches shop:2 by name, and cannot be shop:1 because the codes differ.
    load(&engine, "depot", &[("9", Some("anvil"), Some("B"))]);
    resolve(&engine);

    let items = stored(&engine);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, "conflict");
    assert_eq!(
        items[0].records,
        record_id(&engine, "depot", "9").to_string()
    );
    assert_eq!(
        items[0].identities,
        identity(&engine, "shop", "1").unwrap().to_string()
    );
}

#[test]
fn a_draft_that_is_not_committed_still_records_what_it_left_out() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", None, Some("K7"))],
    );
    resolve(&engine);
    load(&engine, "depot", &[("9", Some("anvil"), Some("K7"))]);

    let scope = spool::Scope::new(spool::core::EntityType::new("thing").unwrap());
    let run = engine
        .begin_run(&spool::StrategyRef::rules(), &scope)
        .unwrap();
    let id = |source: &str, key: &str| record_id(&engine, source, key).to_string();
    let rows: Vec<Vec<Option<String>>> = [("shop", "1"), ("shop", "2")]
        .iter()
        .map(|(source, key)| {
            vec![
                Some(id("depot", "9")),
                Some(id(source, key)),
                Some("match".to_owned()),
            ]
        })
        .collect();
    let rows: Vec<Vec<Option<&str>>> = rows
        .iter()
        .map(|row| row.iter().map(|cell| cell.as_deref()).collect())
        .collect();
    engine
        .submit_judgements(
            run,
            spool::Rows::from_text(&["left_id", "right_id", "verdict"], &rows).unwrap(),
        )
        .unwrap();

    let policy = spool::core::Policy::default();
    let draft = engine.reconcile(run, &policy).unwrap();
    assert!(draft.changeset.ops.is_empty());
    assert_eq!(draft.review.len(), 1);
    assert_eq!(open_reviews(&engine), 1);

    // There was nothing to commit, so the run is finished, and it cannot be
    // reconciled a second time.
    assert_eq!(engine.run(run).unwrap().status, spool::RunStatus::Succeeded);
    let error = engine.reconcile(run, &policy).unwrap_err();
    assert!(matches!(error, spool::Error::Run { .. }), "{error}");
    assert_eq!(stored(&engine).len(), 1);
}
