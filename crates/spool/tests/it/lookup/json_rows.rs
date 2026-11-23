//! Rows given as JSON: the types their columns keep, and those refused.

use super::*;

#[test]
fn rows_given_as_json_keep_their_types() {
    let engine = engine();
    engine
        .load(
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
        .load(
            "till",
            "thing",
            json_rows(json!([{ "id": 1, "name": "anvil", "price": 12, "stocked": true }])),
        )
        .unwrap();
    assert_eq!((report.inserted, report.duplicates), (0, 1));

    // No rows at all is not an error.
    let report = engine.load("till", "thing", json_rows(json!([]))).unwrap();
    assert_eq!(report.received, 0);
}

#[test]
fn json_rows_without_a_value_are_rows_all_the_same() {
    let engine = engine();
    let report = engine
        .load("till", "thing", json_rows(json!([{}, { "name": null }])))
        .unwrap();
    assert_eq!(
        (report.received, report.inserted, report.duplicates),
        (2, 1, 1),
        "two rows that say nothing are one record, seen twice"
    );

    // A column is every key that occurs, with a value or without.
    let report = engine
        .load(
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
    let load = |rows: serde_json::Value| engine.load("scale", "thing", json_rows(rows)).unwrap();
    load(json!([
        { "id": "1", "weight": 0.5 },
        { "id": "2", "weight": 9007199254740991u64 },
    ]));
    let report = load(json!([{ "id": "2", "weight": 9007199254740991u64 }]));
    assert_eq!((report.inserted, report.duplicates), (0, 1));

    // On their own, integers are exact as far as 64 bits go, signed or not.
    load(json!([
        { "id": "3", "weight": 9007199254740993u64 },
        { "id": "4", "weight": i64::MIN },
    ]));
    load(json!([
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
    let report = load(json!([{ "id": "6", "weight": 7 }]));
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
