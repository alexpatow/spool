//! Reads of single things, typed review items, and rows given as JSON.

mod common;

use common::*;
use serde_json::json;
use spool::core::{Actor, EntityType, IdGenerator, Op, SeededIds, Seq};
use spool::{
    Engine, Error, IdentityFilter, Manifest, Options, RecordFilter, ResolveOptions, ReviewFilter,
    ReviewKind, ReviewStatus, Rows, YamlFile,
};

/// The tests' things, and two sources more whose rows hold numbers.
fn engine() -> Engine {
    let mut thing = thing();
    thing.entity.yaml +=
        "  till:\n    name: name\n    code: code\n  scale: {}\n  notes:\n    name: name\n";
    let mut project = project_of(vec![thing]);
    project.sources.extend([
        YamlFile::new(
            "notes",
            "key: id\nevidence: where\ncolumns: [id, name, where]\n",
        ),
        YamlFile::new(
            "till",
            "key: id\ncolumns: [id, name, price, stocked, code]\n",
        ),
        YamlFile::new("scale", "key: id\ncolumns: [id, weight]\n"),
    ]);
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&project.compile().unwrap()).unwrap();
    engine
}

fn thing_type() -> EntityType {
    EntityType::new("thing").unwrap()
}

/// Two anvils that share an identity, a hammer of its own, and a record that
/// matches both and goes to review.
fn populated() -> Engine {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", None, Some("H1")),
        ],
    );
    engine.resolve(&ResolveOptions::default()).unwrap();
    ingest(&engine, "depot", &[("9", Some("anvil"), Some("H1"))]);
    engine.resolve(&ResolveOptions::default()).unwrap();
    engine
}

fn json_rows(rows: serde_json::Value) -> Rows {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_value(rows).unwrap();
    Rows::from_json(&rows).unwrap()
}

#[test]
fn an_record_is_found_by_its_id_and_by_its_source_and_key() {
    let engine = populated();
    let id = record_id(&engine, "shop", "1");

    let entry = engine.record(id).unwrap().unwrap();
    assert_eq!(entry.id, id);
    assert_eq!(entry.entity_type, thing_type());
    assert_eq!(
        (entry.source.as_str(), entry.key.as_deref()),
        ("shop", Some("1"))
    );
    assert_eq!(entry.payload, json!({ "id": "1", "name": "anvil" }));
    assert_eq!(entry.evidence, None);
    assert_eq!(entry.identity, identity(&engine, "shop", "1"));
    assert_eq!(entry.ingest_run.0, 1);

    // A caller knows its rows by source and key; the engine knows the ID.
    let found = engine.records(&RecordFilter::keyed("shop", "1")).unwrap();
    assert_eq!(found, [entry]);
    assert!(
        engine
            .records(&RecordFilter::keyed("shop", "none"))
            .unwrap()
            .is_empty()
    );

    // The one left for review has no identity.
    let unassigned = engine
        .records(&RecordFilter {
            unassigned: true,
            ..RecordFilter::default()
        })
        .unwrap();
    assert_eq!(unassigned.len(), 1);
    assert_eq!(unassigned[0].id, record_id(&engine, "depot", "9"));
    assert_eq!(unassigned[0].identity, None);

    // Pages continue after the last record seen.
    let all = engine.records(&RecordFilter::default()).unwrap();
    assert_eq!(all.len(), 4);
    let first = engine
        .records(&RecordFilter {
            limit: 3,
            ..RecordFilter::default()
        })
        .unwrap();
    let rest = engine
        .records(&RecordFilter {
            after: Some(first[2].id),
            ..RecordFilter::default()
        })
        .unwrap();
    assert_eq!([first, rest].concat(), all);

    let unknown = spool::core::RecordId::from_u128(7);
    assert_eq!(engine.record(unknown).unwrap(), None);
    assert_eq!(
        engine
            .identities_of(&[id, unknown, record_id(&engine, "depot", "9")])
            .unwrap(),
        [identity(&engine, "shop", "1"), None, None]
    );
}

#[test]
fn records_are_listed_by_source_and_key() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[
            ("b", Some("bolt"), None),
            ("10", Some("ten"), None),
            ("2", Some("two"), None),
        ],
    );
    ingest(&engine, "depot", &[("z", Some("last of the depot"), None)]);
    // A second version of a row, and rows that have no key.
    ingest(&engine, "shop", &[("10", Some("ten again"), None)]);
    engine
        .ingest(
            "shop",
            "thing",
            json_rows(json!([{ "name": "keyless" }, { "name": "keyless too" }])),
        )
        .unwrap();

    // The rows of a source are together, in the order of their keys as
    // text, a row's versions the oldest first, and the keyless ones last.
    let all = engine.records(&RecordFilter::default()).unwrap();
    let listed: Vec<(&str, Option<&str>, u64)> = all
        .iter()
        .map(|record| {
            (
                record.source.as_str(),
                record.key.as_deref(),
                record.ingest_run.0,
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("depot", Some("z"), 2),
            ("shop", Some("10"), 1),
            ("shop", Some("10"), 3),
            ("shop", Some("2"), 1),
            ("shop", Some("b"), 1),
            ("shop", None, 4),
            ("shop", None, 4),
        ]
    );
    assert_eq!(all[2].supersedes, Some(all[1].id));
    assert!(all[5].id < all[6].id, "the keyless ones by their IDs");

    // Every page size walks the same list, with no record twice or missing.
    for limit in 1..=all.len() as u64 {
        let mut walked = Vec::new();
        let mut after = None;
        loop {
            let page = engine
                .records(&RecordFilter {
                    limit,
                    after,
                    ..RecordFilter::default()
                })
                .unwrap();
            let Some(last) = page.last() else { break };
            after = Some(last.id);
            walked.extend(page);
        }
        assert_eq!(walked, all, "in pages of {limit}");
    }
    // A filter keeps the order and the place.
    let shop = engine
        .records(&RecordFilter {
            source: Some("shop".to_owned()),
            after: Some(all[3].id),
            ..RecordFilter::default()
        })
        .unwrap();
    assert_eq!(shop, all[4..]);

    // A place in the list is a record the store holds.
    let unknown = spool::core::RecordId::from_u128(7);
    let refused = engine
        .records(&RecordFilter {
            after: Some(unknown),
            ..RecordFilter::default()
        })
        .unwrap_err();
    assert!(
        matches!(refused, Error::UnknownRecord(id) if id == unknown),
        "{refused}"
    );
}

#[test]
fn mentions_are_listed_by_their_document_and_their_place_in_it() {
    let engine = engine();
    // What an extractor writes: a key that begins with the ID of the
    // document, and evidence that says where in it the record was found.
    let (first, second) = (
        "11111111-1111-7111-8111-111111111111",
        "99999999-9999-7999-8999-999999999999",
    );
    let mention = |document: &str, key: &str, start: u64, end: u64| {
        json!({
            "id": format!("{document}#thing:{start}-{end}"),
            "name": format!("at {start}"),
            "where": json!({ "document": document, "key": key, "start": start, "end": end })
                .to_string(),
        })
    };
    engine
        .ingest(
            "notes",
            "thing",
            json_rows(json!([
                mention(first, "b.txt", 105, 110),
                mention(first, "b.txt", 12, 20),
                mention(second, "a.txt", 7, 9),
                { "id": "k", "name": "a row" },
                { "id": "z", "name": "a row of a page", "where": json!({ "page": 4 }).to_string() },
            ])),
        )
        .unwrap();

    // The documents are in the order of their keys, which is what a reader
    // knows them by, and a document's mentions in the order of the text:
    // by their keys, `[105-110]` would come before `[12-20]`.
    let all = engine.records(&RecordFilter::default()).unwrap();
    let names: Vec<String> = all.iter().map(|record| record.name()).collect();
    assert_eq!(
        names,
        [
            "notes:a.txt[7-9]",
            "notes:b.txt[12-20]",
            "notes:b.txt[105-110]",
            "notes:k",
            "notes:z",
        ]
    );
    for limit in 1..=all.len() as u64 {
        let mut walked = Vec::new();
        let mut after = None;
        loop {
            let page = engine
                .records(&RecordFilter {
                    limit,
                    after,
                    ..RecordFilter::default()
                })
                .unwrap();
            let Some(last) = page.last() else { break };
            after = Some(last.id);
            walked.extend(page);
        }
        assert_eq!(walked, all, "in pages of {limit}");
    }
}

#[test]
fn an_identity_is_read_with_its_members_and_its_history() {
    let engine = populated();
    let anvil = identity(&engine, "shop", "1").unwrap();

    let view = engine.identity(anvil).unwrap();
    assert_eq!((view.id, view.current), (anvil, anvil));
    assert_eq!(view.entity_type, thing_type());
    assert_eq!(view.created, Seq(1));
    let keys: Vec<(&str, Option<&str>)> = {
        let mut keys: Vec<_> = view
            .members
            .iter()
            .map(|member| (member.source.as_str(), member.key.as_deref()))
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(keys, [("shop", Some("1")), ("shop", Some("2"))]);
    assert!(view.members.iter().all(|member| member.since == Seq(1)));

    // History holds only the operations that name the identity.
    let history = engine.history(anvil).unwrap();
    let [entry] = &history[..] else {
        panic!("one changeset touched the identity");
    };
    assert_eq!(entry.seq, Seq(1));
    assert_eq!(entry.actor, Actor::Engine);
    assert_eq!(entry.changeset, engine.log().unwrap()[0].changeset.id);
    assert_eq!(entry.ops.len(), 3);
    assert!(matches!(entry.ops[0], Op::CreateIdentity { identity, .. } if identity == anvil));
    assert!(engine.log().unwrap()[0].changeset.ops.len() > 3);

    let listed = engine.identities(&IdentityFilter::default()).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed[0].id < listed[1].id);
    let sizes: u64 = listed.iter().map(|identity| identity.members).sum();
    assert_eq!(sizes, 3);
    let after = engine
        .identities(&IdentityFilter {
            after: Some(listed[0].id),
            entity_type: Some(thing_type()),
            ..IdentityFilter::default()
        })
        .unwrap();
    assert_eq!(after, listed[1..]);

    let unknown = SeededIds::new(40).identity_id();
    for error in [
        engine.identity(unknown).map(|_| ()).unwrap_err(),
        engine.history(unknown).map(|_| ()).unwrap_err(),
    ] {
        assert!(
            matches!(error, Error::UnknownIdentity(id) if id == unknown),
            "{error}"
        );
    }
}

#[test]
fn review_items_are_read_as_typed_records() {
    let engine = populated();
    let [item] = &engine.review_items(&ReviewFilter::open()).unwrap()[..] else {
        panic!("one open review item");
    };
    assert_eq!(item.kind, ReviewKind::PossibleMerge);
    assert_eq!(item.status, ReviewStatus::Open);
    assert_eq!(item.entity_type, thing_type());
    assert_eq!(item.records, [record_id(&engine, "depot", "9")]);
    let mut matched = vec![
        identity(&engine, "shop", "1").unwrap(),
        identity(&engine, "shop", "3").unwrap(),
    ];
    matched.sort();
    assert_eq!(item.identities, matched);
    assert_eq!((item.resolved_seq, item.superseded_by), (None, None));

    // A reviewer decides, and the item says which changeset settled it.
    let mut ids = SeededIds::new(50);
    let committed = engine
        .commit(spool::core::Changeset {
            id: ids.changeset_id(),
            base: engine.head().unwrap(),
            actor: Actor::User("reviewer".to_owned()),
            reason: Some("it is the anvil".to_owned()),
            run: None,
            ops: vec![Op::Assign {
                record: item.records[0],
                from: None,
                to: Some(matched[0]),
            }],
        })
        .unwrap();
    assert!(
        engine
            .review_items(&ReviewFilter::open())
            .unwrap()
            .is_empty()
    );
    let [closed] = &engine.review_items(&ReviewFilter::default()).unwrap()[..] else {
        panic!("one review item");
    };
    assert_eq!(closed.status, ReviewStatus::Resolved);
    assert_eq!(closed.resolved_seq, Some(committed.seq));
    assert_eq!(closed.id, item.id);

    let info = engine.info().unwrap();
    let [thing] = &info.entity_types[..] else {
        panic!("one entity type");
    };
    assert_eq!(info.head, committed.seq);
    assert_eq!(
        info.manifest_hash,
        engine.manifest().map(|manifest| manifest.hash.clone())
    );
    assert_eq!(
        (thing.records, thing.unassigned, thing.identities),
        (4, 0, 2)
    );
    assert_eq!(thing.open_review_items, 0);
    assert_eq!(thing.features.len(), 3);
}

#[test]
fn the_log_is_read_in_pages_and_ids_are_minted_for_callers() {
    let engine = populated();
    let log = engine.log().unwrap();
    assert_eq!(log.len(), 1);
    let minted = engine.mint_ids(2, 1);
    assert_eq!((minted.identities.len(), minted.changesets.len()), (2, 1));
    assert_ne!(minted.identities[0], minted.identities[1]);

    let unassigned = record_id(&engine, "depot", "9");
    engine
        .commit(spool::core::Changeset {
            id: minted.changesets[0],
            base: engine.head().unwrap(),
            actor: Actor::Agent("sorter".to_owned()),
            reason: None,
            run: None,
            ops: vec![
                Op::CreateIdentity {
                    identity: minted.identities[0],
                    entity_type: thing_type(),
                },
                Op::Assign {
                    record: unassigned,
                    from: None,
                    to: Some(minted.identities[0]),
                },
            ],
        })
        .unwrap();

    let log = engine.log().unwrap();
    assert_eq!(engine.log_after(Seq::ZERO, 10).unwrap(), log);
    assert_eq!(engine.log_after(Seq::ZERO, 1).unwrap(), log[..1]);
    assert_eq!(engine.log_after(Seq(1), 10).unwrap(), log[1..]);
    assert!(engine.log_after(Seq(2), 10).unwrap().is_empty());
    assert_eq!(log[1].changeset.ops.len(), 2);
}

#[test]
fn a_log_position_no_log_reaches_has_nothing_after_it_and_nothing_at_it() {
    let engine = populated();
    assert!(!engine.log().unwrap().is_empty());
    // The database counts positions in signed 64 bits, and a caller may
    // name any unsigned one.
    for position in [i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX] {
        assert!(
            engine.log_after(Seq(position), 10).unwrap().is_empty(),
            "after {position}"
        );
        assert!(
            matches!(
                engine.undo(Seq(position), Actor::User("ada".to_owned()), None, true),
                Err(Error::UnknownChangeset(Seq(unknown))) if unknown == position
            ),
            "undo of {position}"
        );
    }
}

#[test]
fn rows_given_as_json_keep_their_types() {
    let engine = engine();
    engine
        .ingest(
            "till",
            "thing",
            json_rows(json!([
                { "id": 1, "name": "anvil", "price": 12, "stocked": true },
                { "id": 2, "name": "hammer", "price": 7.5, "stocked": false, "code": null },
                { "id": 3 },
            ])),
        )
        .unwrap();

    let payload = |key: &str| {
        engine
            .records(&RecordFilter::keyed("till", key))
            .unwrap()
            .remove(0)
            .payload
    };
    assert_eq!(
        payload("1"),
        json!({ "id": 1, "name": "anvil", "price": 12, "stocked": true })
    );
    assert_eq!(
        payload("2"),
        json!({ "id": 2, "name": "hammer", "price": 7.5, "stocked": false })
    );
    assert_eq!(payload("3"), json!({ "id": 3 }));

    // A row is the same record whatever it arrives with: on its own,
    // the price column is whole numbers; above, it was not.
    let report = engine
        .ingest(
            "till",
            "thing",
            json_rows(json!([{ "id": 1, "name": "anvil", "price": 12, "stocked": true }])),
        )
        .unwrap();
    assert_eq!((report.inserted, report.duplicates), (0, 1));

    // No rows at all is not an error.
    let report = engine
        .ingest("till", "thing", json_rows(json!([])))
        .unwrap();
    assert_eq!(report.received, 0);
}

#[test]
fn a_snapshot_read_while_changesets_commit_is_of_one_log_position() {
    const THINGS: usize = 120;
    let engine = engine();
    let keys: Vec<String> = (0..THINGS).map(|index| index.to_string()).collect();
    let things: Vec<(&str, Option<&str>, Option<&str>)> = keys
        .iter()
        .map(|key| (key.as_str(), Some(key.as_str()), None))
        .collect();
    ingest(&engine, "shop", &things);
    let unassigned: Vec<_> = keys
        .iter()
        .map(|key| record_id(&engine, "shop", key))
        .collect();

    // Every changeset creates one identity and gives it one member, so at
    // log position N there are N of each.
    let done = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for record in &unassigned {
                let minted = engine.mint_ids(1, 1);
                engine
                    .commit(spool::core::Changeset {
                        id: minted.changesets[0],
                        base: engine.head().unwrap(),
                        actor: Actor::Agent("sorter".to_owned()),
                        reason: None,
                        run: None,
                        ops: vec![
                            Op::CreateIdentity {
                                identity: minted.identities[0],
                                entity_type: thing_type(),
                            },
                            Op::Assign {
                                record: *record,
                                from: None,
                                to: Some(minted.identities[0]),
                            },
                        ],
                    })
                    .unwrap();
            }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        while !done.load(std::sync::atomic::Ordering::SeqCst) {
            let snapshot = engine.snapshot().unwrap();
            let head = snapshot.head.0 as usize;
            assert_eq!(
                (snapshot.identities.len(), snapshot.memberships.len()),
                (head, head),
                "the identities and memberships of log position {head}"
            );
        }
    });
    assert_eq!(engine.head().unwrap(), Seq(THINGS as u64));
    engine.verify().unwrap();
}

#[test]
fn json_rows_without_a_value_are_rows_all_the_same() {
    let engine = engine();
    let report = engine
        .ingest("till", "thing", json_rows(json!([{}, { "name": null }])))
        .unwrap();
    assert_eq!(
        (report.received, report.inserted, report.duplicates),
        (2, 1, 1),
        "two rows that say nothing are one record, seen twice"
    );

    // A column is every key that occurs, with a value or without.
    let report = engine
        .ingest(
            "till",
            "thing",
            json_rows(json!([{ "id": null, "name": null }])),
        )
        .unwrap();
    assert_eq!((report.received, report.duplicates), (1, 1));
}

#[test]
fn json_integers_are_kept_exactly_or_refused() {
    let rows = |rows: serde_json::Value| -> Vec<serde_json::Map<String, serde_json::Value>> {
        serde_json::from_value(rows).unwrap()
    };

    // 2^53 + 1 is not a number a float can hold, so it cannot share a column
    // with a fraction: it would be stored as its neighbour.
    let error = Rows::from_json(&rows(json!([
        { "id": "1", "weight": 0.5 },
        { "id": "2", "weight": 9007199254740993u64 },
    ])))
    .err()
    .unwrap();
    assert!(
        matches!(&error, Error::Rows(message) if message.contains("row 2") && message.contains("`weight`")),
        "{error}"
    );
    let error = Rows::from_json(&rows(json!([
        { "id": "1", "weight": -9007199254740993i64 },
        { "id": "2", "weight": 0.5 },
    ])))
    .err()
    .unwrap();
    assert!(
        matches!(&error, Error::Rows(message) if message.contains("row 1") && message.contains("`weight`")),
        "{error}"
    );

    // Below that, an integer beside a fraction is the integer it was.
    let engine = engine();
    let ingest =
        |rows: serde_json::Value| engine.ingest("scale", "thing", json_rows(rows)).unwrap();
    ingest(json!([
        { "id": "1", "weight": 0.5 },
        { "id": "2", "weight": 9007199254740991u64 },
    ]));
    let report = ingest(json!([{ "id": "2", "weight": 9007199254740991u64 }]));
    assert_eq!((report.inserted, report.duplicates), (0, 1));

    // On their own, integers are exact as far as 64 bits go, signed or not.
    ingest(json!([
        { "id": "3", "weight": 9007199254740993u64 },
        { "id": "4", "weight": i64::MIN },
    ]));
    ingest(json!([
        { "id": "5", "weight": u64::MAX },
        { "id": "6", "weight": 7 },
    ]));
    let weight = |key: &str| {
        engine
            .records(&RecordFilter::keyed("scale", key))
            .unwrap()
            .remove(0)
            .payload["weight"]
            .to_string()
    };
    assert_eq!(weight("2"), "9007199254740991");
    assert_eq!(weight("3"), "9007199254740993");
    assert_eq!(weight("4"), "-9223372036854775808");
    assert_eq!(weight("5"), "18446744073709551615");
    assert_eq!(weight("6"), "7");
    let report = ingest(json!([{ "id": "6", "weight": 7 }]));
    assert_eq!((report.inserted, report.duplicates), (0, 1));

    // A column cannot hold both a negative integer and one past the signed
    // range: no one type keeps both.
    let error = Rows::from_json(&rows(json!([
        { "id": "7", "weight": -1 },
        { "id": "8", "weight": u64::MAX },
    ])))
    .err()
    .unwrap();
    assert!(
        matches!(&error, Error::Rows(message) if message.contains("row 2") && message.contains("`weight`")),
        "{error}"
    );
}

#[test]
fn json_rows_that_have_no_one_type_per_column_are_refused() {
    let nested: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_value(json!([{ "id": "1" }, { "id": "2", "tags": ["a"] }])).unwrap();
    let error = Rows::from_json(&nested).err().unwrap();
    assert!(
        matches!(&error, Error::Rows(message) if message.contains("row 2") && message.contains("`tags`")),
        "{error}"
    );

    let mixed: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_value(json!([{ "id": "1" }, { "id": 2 }])).unwrap();
    let error = Rows::from_json(&mixed).err().unwrap();
    assert!(
        matches!(&error, Error::Rows(message) if message.contains("more than one type")),
        "{error}"
    );
}

#[test]
fn a_manifest_is_written_and_read_back_and_checked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".spool").join("manifest.json");
    let manifest = things();
    manifest.write_file(&path).unwrap();
    assert_eq!(Manifest::read_file(&path).unwrap(), manifest);

    // A manifest edited by hand no longer has the hash it claims.
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("l.name = r.name", "true")).unwrap();
    let error = Manifest::read_file(&path).unwrap_err();
    assert!(
        matches!(&error, Error::Project { message, .. } if message.contains("hash")),
        "{error}"
    );
    std::fs::write(&path, "{}").unwrap();
    assert!(matches!(
        Manifest::read_file(&path),
        Err(Error::Project { .. })
    ));
    assert!(matches!(
        Manifest::read_file(dir.path().join("missing.json")),
        Err(Error::Io { .. })
    ));
}
