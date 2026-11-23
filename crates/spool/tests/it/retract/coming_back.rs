//! A row its source sends again after taking it back, and where it goes.

use super::*;

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
fn a_row_that_comes_back_as_it_was_before_it_changed_is_that_again() {
    let engine = engine();
    let anvil = three_anvils(&engine);
    let first = record_id(&engine, "shop", "1");
    load(&engine, "shop", &[("1", Some("anvil mk2"), None)]);
    resolve(&engine);
    retract(&engine, "shop", &["1"]);

    // The shop holds the row again, and it says what its first version
    // said. Both versions come back, and the row has a third, which is
    // what the shop holds now.
    let report = load(&engine, "shop", &[("1", Some("anvil"), None)]);
    assert_eq!(
        (report.inserted, report.superseded, report.restored),
        (1, 1, 2)
    );
    assert_eq!(report.returned, 2);
    let versions = engine.records(&RecordFilter::keyed("shop", "1")).unwrap();
    let [latest] = &versions
        .iter()
        .filter(|version| version.superseded_by.is_none())
        .collect::<Vec<_>>()[..]
    else {
        panic!("one version is the latest: {versions:?}");
    };
    assert_eq!(latest.payload["name"], "anvil");
    assert_ne!(latest.id, first);
    assert_eq!(latest.identity, None);

    resolve(&engine);
    let versions = records(&engine, "shop", "1");
    assert_eq!(versions.len(), 3);
    assert_eq!(engine.identities_of(&versions).unwrap(), [Some(anvil); 3]);
    engine.verify().unwrap();
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
