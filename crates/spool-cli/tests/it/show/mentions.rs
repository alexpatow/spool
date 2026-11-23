//! Mentions in their documents, and the versions of a row, by what names them.

use super::*;

#[test]
fn a_mention_is_shown_in_its_document() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    let filler = "x".repeat(100);
    let note = format!("{filler} The Anvil is\nblack, as anvils are. {filler}");
    server.call(
        "/v1/documents",
        json!({ "source": "notes", "documents": [
            { "key": "a.txt", "uri": "file:///notes/a.txt", "text": note },
            { "key": "b.txt", "text": "A hammer." },
        ] }),
    );
    let documents = server.read("/v1/documents?source=notes&key=a.txt");
    let document = documents["documents"][0]["id"].as_str().unwrap();

    // Evidence that names a place in a stored document is what an extractor
    // writes. A caller that finds mentions itself can write the same.
    let evidence = json!({ "document": document, "key": "a.txt", "start": 105, "end": 110 });
    let response = server.post(
        "/v1/load",
        json!({
            "source": "notes",
            "entity_type": "thing",
            "rows": [{ "id": "m1", "code": "anvil", "where": evidence.to_string() }],
        }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");

    let listed = shown(&server, &["show", "documents", "--source", "notes"]);
    has(
        &listed,
        "DOCUMENT                              SOURCE AND KEY  CHARACTERS  READ BY",
    );
    has(
        &listed,
        &format!(
            "{document}  notes:a.txt     {:<10}  not read yet",
            note.chars().count()
        ),
    );
    assert_eq!(listed.lines().count(), 3, "{listed}");
    let one = shown(&server, &["show", "documents", document]);
    has(&one, &format!("document    {document}"));
    has(&one, "uri         file:///notes/a.txt");
    assert!(one.ends_with(&format!("\n\n{note}\n")), "{one}");

    let record = shown(&server, &["show", "records", "notes:m1"]);
    has(&record, "evidence");
    has(&record, &format!("  document  {document}"));
    has(&record, "in document notes:a.txt, characters 105 to 110");
    // Eighty characters either side, on one line, with the mention marked.
    has(
        &record,
        &format!(
            "  …{} The [Anvil] is black, as anvils are. {}…",
            "x".repeat(75),
            "x".repeat(54)
        ),
    );
    let bundle = answered(&server, &["show", "records", "notes:m1"]);
    assert_eq!(bundle["mention"]["text"], "Anvil");
    assert_eq!(bundle["mention"]["document"], document);
    assert_eq!(bundle["mention"]["cut_before"], true);
    // The list names a mention by its document and its place there, as
    // an explanation does, and the name finds the record.
    let found = shown(&server, &["show", "records", "--document", document]);
    has(&found, "notes:a.txt[105-110]  anvil  -");
    assert_eq!(
        shown(&server, &["show", "records", "notes:a.txt[105-110]"]),
        record
    );
    let (code, complaint) = refused(&server, &["show", "records", "notes:a.txt[1-2]"]);
    assert_eq!(code, Some(1));
    assert_eq!(
        complaint,
        "error: there is no record with key `a.txt[1-2]` from source `notes`\n"
    );

    // Evidence that points outside the document is shown as it is.
    let evidence = json!({ "document": document, "start": 5, "end": 5000 });
    server.call(
        "/v1/load",
        json!({
            "source": "notes",
            "entity_type": "thing",
            "rows": [{ "id": "m2", "code": "tongs", "where": evidence.to_string() }],
        }),
    );
    let record = shown(&server, &["show", "records", "notes:m2"]);
    has(&record, "  end       5000");
    assert!(!record.contains("in document"), "{record}");

    // A document's mentions are listed in the order of its text. The key an
    // extractor gives one begins with the ID of the document, and by those
    // keys `[105-110]` would come before `[12-17]`.
    let evidence = json!({ "document": document, "key": "a.txt", "start": 12, "end": 17 });
    server.call(
        "/v1/load",
        json!({
            "source": "notes",
            "entity_type": "thing",
            "rows": [{ "id": "a-first", "code": "xs", "where": evidence.to_string() }],
        }),
    );
    let listed = shown(&server, &["show", "records", "--source", "notes"]);
    let names: Vec<&str> = listed
        .lines()
        .skip(1)
        .take(3)
        .map(|line| line.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(
        names,
        ["notes:a.txt[12-17]", "notes:a.txt[105-110]", "notes:m2"],
        "{listed}"
    );

    // A mention is named by its document and its place in it, when its
    // evidence says both.
    let explanation = shown(&server, &["explain", "notes:m1", "notes:m2"]);
    let summary = explanation.lines().next().unwrap();
    assert!(summary.contains("`notes:a.txt[105-110]`"), "{explanation}");
    assert!(summary.contains("`notes:m2`"), "{explanation}");
}

#[test]
fn a_mention_named_by_its_place_is_the_latest_there_is() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    // Four versions of a document, each with a mention at the same place,
    // and a fifth with none. The IDs of documents say nothing of their
    // order: they are hashes of the text.
    let mention = |row: &str, code: &str, document: &str| {
        let evidence = json!({ "document": document, "key": "a.txt", "start": 4, "end": 9 });
        let response = server.post(
            "/v1/load",
            json!({
                "source": "notes",
                "entity_type": "thing",
                "rows": [{ "id": row, "code": code, "where": evidence.to_string() }],
            }),
        );
        assert_eq!(response.ok()["run"]["status"], "succeeded");
    };
    let mut versions = Vec::new();
    for number in 1..=5 {
        let text = match number {
            5 => "Nothing is here any more.".to_owned(),
            number => format!("The anvil, as it was in version {number}."),
        };
        server.call(
            "/v1/documents",
            json!({ "source": "notes", "documents": [{ "key": "a.txt", "text": text }] }),
        );
        let latest = server.read("/v1/documents?source=notes&key=a.txt&latest=true");
        let document = latest["documents"][0]["id"].as_str().unwrap().to_owned();
        if number < 5 {
            mention(&format!("m{number}"), &format!("v{number}"), &document);
        }
        versions.push(document);
    }
    let mut sorted = versions[..4].to_vec();
    sorted.sort();
    assert_ne!(
        sorted[0], versions[3],
        "the test needs the fourth version not to have the lowest ID"
    );

    // The name is that of the mention in the latest version that has one.
    let name = "notes:a.txt[4-9]";
    let record = shown(&server, &["show", "records", name]);
    has(&record, "key          m4");
    has(&record, "  code    v4");
    assert_eq!(record, shown(&server, &["show", "records", "notes:m4"]));

    // A mention that was found again with other values is a version of the
    // same record, and the name is the latest of them, as a key is.
    mention("m4", "v4 again", &versions[3]);
    let record = shown(&server, &["show", "records", name]);
    has(&record, "key          m4");
    has(&record, "  code    v4 again");
    let explanation = shown(&server, &["explain", name, "notes:m1"]);
    has(&explanation, "code v1 v4 again");

    // Two records at one place in one version are two, and the name does
    // not choose.
    mention("other", "tongs", &versions[3]);
    let (code, complaint) = refused(&server, &["show", "records", name]);
    assert_eq!(code, Some(1));
    let (first, second) = (
        id_of(&server, "notes", "m4"),
        id_of(&server, "notes", "other"),
    );
    assert!(
        complaint
            .starts_with("error: 2 records are named `notes:a.txt[4-9]`: name one by its ID, of "),
        "{complaint}"
    );
    assert!(complaint.contains(&second), "{complaint}");
    assert!(
        !complaint.contains(&first),
        "the version that was replaced is not one of them"
    );
}

#[test]
fn a_source_and_key_name_the_latest_version_however_many_there_were() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    // More versions of one row than a page of records holds. They are
    // listed by ID, which says nothing of their order in time.
    let versions = 250;
    for version in 1..=versions {
        let code = format!("anvil-{version}");
        load(
            &server,
            "shop",
            "thing",
            rows(&[&["id", "code"], &["1", &code]]),
        );
    }
    load(
        &server,
        "shop",
        "thing",
        rows(&[&["id", "code"], &["2", "anvil-250"]]),
    );
    let first = server.read("/v1/records?source=shop&key=1");
    assert_eq!(first["records"].as_array().unwrap().len(), 100);
    assert!(
        first["records"]
            .as_array()
            .unwrap()
            .iter()
            .all(|version| !version["superseded_by"].is_null()),
        "the latest version is not on the first page, or this tests nothing"
    );

    let latest = answered(&server, &["show", "records", "shop:1"]);
    assert_eq!(latest["record"]["payload"]["code"], "anvil-250");
    assert_eq!(latest["record"]["superseded_by"], Value::Null);

    resolve(&server, "thing");
    let explanation = answered(&server, &["explain", "shop:1", "shop:2"]);
    assert_eq!(explanation["outcome"], "same_identity");
    let sides = [&explanation["left"], &explanation["right"]];
    assert!(
        sides
            .iter()
            .all(|side| side["record"]["payload"]["code"] == "anvil-250"),
        "{explanation}"
    );
}

#[test]
#[ignore = "starts uv, which downloads packages the first time"]
fn what_an_extractor_found_is_shown_and_explained() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let extractor = project.join("extractors/words.py");
    std::fs::create_dir_all(extractor.parent().unwrap()).unwrap();
    std::fs::write(extractor, WORDS).unwrap();
    let manifest = build(&project);
    let server = Server::start(&[
        "serve",
        "--manifest",
        path(&manifest),
        "--project",
        path(&project),
        "--tasks",
        path(&dir.path().join("tasks")),
        "--store",
        path(&dir.path().join("store.duckdb")),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ]);
    server.call(
        "/v1/documents",
        json!({ "source": "notes", "documents": [
            { "key": "a.txt", "text": "the Anvil rang" },
            { "key": "b.txt", "text": "an Anvil, again" },
        ] }),
    );
    let run = server.call("/v1/extractions", json!({ "extractor": "words" }))["run"].clone();
    assert_eq!(run["status"], "succeeded", "{run}");
    resolve(&server, "thing");

    let overview = shown(&server, &["show"]);
    has(&overview, "documents  2");
    has(&overview, "extractor `words` (python, extractors/words.py)");
    let extraction = shown(&server, &["show", "runs", &run["run"].to_string()]);
    has(&extraction, "kind       extract");
    has(&extraction, "documents  2 read");
    has(
        &extraction,
        "mentions   2 found: 2 new, 0 known, 0 new versions of a known mention",
    );
    let documents = shown(&server, &["show", "documents"]);
    assert!(
        documents.contains(&format!("  words in run {} (1 mention)\n", run["run"])),
        "{documents}"
    );

    let mentions = server.read("/v1/records?source=notes");
    let [first, second] = &mentions["records"].as_array().unwrap()[..] else {
        panic!("two mentions: {mentions}");
    };
    let record = shown(&server, &["show", "records", first["id"].as_str().unwrap()]);
    has(&record, "  extractor  words");
    assert!(
        record.contains("\n  the [Anvil] rang\n") || record.contains("\n  an [Anvil], again\n"),
        "{record}"
    );

    // A mention is named by its document and its place in it.
    let explanation = shown(
        &server,
        &[
            "explain",
            first["id"].as_str().unwrap(),
            second["id"].as_str().unwrap(),
        ],
    );
    let first_line = explanation.lines().next().unwrap();
    assert!(
        first_line.contains("`notes:a.txt[4-9]`") && first_line.contains("`notes:b.txt[3-8]`"),
        "{explanation}"
    );
    assert!(
        first_line.contains("belong to the same identity"),
        "{explanation}"
    );
}
