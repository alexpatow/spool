//! `spool show attributes`: what the records that stand hold.

use super::*;

#[test]
#[expect(clippy::too_many_lines)]
fn the_attributes_of_the_records_that_stand_are_shown_across_them() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);
    load(
        &server,
        "shop",
        "thing",
        rows(&[
            &["id", "code", "colour", "name", "weight"],
            &["1", "anvil", "black", "Big anvil", "40"],
            &["2", "anvil", "black", "", "40"],
            &["3", "anvil", "red", "Red anvil", ""],
            &["4", "hammer", "black", "", ""],
        ]),
    );
    // A new version of a row replaces the old one, and a row its source
    // took back stands no more: neither is counted.
    load(
        &server,
        "shop",
        "thing",
        rows(&[
            &["id", "code", "colour", "name", "weight"],
            &["1", "anvil", "grey", "Big anvil", "40"],
        ]),
    );
    server.call(
        "/v1/retract",
        json!({ "source": "shop", "entity_type": "thing", "keys": ["4"] }),
    );

    let shop = shown(&server, &["show", "attributes", "--source", "shop"]);
    has(&shop, "entity type `thing`, source `shop`: 3 records");
    has(&shop, "ATTRIBUTE FILLED DISTINCT MOST COMMON FEATURES");
    has(&shop, "id (key)  3 (100%)  3  e.g. 1, 2, 3  -");
    has(&shop, "code  3 (100%)  1  anvil (3)  code");
    has(&shop, "colour  3 (100%)  3  e.g. black, grey, red  colour");
    has(&shop, "name  2 (66%)  2  e.g. Big anvil, Red anvil  -");
    has(&shop, "weight  2 (66%)  1  40 (2)  -");
    has(
        &shop,
        "`spool show records --source shop --attributes` lists the values record by record",
    );

    // Every source the type maps is there, with records or without, and a
    // source's evidence column is not one of its attributes.
    let all = answered(&server, &["show", "attributes"]);
    assert_eq!(all, server.read("/v1/attributes"));
    let groups = all["groups"].as_array().unwrap();
    assert_eq!(groups.len(), THING_SOURCES.len(), "{all}");
    let notes = groups
        .iter()
        .find(|group| group["source"] == "notes")
        .unwrap();
    assert_eq!(notes["records"], 0);
    let names: Vec<&str> = notes["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|attribute| attribute["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["id", "code", "colour", "name", "weight"]);
    let printed = shown(&server, &["show", "attributes", "--source", "notes"]);
    has(
        &printed,
        "entity type `thing`, source `notes`: no records yet",
    );
    assert_eq!(
        shown(&server, &["show", "attributes", "--source", "nowhere"]),
        "no records of that entity type and source\n"
    );

    // A list of records shows the version of each row that nothing
    // replaced, and every version when asked.
    let records = shown(&server, &["show", "records"]);
    assert_eq!(
        records
            .lines()
            .filter(|line| line.starts_with("shop:"))
            .count(),
        4,
        "{records}"
    );
    let history = shown(
        &server,
        &[
            "show",
            "records",
            "--source",
            "shop",
            "--key",
            "1",
            "--all-versions",
        ],
    );
    has(&history, "SOURCE AND KEY  NAME  IDENTITY  VERSION");
    has(&history, "shop:1  anvil  -  replaced");
    has(&history, "shop:1  anvil  -  current");
    let latest = answered(
        &server,
        &["show", "records", "--source", "shop", "--key", "1"],
    );
    assert_eq!(latest["records"].as_array().unwrap().len(), 1);
    assert_eq!(latest["records"][0]["payload"]["colour"], "grey");

    // Their attributes are one option away, a column each, the key apart.
    let wide = shown(&server, &["show", "records", "--attributes"]);
    has(
        &wide,
        "SOURCE AND KEY  CODE  COLOUR  NAME  WEIGHT  IDENTITY",
    );
    has(&wide, "shop:1  anvil  grey  Big anvil  40  -");
    has(&wide, "shop:3  anvil  red  Red anvil  -  -");

    // So does a list of documents.
    for text in ["one", "two"] {
        server.call(
            "/v1/documents",
            json!({ "source": "notes", "documents": [{ "key": "a.txt", "text": text }] }),
        );
    }
    let documents = shown(&server, &["show", "documents"]);
    assert_eq!(documents.lines().count(), 2, "{documents}");
    let every = shown(&server, &["show", "documents", "--all-versions"]);
    assert_eq!(every.lines().count(), 3, "{every}");
    has(
        &every,
        "DOCUMENT  SOURCE AND KEY  CHARACTERS  READ BY  VERSION",
    );
    assert_eq!(every.matches(" replaced\n").count(), 1, "{every}");
    assert_eq!(every.matches(" current\n").count(), 1, "{every}");

    // A mention found in a version of a document that a later one replaced
    // is not current, though nothing replaced the mention itself.
    let versions = server.read("/v1/documents?source=notes&key=a.txt")["documents"].clone();
    let version = |replaced: bool| {
        versions
            .as_array()
            .unwrap()
            .iter()
            .find(|document| document["superseded_by"].is_null() != replaced)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    for (row, code, document) in [
        ("m-old", "anvil", version(true)),
        ("m-new", "tongs", version(false)),
    ] {
        let evidence = json!({ "document": document, "key": "a.txt", "start": 0, "end": 3 });
        load(
            &server,
            "notes",
            "thing",
            rows(&[
                &["id", "code", "where"],
                &[row, code, &evidence.to_string()],
            ]),
        );
    }
    let mentions = answered(&server, &["show", "records", "--source", "notes"]);
    let keys: Vec<&str> = mentions["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["m-new"], "{mentions}");
    let every = shown(
        &server,
        &["show", "records", "--source", "notes", "--all-versions"],
    );
    assert_eq!(every.matches("notes:a.txt[0-3]").count(), 2, "{every}");
    has(&every, "notes:a.txt[0-3]  anvil  -  replaced");
    has(&every, "notes:a.txt[0-3]  tongs  -  current");
    let notes = shown(&server, &["show", "attributes", "--source", "notes"]);
    has(&notes, "entity type `thing`, source `notes`: 1 record");
    has(&notes, "code  1 (100%)  1  tongs  code");

    for (arguments, complaint) in [
        (
            &["show", "attributes", "--key", "1"][..],
            "unexpected argument '--key'",
        ),
        (
            &["show", "records", "shop:1", "--attributes"],
            "the argument '[RECORD]' cannot be used with '--attributes'",
        ),
        (
            &["show", "runs", "--all-versions"],
            "unexpected argument '--all-versions'",
        ),
        (
            &["show", "documents", "--latest"],
            "unexpected argument '--latest'",
        ),
    ] {
        let (code, said) = refused(&server, arguments);
        assert_eq!(code, Some(2), "{arguments:?}");
        assert!(said.starts_with(&format!("error: {complaint}")), "{said}");
    }
}

#[test]
fn an_attribute_the_source_no_longer_declares_is_still_listed() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::dev(&project);
    load(
        &server,
        "shop",
        "thing",
        rows(&[&["id", "code", "weight"], &["1", "anvil", "40"]]),
    );
    std::fs::write(
        project.join("sources/shop.yaml"),
        "key: id\ncolumns: [id, code, colour, name]\n",
    )
    .unwrap();
    server.printed("applied");

    let wide = shown(
        &server,
        &["show", "records", "--source", "shop", "--attributes"],
    );
    has(
        &wide,
        "SOURCE AND KEY  CODE  COLOUR  NAME  WEIGHT  IDENTITY",
    );
    has(&wide, "shop:1  anvil  -  -  40  -");
    let profile = shown(&server, &["show", "attributes", "--source", "shop"]);
    has(&profile, "weight  1 (100%)  1  40  -");
}
