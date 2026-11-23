//! Rows that change at their source, and the versions they leave.

use super::*;

#[test]
fn a_changed_row_supersedes_the_one_before_and_keeps_its_identity() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), Some("K1"))]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();
    let first = record_id(&engine, "shop", "1");

    // The shop renames the row and gives it another code. No rule connects
    // the new version with the old one: the row's key does.
    let report = load(&engine, "shop", &[("1", Some("hammer"), Some("K2"))]);
    assert_eq!((report.inserted, report.superseded), (1, 1));
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].candidates, 0);
    assert_eq!(report.resolved[0].identities_created, 0);

    let versions = engine.records(&RecordFilter::keyed("shop", "1")).unwrap();
    assert_eq!(versions.len(), 2);
    let second = versions.iter().find(|version| version.id != first).unwrap();
    assert_eq!(second.supersedes, Some(first));
    assert_eq!(second.identity, Some(anvil));
    let old = engine.record(first).unwrap().unwrap();
    assert_eq!((old.supersedes, old.superseded_by), (None, Some(second.id)));
    // Both versions are evidence about the identity, and both stay.
    assert_eq!(engine.identity(anvil).unwrap().members.len(), 2);

    // The same content again is the same record, and nothing new.
    let report = load(&engine, "shop", &[("1", Some("hammer"), Some("K2"))]);
    assert_eq!((report.inserted, report.superseded), (0, 0));
    engine.verify().unwrap();
}

#[test]
fn versions_sent_in_one_call_follow_each_other_in_row_order() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    let report = load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil mk2"), None),
            ("7", Some("tongs"), None),
            ("1", Some("anvil mk3"), None),
        ],
    );
    assert_eq!((report.inserted, report.superseded), (3, 2));
    // The same key at another source is another row.
    let report = load(&engine, "depot", &[("1", Some("bellows"), None)]);
    assert_eq!(report.superseded, 0);

    let chain = select(
        &engine,
        "SELECT o.payload->>'name', p.payload->>'name'
         FROM spool.record o
         LEFT JOIN spool.record p ON p.record_id = o.supersedes
         ORDER BY o.source_id, o.source_key, o.payload->>'name'",
    );
    let chain: Vec<(&str, Option<&str>)> = chain
        .iter()
        .map(|row| (row[0].as_deref().unwrap(), row[1].as_deref()))
        .collect();
    assert_eq!(
        chain,
        [
            ("bellows", None),
            ("anvil", None),
            ("anvil mk2", Some("anvil")),
            ("anvil mk3", Some("anvil mk2")),
            ("tongs", None),
        ]
    );

    // All three versions were new to the run, and they are one thing.
    resolve(&engine);
    let versions = records(&engine, "shop", "1");
    let identities = engine.identities_of(&versions).unwrap();
    assert!(identities[0].is_some());
    assert!(identities.iter().all(|found| *found == identities[0]));
    engine.verify().unwrap();
}

#[test]
fn a_row_that_changes_back_is_what_it_was_again_and_keeps_its_identity() {
    let engine = engine();
    let anvil = [("1", Some("anvil"), Some("K1"))];
    let hammer = [("1", Some("hammer"), Some("K2"))];
    load(&engine, "shop", &anvil);
    resolve(&engine);
    let identity = identity(&engine, "shop", "1").unwrap();
    let first = record_id(&engine, "shop", "1");
    load(&engine, "shop", &hammer);
    resolve(&engine);
    let second = engine
        .record(first)
        .unwrap()
        .unwrap()
        .superseded_by
        .unwrap();

    // The shop has the row as it was at first. Its content is that of a
    // record the store holds, and the row is that record's no longer: the
    // row has a third version, which says what the first said.
    let report = load(&engine, "shop", &anvil);
    assert_eq!(
        (report.inserted, report.duplicates, report.superseded),
        (1, 0, 1)
    );
    assert_eq!(history(&engine, "shop", "1"), ["anvil", "hammer", "anvil"]);
    let third = engine
        .record(second)
        .unwrap()
        .unwrap()
        .superseded_by
        .unwrap();
    let entry = engine.record(third).unwrap().unwrap();
    assert_eq!(
        entry.payload,
        engine.record(first).unwrap().unwrap().payload
    );
    assert_eq!(
        (entry.supersedes, entry.superseded_by),
        (Some(second), None)
    );
    // Its ID is the content's and the ID of what it follows, so it is the
    // same in any store with this history.
    assert_eq!(third, first.held_again(second));
    // The first version is as it was: one version replaced it.
    let entry = engine.record(first).unwrap().unwrap();
    assert_eq!(
        (entry.supersedes, entry.superseded_by),
        (None, Some(second))
    );

    // Like any version, it joins the identity of the one before it.
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].new_records, 1);
    assert_eq!(report.resolved[0].identities_created, 0);
    assert_eq!(
        engine.identities_of(&[first, second, third]).unwrap(),
        [Some(identity); 3]
    );
    assert_eq!(engine.identity(identity).unwrap().members.len(), 3);

    // What the row holds now, sent again, is nothing new.
    let report = load(&engine, "shop", &anvil);
    assert_eq!((report.inserted, report.duplicates), (0, 1));
    // And the row can change the same way again.
    let report = load(&engine, "shop", &hammer);
    assert_eq!((report.inserted, report.superseded), (1, 1));
    assert_eq!(
        history(&engine, "shop", "1"),
        ["hammer", "anvil", "hammer", "anvil"]
    );
    let report = load(&engine, "shop", &hammer);
    assert_eq!((report.inserted, report.duplicates), (0, 1));
    resolve(&engine);
    assert_eq!(engine.identity(identity).unwrap().members.len(), 4);
    engine.verify().unwrap();
}

#[test]
fn a_row_that_changes_back_within_one_call_ends_as_the_last_row_says() {
    let engine = engine();
    // Two rows in a row that say the same are one version.
    let report = load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("1", Some("hammer"), None),
            ("1", Some("hammer"), None),
            ("1", Some("anvil"), None),
            ("2", Some("tongs"), None),
        ],
    );
    assert_eq!(
        (report.received, report.inserted, report.duplicates),
        (5, 4, 1)
    );
    assert_eq!(report.superseded, 2);
    assert_eq!(history(&engine, "shop", "1"), ["anvil", "hammer", "anvil"]);

    // The store holds the hammer as a version, and the call begins with it:
    // that is a change, and the anvil after it is what the row holds.
    let report = load(
        &engine,
        "shop",
        &[("1", Some("hammer"), None), ("1", Some("anvil"), None)],
    );
    assert_eq!((report.inserted, report.superseded), (2, 2));
    assert_eq!(
        history(&engine, "shop", "1"),
        ["anvil", "hammer", "anvil", "hammer", "anvil"]
    );

    resolve(&engine);
    let versions = records(&engine, "shop", "1");
    let identities = engine.identities_of(&versions).unwrap();
    assert!(identities[0].is_some());
    assert!(identities.iter().all(|found| *found == identities[0]));
    engine.verify().unwrap();
}

#[test]
fn a_row_without_a_key_has_no_versions_and_is_stored_once() {
    let engine = engine();
    let keyless = |name: &'static str| -> spool::Rows {
        spool::Rows::from_text(&["name"], &[vec![Some(name)]]).unwrap()
    };
    for (name, inserted) in [("anvil", 1), ("hammer", 1), ("anvil", 0)] {
        let report = engine.load("shop", "thing", keyless(name)).unwrap();
        assert_eq!(report.inserted, inserted, "{name}");
        assert_eq!(report.superseded, 0);
    }
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.record"), 2);
}

#[test]
fn a_new_version_that_matches_another_identity_is_left_for_review() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("hammer"), None)],
    );
    resolve(&engine);
    let (anvil, hammer) = (
        identity(&engine, "shop", "1").unwrap(),
        identity(&engine, "shop", "2").unwrap(),
    );

    // Row 1 now reads like row 2. Its key says it is still the anvil, the
    // rules say it is the hammer, and deciding would merge the two.
    load(&engine, "shop", &[("1", Some("hammer"), None)]);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].records_assigned, 0);
    let items = engine.review_items(&ReviewFilter::open()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ReviewKind::PossibleMerge);
    assert_eq!(items[0].identities.len(), 2);
    assert!(items[0].identities.contains(&anvil) && items[0].identities.contains(&hammer));
    engine.verify().unwrap();
}
