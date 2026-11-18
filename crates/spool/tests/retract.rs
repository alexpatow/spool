//! Rows a source takes back: the record stays as evidence, leaves its
//! identity, is matched with nothing, and comes back when the source sends
//! the row again.

mod common;

use common::*;
use spool::core::{Actor, Op, RecordId};
use spool::{
    DraftFilter, DraftStatus, Engine, Error, RecordFilter, ResolveOptions, ResolveReport,
    RetractReport, ReviewFilter, ReviewStatus, RunKind, RunStatus,
};

fn resolve(engine: &Engine) -> ResolveReport {
    engine.resolve(&ResolveOptions::default()).unwrap()
}

fn feed() -> Actor {
    Actor::Agent("feed".to_owned())
}

fn retract(engine: &Engine, source: &str, keys: &[&str]) -> RetractReport {
    let keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
    engine
        .retract(source, "thing", &keys, feed(), None)
        .unwrap()
}

fn set(members: &[&str]) -> std::collections::BTreeSet<String> {
    members.iter().map(|member| (*member).to_owned()).collect()
}

/// The anvil as the shop, the depot and the kiosk know it: one identity.
fn three_anvils(engine: &Engine) -> spool::core::IdentityId {
    load(engine, "shop", &[("1", Some("anvil"), None)]);
    load(engine, "depot", &[("1", Some("anvil"), None)]);
    load(engine, "kiosk", &[("1", Some("anvil"), None)]);
    resolve(engine);
    identity(engine, "shop", "1").unwrap()
}

#[test]
fn a_row_its_source_took_back_leaves_its_identity_and_stays_as_evidence() {
    let engine = engine();
    let anvil = three_anvils(&engine);
    let record = record_id(&engine, "shop", "1");
    let head = engine.head().unwrap();

    let report = retract(&engine, "shop", &["1"]);
    assert_eq!(
        (
            report.received,
            report.unknown,
            report.retracted,
            report.detached
        ),
        (1, 0, 1, 1)
    );

    // The record is still there, as it was received, and says it was
    // taken back.
    let entry = engine.record(record).unwrap().unwrap();
    assert_eq!(entry.payload["name"], "anvil");
    assert_eq!(entry.retracted, Some(report.run));
    assert_eq!(entry.identity, None);
    assert_eq!(members(&engine, anvil), set(&["depot:1", "kiosk:1"]));

    // It left by a changeset like any other, under the name of whoever
    // asked, and the log shows what it was a member of.
    let committed = report.committed.clone().expect("a changeset detached it");
    assert_eq!(committed.seq.0, head.0 + 1);
    let log = engine.log().unwrap();
    let entry = log.last().unwrap();
    assert_eq!(entry.changeset.actor, feed());
    assert_eq!(
        entry.changeset.reason.as_deref(),
        Some("source `shop` took the rows back")
    );
    // A changeset names the reconcile run that drafted it, and none did.
    assert_eq!(entry.changeset.run, None);
    assert_eq!(
        entry.changeset.ops,
        [Op::Assign {
            record,
            from: Some(anvil),
            to: None
        }]
    );
    let history = engine.history(anvil).unwrap();
    assert_eq!(history.last().unwrap().seq, committed.seq);

    // The retraction is a run, and reads back as one.
    let run = engine.run(report.run).unwrap();
    assert_eq!(run.kind, RunKind::Retract);
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(run.retraction, Some(report));
    engine.verify().unwrap();
}

#[test]
fn a_row_that_was_taken_back_is_matched_with_nothing() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), Some("K1"))]);
    load(&engine, "depot", &[("1", Some("mallet"), Some("K1"))]);
    resolve(&engine);
    let shop = record_id(&engine, "shop", "1");
    retract(&engine, "shop", &["1"]);

    // Only the shop's row said that K1 is an anvil. An anvil that arrives
    // now is a thing of its own.
    load(&engine, "kiosk", &[("1", Some("anvil"), None)]);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].new_records, 1);
    assert_eq!(report.resolved[0].candidates, 0);
    assert_eq!(report.resolved[0].identities_created, 1);
    assert_ne!(
        identity(&engine, "kiosk", "1"),
        identity(&engine, "depot", "1")
    );

    // Matching everything again does not look at it either.
    let report = engine
        .resolve(&ResolveOptions {
            rematch: true,
            ..ResolveOptions::default()
        })
        .unwrap();
    assert_eq!(report.resolved[0].candidates, 0);
    assert_eq!(report.resolved[0].records_assigned, 0);
    assert_eq!(engine.identity_of(shop).unwrap(), None);
    assert_eq!(
        count(&engine, "SELECT count(*) FROM spool.features_thing"),
        2
    );
    assert_eq!(
        count(
            &engine,
            "SELECT count(*) FROM spool.entity_thing WHERE name = 'anvil'"
        ),
        1
    );
    engine.verify().unwrap();
}

#[test]
fn a_row_taken_back_before_it_had_an_identity_is_never_given_one() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    let report = retract(&engine, "shop", &["1"]);
    assert_eq!((report.retracted, report.detached), (1, 0));
    assert_eq!(report.committed, None);

    let report = resolve(&engine);
    assert_eq!(report.resolved[0].new_records, 1);
    assert_eq!(report.resolved[0].records_assigned, 1);
    assert_eq!(identity(&engine, "shop", "1"), None);
    assert!(identity(&engine, "shop", "2").is_some());

    // Nothing is left to do, so the next resolve starts no run.
    assert!(resolve(&engine).resolved.is_empty());
    engine.verify().unwrap();
}

#[test]
fn every_version_of_a_row_is_taken_back() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    load(&engine, "shop", &[("1", Some("anvil mk2"), None)]);
    load(&engine, "depot", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "depot", "1").unwrap();
    assert_eq!(engine.identity(anvil).unwrap().members.len(), 3);

    let report = retract(&engine, "shop", &["1"]);
    assert_eq!((report.retracted, report.detached), (2, 2));
    assert_eq!(members(&engine, anvil), set(&["depot:1"]));
    for version in engine.records(&RecordFilter::keyed("shop", "1")).unwrap() {
        assert_eq!(version.retracted, Some(report.run));
        assert_eq!(version.identity, None);
    }
    engine.verify().unwrap();
}

#[test]
fn taking_back_what_is_gone_or_was_never_there_changes_nothing() {
    let engine = engine();
    three_anvils(&engine);
    retract(&engine, "shop", &["1"]);
    let head = engine.head().unwrap();

    // The same key twice in one call is one row. The depot's row 1 is not
    // the shop's.
    let report = retract(&engine, "shop", &["1", "1", "7"]);
    assert_eq!(
        (
            report.received,
            report.unknown,
            report.retracted,
            report.detached
        ),
        (2, 1, 0, 0)
    );
    assert_eq!(report.committed, None);
    assert_eq!(engine.head().unwrap(), head);
    assert!(identity(&engine, "depot", "1").is_some());

    // A source or a type the project does not have is refused.
    let keys = ["1".to_owned()];
    let error = engine
        .retract("market", "thing", &keys, feed(), None)
        .unwrap_err();
    assert!(matches!(error, Error::Retract(_)), "{error}");
    let error = engine
        .retract("shop", "gadget", &keys, feed(), None)
        .unwrap_err();
    assert!(matches!(error, Error::UndeclaredType(_)), "{error}");
    let error = engine
        .retract("shop", "thing", &[], feed(), None)
        .unwrap_err();
    assert!(matches!(error, Error::Retract(_)), "{error}");
    engine.verify().unwrap();
}

#[test]
fn a_row_the_source_sends_again_comes_back() {
    let engine = engine();
    let anvil = three_anvils(&engine);
    let record = record_id(&engine, "shop", "1");
    retract(&engine, "shop", &["1"]);

    // The same content is the same record. It is not new, it is no longer
    // taken back, and it is in the identity it left: what put it there
    // still holds.
    let head = engine.head().unwrap();
    let report = load(&engine, "shop", &[("1", Some("anvil"), None)]);
    assert_eq!(
        (report.inserted, report.duplicates, report.restored),
        (0, 1, 1)
    );
    assert_eq!(report.returned, 1);
    let committed = report.committed.clone().expect("a changeset put it back");
    assert_eq!(committed.seq.0, head.0 + 1);
    let entry = engine.record(record).unwrap().unwrap();
    assert_eq!(entry.retracted, None);
    assert_eq!(entry.identity, Some(anvil));
    assert_eq!(
        members(&engine, anvil),
        set(&["shop:1", "depot:1", "kiosk:1"])
    );
    let log = engine.log().unwrap();
    let entry = log.last().unwrap();
    assert_eq!(entry.changeset.actor, Actor::Engine);
    assert_eq!(
        entry.changeset.reason.as_deref(),
        Some("source `shop` sent the rows again")
    );
    assert_eq!(
        entry.changeset.ops,
        [Op::Assign {
            record,
            from: None,
            to: Some(anvil)
        }]
    );
    // The run reads back with what it did.
    assert_eq!(engine.run(report.run).unwrap().load, Some(report));

    // It has an identity, so it is nothing for a run to look at.
    assert_eq!(resolve(&engine).resolved, []);

    // Both the retraction and its end are kept.
    assert_eq!(
        select(
            &engine,
            "SELECT retract_run::VARCHAR, restored_run::VARCHAR FROM spool.retraction"
        ),
        [[Some("6".to_owned()), Some("7".to_owned())]]
    );
    engine.verify().unwrap();
}

#[test]
fn a_row_that_comes_back_changed_brings_its_earlier_versions_with_it() {
    let engine = engine();
    let anvil = three_anvils(&engine);
    let first = record_id(&engine, "shop", "1");
    retract(&engine, "shop", &["1"]);

    let report = load(&engine, "shop", &[("1", Some("anvil mk2"), None)]);
    assert_eq!(
        (report.inserted, report.superseded, report.restored),
        (1, 1, 1)
    );
    assert_eq!(report.returned, 1);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].new_records, 1);

    // Nothing but the key says that mk2 is the anvil. The version before
    // it went back to its identity, and the key did the rest.
    let versions = records(&engine, "shop", "1");
    assert_eq!(versions.len(), 2);
    assert_eq!(
        engine.identities_of(&versions).unwrap(),
        [Some(anvil), Some(anvil)]
    );
    assert_eq!(engine.record(first).unwrap().unwrap().retracted, None);
    engine.verify().unwrap();
}

#[test]
fn a_record_that_was_taken_back_cannot_be_given_an_identity() {
    let engine = engine();
    let anvil = three_anvils(&engine);
    let record = record_id(&engine, "shop", "1");
    let report = retract(&engine, "shop", &["1"]);
    let refused = |error: Error| match error {
        Error::Retracted(named) => assert_eq!(named, record),
        other => panic!("expected a refusal that names the record, not: {other}"),
    };

    let back = vec![Op::Assign {
        record,
        from: None,
        to: Some(anvil),
    }];
    refused(
        engine
            .author(Actor::User("reviewer".to_owned()), None, back)
            .unwrap_err(),
    );
    // Undoing the changeset that detached it would put it back, and is
    // refused the same way.
    refused(
        engine
            .undo(
                report.committed.unwrap().seq,
                Actor::User("reviewer".to_owned()),
                None,
                true,
            )
            .unwrap_err(),
    );
    assert_eq!(engine.identity_of(record).unwrap(), None);
    engine.verify().unwrap();
}

#[test]
fn a_draft_that_places_a_record_is_closed_when_the_record_is_taken_back() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    let report = engine
        .resolve(&ResolveOptions {
            leave_for_review: true,
            ..ResolveOptions::default()
        })
        .unwrap();
    let draft = report.resolved[0].draft.expect("a draft was stored");

    let retraction = retract(&engine, "shop", &["1"]);
    let stored = engine.draft(draft).unwrap();
    assert_eq!(stored.draft.status, DraftStatus::Superseded);
    assert!(engine.drafts(&DraftFilter::open()).unwrap().is_empty());
    let error = engine.approve_draft(draft).unwrap_err();
    assert!(matches!(error, Error::Draft { .. }), "{error}");
    assert_eq!(retraction.detached, 0);

    // The run that drafted it never succeeded, so the row that is left is
    // still new to the next one.
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].new_records, 1);
    assert!(identity(&engine, "shop", "2").is_some());
    assert_eq!(identity(&engine, "shop", "1"), None);
    engine.verify().unwrap();
}

/// An anvil without a code and a hammer with the code K2, each an identity
/// of its own.
fn an_anvil_and_a_hammer(engine: &Engine) {
    load(
        engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("hammer"), Some("K2")),
        ],
    );
    resolve(engine);
}

#[test]
fn a_review_item_is_closed_when_the_record_it_asks_about_is_taken_back() {
    let engine = engine();
    an_anvil_and_a_hammer(&engine);
    // An anvil with the hammer's code: the rules cannot say which it is.
    load(&engine, "depot", &[("9", Some("anvil"), Some("K2"))]);
    resolve(&engine);
    let [item] = &engine.review_items(&ReviewFilter::open()).unwrap()[..] else {
        panic!("one open review item");
    };
    let id = item.id;

    let report = retract(&engine, "depot", &["9"]);
    assert!(
        engine
            .review_items(&ReviewFilter::open())
            .unwrap()
            .is_empty()
    );
    let closed = engine
        .review_items(&ReviewFilter {
            status: Some(ReviewStatus::Superseded),
            ..ReviewFilter::default()
        })
        .unwrap();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].id, id);
    assert_eq!(closed[0].superseded_by, Some(report.run));

    // Nothing is left to examine.
    assert!(resolve(&engine).resolved.is_empty());
    engine.verify().unwrap();

    // The row comes back, and the question with it.
    load(&engine, "depot", &[("9", Some("anvil"), Some("K2"))]);
    let report = resolve(&engine);
    assert_eq!(
        (
            report.resolved[0].new_records,
            report.resolved[0].reexamined
        ),
        (1, 0)
    );
    let open = engine.review_items(&ReviewFilter::open()).unwrap();
    assert_eq!(open.len(), 1);
    assert_ne!(open[0].id, id);
    engine.verify().unwrap();
}

#[test]
fn a_review_item_stays_open_for_the_records_that_were_not_taken_back() {
    let engine = engine();
    an_anvil_and_a_hammer(&engine);
    // Two rows the rules cannot place, held back together.
    load(&engine, "depot", &[("9", Some("anvil"), Some("K2"))]);
    load(&engine, "kiosk", &[("9", Some("anvil"), Some("K2"))]);
    resolve(&engine);
    let [item] = &engine.review_items(&ReviewFilter::open()).unwrap()[..] else {
        panic!("one open review item");
    };
    let id = item.id;
    assert_eq!(item.records.len(), 2);

    // The kiosk's row still waits for an identity.
    retract(&engine, "depot", &["9"]);
    let open = engine.review_items(&ReviewFilter::open()).unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, id);
    assert_eq!(
        engine
            .records(&RecordFilter {
                unassigned: true,
                ..RecordFilter::default()
            })
            .unwrap()
            .len(),
        1
    );
    engine.verify().unwrap();

    // A reviewer places it, and nothing is left to ask: the depot's row is
    // not waiting.
    let hammer = identity(&engine, "shop", "2").unwrap();
    let placed = engine
        .author(
            Actor::User("reviewer".to_owned()),
            None,
            vec![Op::Assign {
                record: record_id(&engine, "kiosk", "9"),
                from: None,
                to: Some(hammer),
            }],
        )
        .unwrap();
    let item = &engine
        .review_items(&ReviewFilter {
            id: Some(id),
            ..ReviewFilter::default()
        })
        .unwrap()[0];
    assert_eq!(item.status, ReviewStatus::Resolved);
    assert_eq!(item.resolved_seq, placed.committed.map(|done| done.seq));
    assert!(resolve(&engine).resolved.is_empty());
    engine.verify().unwrap();
}

#[test]
fn what_a_store_holds_says_how_much_was_taken_back() {
    let engine = engine();
    three_anvils(&engine);
    load(&engine, "shop", &[("2", Some("hammer"), None)]);
    retract(&engine, "shop", &["1"]);

    let info = engine.info().unwrap();
    let thing = &info.entity_types[0];
    assert_eq!(
        (thing.records, thing.unassigned, thing.retracted),
        (4, 1, 1)
    );
    let shop = thing
        .sources
        .iter()
        .find(|source| source.source == "shop")
        .unwrap();
    assert_eq!((shop.records, shop.unassigned, shop.retracted), (2, 1, 1));
}

#[test]
fn what_was_taken_back_stays_so_through_a_rebuild_a_changed_declaration_and_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.duckdb");
    let record: RecordId;
    {
        let engine = Engine::open(&path, spool::Options::seeded(1)).unwrap();
        engine.apply(&things()).unwrap();
        three_anvils(&engine);
        record = record_id(&engine, "shop", "1");
        retract(&engine, "shop", &["1"]);
        engine.rebuild_projection().unwrap();
        engine.verify().unwrap();

        // A changed declaration computes every feature row again, and has
        // none for a row that was taken back.
        let mut changed = thing();
        changed.entity = entity(
            Some("name"),
            &[
                ("name", "{ from: name, transform: upper }"),
                ("code", "code"),
            ],
            &SOURCES,
        );
        let mut project = project_of(vec![changed]);
        project.transforms = vec![spool::SqlFile::new("upper", "lambda value: upper(value)")];
        engine.apply(&project.compile().unwrap()).unwrap();
        assert_eq!(
            select(
                &engine,
                "SELECT name FROM spool.features_thing ORDER BY record_id"
            ),
            [[Some("ANVIL".to_owned())], [Some("ANVIL".to_owned())]]
        );
        engine.verify().unwrap();
    }
    let engine = Engine::open(&path, spool::Options::seeded(2)).unwrap();
    let entry = engine.record(record).unwrap().unwrap();
    assert!(entry.retracted.is_some());
    assert_eq!(entry.identity, None);
    engine.verify().unwrap();
}

#[test]
fn a_record_that_was_taken_back_is_still_explained() {
    let engine = engine();
    three_anvils(&engine);
    let shop = record_id(&engine, "shop", "1");
    let depot = record_id(&engine, "depot", "1");
    let taken = retract(&engine, "shop", &["1"]);

    // What a run recorded of it is still there to read, and the
    // explanation says why it has no identity all the same.
    let explanation = engine.explain(shop, depot).unwrap();
    assert_eq!(explanation.outcome, spool::Outcome::Unassigned);
    assert_eq!(explanation.left.record.retracted, Some(taken.run));
    assert_eq!(explanation.left.features, None);
    assert!(explanation.right.features.is_some());
    let depot_identity = identity(&engine, "depot", "1").unwrap();
    assert_eq!(
        explanation.summary[..2],
        [
            format!("`shop:1` has no identity. `depot:1` belongs to identity {depot_identity}."),
            format!(
                "`shop:1` was taken back by its source in run {}: it was taken out of its identity, and no run examines it until the source sends the row again.",
                taken.run
            ),
        ]
    );
    assert!(
        explanation.summary[2].contains("judged them `match`"),
        "{:?}",
        explanation.summary
    );
}

#[test]
fn a_row_that_was_alone_comes_back_to_the_identity_it_had() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();
    retract(&engine, "shop", &["1"]);
    assert_eq!(members(&engine, anvil), set(&[]));

    // Nothing is left to match it with, and it is the same thing it was.
    let report = load(&engine, "shop", &[("1", Some("anvil"), None)]);
    assert_eq!((report.restored, report.returned), (1, 1));
    assert_eq!(identity(&engine, "shop", "1"), Some(anvil));
    assert_eq!(resolve(&engine).resolved, []);
    assert_eq!(
        count(&engine, "SELECT count(*) FROM spool.identity_current"),
        1
    );
    engine.verify().unwrap();
}

#[test]
fn a_row_comes_back_to_the_identity_that_stands_for_the_one_it_left() {
    let engine = engine();
    let anvil = three_anvils(&engine);
    load(&engine, "shop", &[("2", Some("hammer"), None)]);
    resolve(&engine);
    let hammer = identity(&engine, "shop", "2").unwrap();
    retract(&engine, "shop", &["1"]);

    // While it is away, a reviewer says the anvils and the hammer are one
    // thing.
    engine
        .author(
            Actor::User("reviewer".to_owned()),
            None,
            vec![Op::Merge {
                from: anvil,
                into: hammer,
            }],
        )
        .unwrap();

    let report = load(&engine, "shop", &[("1", Some("anvil"), None)]);
    assert_eq!(report.returned, 1);
    assert_eq!(identity(&engine, "shop", "1"), Some(hammer));
    assert_eq!(
        members(&engine, hammer),
        set(&["shop:1", "shop:2", "depot:1", "kiosk:1"])
    );
    engine.verify().unwrap();
}

#[test]
fn a_row_a_reviewer_placed_comes_back_to_where_the_reviewer_put_it() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    load(&engine, "depot", &[("1", Some("mallet"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();
    let mallet = record_id(&engine, "depot", "1");
    let alone = identity(&engine, "depot", "1").unwrap();
    engine
        .author(
            Actor::User("reviewer".to_owned()),
            Some("the depot calls an anvil a mallet".to_owned()),
            vec![Op::Assign {
                record: mallet,
                from: Some(alone),
                to: Some(anvil),
            }],
        )
        .unwrap();

    // No rule would put the two together. The reviewer did, and that
    // holds when the row has been away.
    retract(&engine, "depot", &["1"]);
    let report = load(&engine, "depot", &[("1", Some("mallet"), None)]);
    assert_eq!(report.returned, 1);
    assert_eq!(identity(&engine, "depot", "1"), Some(anvil));
    assert_eq!(resolve(&engine).resolved, []);
    engine.verify().unwrap();
}

#[test]
fn a_row_that_cannot_go_back_where_it_was_is_new_to_the_next_run() {
    let engine = engine();
    let anvil = three_anvils(&engine);
    let shop = record_id(&engine, "shop", "1");
    let depot = record_id(&engine, "depot", "1");
    load(&engine, "shop", &[("2", Some("hammer"), None)]);
    resolve(&engine);
    let hammer = identity(&engine, "shop", "2").unwrap();
    retract(&engine, "shop", &["1", "2"]);

    // While they are away, a reviewer says that the shop's anvil is not
    // the depot's.
    engine
        .author(
            Actor::User("reviewer".to_owned()),
            None,
            vec![Op::Constrain {
                kind: spool::core::ConstraintKind::CannotLink,
                a: shop.min(depot),
                b: shop.max(depot),
            }],
        )
        .unwrap();

    // The hammer goes back, and the anvil does not: it waits for a run,
    // which leaves it apart from the others.
    let report = load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("hammer"), None)],
    );
    assert_eq!((report.restored, report.returned), (2, 1));
    assert_eq!(identity(&engine, "shop", "2"), Some(hammer));
    assert_eq!(identity(&engine, "shop", "1"), None);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].new_records, 1);
    assert_ne!(identity(&engine, "shop", "1"), Some(anvil));
    engine.verify().unwrap();
}

#[test]
fn a_row_that_had_no_identity_when_it_was_taken_back_is_given_none_when_it_comes_back() {
    let engine = engine();
    let anvil = three_anvils(&engine);
    let record = record_id(&engine, "shop", "1");
    // A reviewer took it out of its identity before the shop took it back.
    engine
        .author(
            Actor::User("reviewer".to_owned()),
            None,
            vec![Op::Assign {
                record,
                from: Some(anvil),
                to: None,
            }],
        )
        .unwrap();
    let taken = retract(&engine, "shop", &["1"]);
    assert_eq!((taken.retracted, taken.detached), (1, 0));

    let report = load(&engine, "shop", &[("1", Some("anvil"), None)]);
    assert_eq!((report.restored, report.returned), (1, 0));
    assert_eq!(report.committed, None);
    assert_eq!(identity(&engine, "shop", "1"), None);
    engine.verify().unwrap();
}

#[test]
fn a_row_that_arrived_while_another_was_away_meets_it_in_a_rematch() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), Some("K1"))]);
    load(&engine, "depot", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();
    retract(&engine, "shop", &["1"]);

    // Only the shop's row says that the kiosk's sledge is the anvil, and
    // the shop's row is away when the sledge arrives.
    load(&engine, "kiosk", &[("9", Some("sledge"), Some("K1"))]);
    resolve(&engine);
    let sledge = identity(&engine, "kiosk", "9").unwrap();
    assert_ne!(sledge, anvil);

    // The shop's row goes back where it was. Both have an identity now,
    // so no run places either again, and both stay as they are.
    let report = load(&engine, "shop", &[("1", Some("anvil"), Some("K1"))]);
    assert_eq!(report.returned, 1);
    assert_eq!(resolve(&engine).resolved, []);
    assert_eq!(identity(&engine, "kiosk", "9"), Some(sledge));

    // Matching everything again compares them, and lists the two
    // identities as a merge someone may want to make.
    let report = engine
        .resolve(&ResolveOptions {
            rematch: true,
            ..ResolveOptions::default()
        })
        .unwrap();
    let suggestions = report.resolved[0].suggestions.as_ref().unwrap();
    let mut pair = [anvil, sledge];
    pair.sort();
    assert_eq!(suggestions.merges.len(), 1);
    assert_eq!(suggestions.merges[0].identities, pair);
    engine.verify().unwrap();
}
