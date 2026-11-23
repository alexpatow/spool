//! Loading rows: what makes a record, and what a load refuses.

use super::*;

#[test]
fn load_stores_each_row_once() {
    let engine = engine();
    let batch = [
        ("1", Some("anvil"), Some("A")),
        ("2", Some("bellows"), None),
        ("1", Some("anvil"), Some("A")),
    ];
    let first = load(&engine, "shop", &batch);
    assert_eq!(
        (first.received, first.inserted, first.duplicates),
        (3, 2, 1)
    );

    let again = load(&engine, "shop", &batch);
    assert_eq!(
        (again.received, again.inserted, again.duplicates),
        (3, 0, 3)
    );

    assert_eq!(count(&engine, "SELECT count(*) FROM spool.record"), 2);
    assert_eq!(
        count(&engine, "SELECT count(*) FROM spool.features_thing"),
        2
    );
    engine.verify().unwrap();
}

#[test]
fn an_record_id_ignores_column_order_and_empty_columns() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);

    let reordered = Rows::from_text(&["name", "id"], &[vec![Some("anvil"), Some("1")]]).unwrap();
    let report = engine.load("shop", "thing", reordered).unwrap();
    assert_eq!((report.inserted, report.duplicates), (0, 1));
}

#[test]
fn a_changed_row_or_another_source_is_another_record() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), Some("A"))]);

    let changed = load(&engine, "shop", &[("1", Some("anvil, large"), Some("A"))]);
    assert_eq!(changed.inserted, 1);
    assert_eq!(records(&engine, "shop", "1").len(), 2);

    let elsewhere = load(&engine, "depot", &[("1", Some("anvil"), Some("A"))]);
    assert_eq!(elsewhere.inserted, 1);
}

#[test]
fn a_load_that_is_not_to_replace_rows_refuses_a_changed_row() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), Some("A")),
            ("2", Some("bellows"), Some("B")),
            ("3", Some("chisel"), Some("C")),
        ],
    );
    let stored = |engine: &Engine| count(engine, "SELECT count(*) FROM spool.record");

    // Two rows under keys the store holds with other values, one it holds as
    // it is, and one that is new. The call is refused whole, the new row
    // with it.
    let changed = || {
        rows(&[
            ("2", Some("bellows, large"), Some("B")),
            ("1", Some("anvil"), Some("A")),
            ("4", Some("drill"), Some("D")),
            ("3", Some("chisel, cold"), Some("C")),
        ])
    };
    let error = engine
        .load_without_replacing("shop", "thing", changed())
        .unwrap_err();
    assert!(
        matches!(&error, Error::Load(message) if message.starts_with(
            "2 rows have keys the store holds with other values: `2`, `3`. Nothing was loaded"
        ) && message.ends_with("load again with `replace` set to true")),
        "{error}"
    );
    assert_eq!(stored(&engine), 3);
    assert_eq!(
        count(&engine, "SELECT count(*) FROM spool.features_thing"),
        3
    );
    assert_eq!(
        select(
            &engine,
            "SELECT status FROM spool.run WHERE kind = 'load' ORDER BY run_id DESC LIMIT 1"
        ),
        [[Some("failed".to_owned())]]
    );
    engine.verify().unwrap();

    // Rows the store holds as they are, and new ones, are loaded as in any
    // load. A key that comes twice in one call replaces nothing the store
    // held: its second row is the next version of its first.
    let added = rows(&[
        ("1", Some("anvil"), Some("A")),
        ("4", Some("drill"), Some("D")),
        ("5", Some("file"), Some("F")),
        ("5", Some("file, flat"), Some("F")),
    ]);
    let report = engine
        .load_without_replacing("shop", "thing", added)
        .unwrap();
    assert_eq!(
        (report.inserted, report.duplicates, report.superseded),
        (3, 1, 1)
    );
    // The same key from another source is another row.
    let elsewhere = rows(&[("2", Some("bellows, small"), Some("B"))]);
    let report = engine
        .load_without_replacing("depot", "thing", elsewhere)
        .unwrap();
    assert_eq!((report.inserted, report.superseded), (1, 0));

    // A load that says nothing of the kind replaces them.
    let report = engine.load("shop", "thing", changed()).unwrap();
    assert_eq!((report.inserted, report.superseded), (2, 2));
    assert_eq!(records(&engine, "shop", "2").len(), 2);

    // What a row said before is other than it says now, so a load that
    // would change it back is refused as well: an older file after a newer
    // one replaces rows as surely as a newer one does.
    let before = || {
        rows(&[
            ("2", Some("bellows"), Some("B")),
            ("3", Some("chisel"), Some("C")),
        ])
    };
    let error = engine
        .load_without_replacing("shop", "thing", before())
        .unwrap_err();
    assert!(
        matches!(&error, Error::Load(message) if message.contains(": `2`, `3`. Nothing")),
        "{error}"
    );
    assert_eq!(records(&engine, "shop", "2").len(), 2);
    let report = engine.load("shop", "thing", before()).unwrap();
    assert_eq!((report.inserted, report.superseded), (2, 2));
    assert_eq!(records(&engine, "shop", "2").len(), 3);

    // One row is named as one.
    let one = rows(&[("4", Some("drill, hand"), Some("D"))]);
    let error = engine
        .load_without_replacing("shop", "thing", one)
        .unwrap_err();
    assert!(
        matches!(&error, Error::Load(message) if message.starts_with(
            "1 row has a key the store holds with other values: `4`. Nothing was loaded"
        ) && message.contains("If the row is meant to change")),
        "{error}"
    );
}

#[test]
fn a_load_that_is_not_to_replace_rows_names_so_many_keys() {
    let engine = engine();
    let keys: Vec<String> = (10..40).map(|key| key.to_string()).collect();
    let first: Vec<_> = keys
        .iter()
        .map(|key| (key.as_str(), Some("a"), None))
        .collect();
    let second: Vec<_> = keys
        .iter()
        .map(|key| (key.as_str(), Some("b"), None))
        .collect();
    load(&engine, "shop", &first);
    let error = engine
        .load_without_replacing("shop", "thing", rows(&second))
        .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("30 rows have keys the store holds with other values: `10`, `11`, "),
        "{message}"
    );
    assert!(message.contains("`29` and 10 more. Nothing"), "{message}");
    assert!(!message.contains("`30`"), "{message}");
}

#[test]
fn a_row_taken_back_is_not_replaced_either() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), Some("A"))]);
    engine
        .retract("shop", "thing", &["1".to_owned()], Actor::Engine, None)
        .unwrap();

    let changed = rows(&[("1", Some("anvil, large"), Some("A"))]);
    let error = engine
        .load_without_replacing("shop", "thing", changed)
        .unwrap_err();
    assert!(
        matches!(&error, Error::Load(message) if message.contains("`1`")),
        "{error}"
    );
    // The row is still taken back: the refused call held nothing again.
    let in_force = "SELECT count(*) FROM spool.retraction WHERE restored_run IS NULL";
    assert_eq!(count(&engine, in_force), 1);
    assert_eq!(records(&engine, "shop", "1").len(), 1);

    // Sent as it was, it comes back.
    let same = rows(&[("1", Some("anvil"), Some("A"))]);
    let report = engine
        .load_without_replacing("shop", "thing", same)
        .unwrap();
    assert_eq!((report.inserted, report.restored), (0, 1));
    assert_eq!(count(&engine, in_force), 0);
}

#[test]
fn load_needs_an_applied_manifest_and_a_declared_type() {
    let bare = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let error = bare.load("shop", "thing", rows(&[])).unwrap_err();
    assert!(matches!(error, Error::NoManifest), "{error}");

    let engine = engine();
    let error = engine.load("shop", "gadget", rows(&[])).unwrap_err();
    assert!(matches!(error, Error::UndeclaredType(name) if name == "gadget"));
}

#[test]
fn evidence_is_stored_beside_the_payload_and_needs_a_key() {
    let mut thing = thing();
    thing.entity.yaml += "  report:\n    name: name\n";
    let mut project = project_of(vec![thing]);
    project.sources.push(YamlFile::new(
        "report",
        "key: locator\nevidence: evidence\ncolumns: [locator, name, evidence]\n",
    ));
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&project.compile().unwrap()).unwrap();
    let mentions = |locator: Option<&'static str>| {
        Rows::from_text(
            &["locator", "name", "evidence"],
            &[vec![locator, Some("anvil"), Some(r#"{"page": 3}"#)]],
        )
        .unwrap()
    };

    let error = engine.load("report", "thing", mentions(None)).unwrap_err();
    assert!(
        matches!(&error, Error::Load(message) if message.contains("evidence")),
        "{error}"
    );
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.record"), 0);
    assert_eq!(
        select(&engine, "SELECT status FROM spool.run WHERE kind = 'load'"),
        [[Some("failed".to_owned())]]
    );

    engine
        .load("report", "thing", mentions(Some("p3:120-125")))
        .unwrap();
    assert_eq!(
        select(
            &engine,
            "SELECT source_key, payload::VARCHAR, evidence->>'page' FROM spool.record"
        ),
        [[
            Some("p3:120-125".to_owned()),
            Some(r#"{"locator":"p3:120-125","name":"anvil"}"#.to_owned()),
            Some("3".to_owned())
        ]]
    );

    // The same words at another place in the document are another record.
    let report = engine
        .load("report", "thing", mentions(Some("p9:14-19")))
        .unwrap();
    assert_eq!(report.inserted, 1);
}

#[test]
fn a_known_row_cannot_arrive_as_another_entity_type() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let widget = EntityTypeSource {
        name: "widget".to_owned(),
        ..thing()
    };
    engine.apply(&manifest_of(vec![thing(), widget])).unwrap();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);

    let error = engine
        .load("shop", "widget", rows(&[("1", Some("anvil"), None)]))
        .unwrap_err();
    assert!(
        matches!(&error, Error::Load(message) if message.contains("`thing`")),
        "{error}"
    );
}
