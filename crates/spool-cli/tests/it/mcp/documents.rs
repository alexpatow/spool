//! Documents loaded, and an extractor run, through tools.

use super::*;

#[test]
fn documents_are_loaded_through_a_tool() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());

    let documents = json!([
        { "key": "a.txt", "text": "Anvil and Hammer.", "uri": "https://notes.test/a" },
        { "key": "b.txt", "text": "Hammer again." },
        { "key": "a.txt", "text": "Anvil and Hammer.", "uri": "https://notes.test/a" },
    ]);
    let load = json!({ "source": "notes", "documents": documents });
    let loaded = tool(&server, "load_documents", load.clone());
    assert_eq!(loaded["run"]["kind"], "load", "{loaded}");
    assert_eq!(loaded["run"]["status"], "succeeded", "{loaded}");
    assert_eq!(loaded["run"]["load"]["received"], 3, "{loaded}");
    assert_eq!(loaded["run"]["load"]["inserted"], 2, "{loaded}");
    assert_eq!(loaded["run"]["load"]["duplicates"], 1, "{loaded}");
    let run = loaded["run"]["run"].as_u64().unwrap();
    assert_eq!(tool(&server, "get_run", json!({ "run": run })), loaded);
    // The same documents again add nothing, and other text under a key is
    // the next version of that document.
    let again = tool(&server, "load_documents", load);
    assert_eq!(again["run"]["load"]["inserted"], 0, "{again}");
    let changed = json!([{ "key": "b.txt", "text": "Hammer once more." }]);
    let changed = json!({ "source": "notes", "documents": changed });
    let changed = tool(&server, "load_documents", changed);
    assert_eq!(changed["run"]["load"]["superseded"], 1, "{changed}");
    let held = server.read("/v1/documents?latest=true")["documents"].clone();
    let latest = || server.read("/v1/documents?latest=true")["documents"].clone();
    let held = held.as_array().unwrap();
    assert_eq!(held.len(), 2, "{held:?}");
    let a = held.iter().find(|document| document["key"] == "a.txt");
    assert_eq!(a.unwrap()["uri"], "https://notes.test/a");
    // Storing a document finds nothing in it.
    assert_eq!(
        server.read("/v1/info")["store"]["entity_types"][0]["records"],
        0
    );

    // What the API refuses of a document, the tool refuses, and a field
    // the tool does not have is refused as well.
    let unkeyed = json!({ "source": "notes", "documents": [{ "key": "", "text": "x" }] });
    let unkeyed = refused(&server, "load_documents", unkeyed, "invalid");
    assert!(
        text(&unkeyed["message"]).contains("document 1: it has no key"),
        "{unkeyed}"
    );
    // Documents of a source the project does not declare would never
    // become records, so they are not stored.
    let undeclared = json!({ "source": "letters", "documents": [{ "key": "c", "text": "x" }] });
    let undeclared = refused(&server, "load_documents", undeclared, "invalid");
    assert!(
        text(&undeclared["message"])
            .starts_with("source `letters` is not declared: the project's sources are `"),
        "{undeclared}"
    );
    for arguments in [
        json!({ "source": "notes", "documents": [{ "key": "c", "text": "x", "title": "y" }] }),
        json!({ "source": "notes", "documents": [{ "key": "c" }] }),
        json!({ "source": "notes", "documents": [], "actor": "user:someone" }),
        json!({ "documents": [] }),
    ] {
        refused(&server, "load_documents", arguments, "invalid");
    }

    // The server runs the project's extractors and no other.
    let runs = server.read("/v1/runs")["runs"].as_array().unwrap().len();
    let unknown = json!({ "extractor": "words" });
    let unknown = refused(&server, "run_extraction", unknown, "invalid");
    assert!(
        text(&unknown["message"]).contains("the project has no extractor `words`"),
        "{unknown}"
    );
    let code = json!({ "extractor": "words", "code": "import os" });
    refused(&server, "run_extraction", code, "invalid");
    // With none named, the project's only extractor runs, and this one has
    // none.
    let none = refused(&server, "run_extraction", json!({}), "invalid");
    assert!(
        text(&none["message"]).starts_with("the project has no extractors"),
        "{none}"
    );
    assert_eq!(latest().as_array().unwrap().len(), 2);
    assert_eq!(
        server.read("/v1/runs")["runs"].as_array().unwrap().len(),
        runs
    );
}

fn word_server(dir: &Path) -> Server {
    let project = dir.join("project");
    write_things(&project);
    std::fs::create_dir_all(project.join("extractors")).unwrap();
    std::fs::write(project.join("extractors/words.py"), WORDS).unwrap();
    let manifest = build(&project);
    Server::start(&[
        "serve",
        "--manifest",
        path(&manifest),
        "--project",
        path(&project),
        "--tasks",
        path(&dir.join("tasks")),
        "--store",
        path(&dir.join("store.duckdb")),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ])
}

#[test]
#[ignore = "runs uv"]
fn adding_one_document_returns_its_placements_and_leaves_other_inputs_pending() {
    let dir = tempfile::tempdir().unwrap();
    let server = word_server(dir.path());
    let add_row = |id, code, colour| {
        server.call(
            "/v1/records",
            json!({
                "source": "shop", "entity_type": "thing",
                "row": { "id": id, "code": code, "colour": colour },
            }),
        )
    };
    let black = add_row("black", "anvil", "black");
    let red = add_row("red", "anvil", "red");
    let hammer = add_row("hammer", "hammer", "red");
    tool(
        &server,
        "load_rows",
        json!({
            "source": "shop", "entity_type": "thing",
            "rows": [{ "id": "pending", "code": "hammer", "colour": "red" }],
        }),
    );
    tool(
        &server,
        "load_documents",
        json!({
            "source": "notes", "documents": [{ "key": "pending.txt", "text": "Other." }],
        }),
    );

    let request = json!({
        "source": "notes",
        "document": { "key": "a.txt", "text": "🛠 Hammer Anvil Wrench Wrench." },
    });
    let added = tool(&server, "add_document", request.clone());
    let mentions = added["records"].as_array().unwrap();
    assert_eq!(mentions.len(), 4, "{added}");
    assert_eq!(mentions[0]["start"], 2);
    assert_eq!(mentions[0]["end"], 8);
    assert_eq!(mentions[0]["identity"], hammer["identity"]);
    assert!(mentions[0]["review"].is_null());
    assert!(mentions[1]["identity"].is_null());
    let review = &mentions[1]["review"];
    assert_eq!(review["kind"], "possible_merge");
    let candidates = review["identities"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.contains(&black["identity"]));
    assert!(candidates.contains(&red["identity"]));
    assert_ne!(mentions[2]["record"], mentions[3]["record"]);
    assert!(mentions[2]["identity"].is_string());
    assert_eq!(mentions[2]["identity"], mentions[3]["identity"]);

    // An identical retry through HTTP reuses the extraction and placements.
    assert_eq!(server.call("/v1/documents/add", request.clone()), added);
    let doc = server.read(&format!("/v1/documents/{}", text(&added["document"])));
    assert_eq!(doc["id"], added["document"]);
    assert_eq!(doc["extractions"].as_array().unwrap().len(), 1);
    assert!(
        server.read("/v1/documents?source=notes&key=pending.txt")["documents"][0]["extractions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(server.read("/v1/records?source=shop&key=pending")["records"][0]["identity"].is_null());

    // The CLI sends a new version under the file name, here with no mentions.
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "no mentions here").unwrap();
    let answer = support::succeed(&[
        "add-document",
        "--source",
        "notes",
        path(&file),
        "--json",
        "--url",
        &format!("http://{}", server.address),
        "--token",
        server.token.as_deref().unwrap(),
    ]);
    let empty: Value = serde_json::from_str(&answer).unwrap();
    assert_ne!(empty["document"], added["document"]);
    assert_eq!(empty["records"], json!([]));
    let restored_text = server.call("/v1/documents/add", request);
    assert_ne!(restored_text["document"], added["document"]);
    assert_eq!(restored_text["records"].as_array().unwrap().len(), 4);

    let failed = json!({
        "source": "notes", "document": { "key": "failed.txt", "text": "unreadable" },
    });
    for _ in 0..2 {
        let error = server
            .post("/v1/documents/add", failed.clone())
            .error(500, "extractor_failed");
        assert!(error.contains("cannot read this text"), "{error}");
    }
    let failed = server.read("/v1/documents?source=notes&key=failed.txt");
    assert_eq!(failed["documents"].as_array().unwrap().len(), 1);
    assert_eq!(
        failed["documents"][0]["extractions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
#[ignore = "runs uv"]
fn an_extractor_is_run_through_a_tool_and_what_it_finds_is_matched() {
    let dir = tempfile::tempdir().unwrap();
    let server = word_server(dir.path());

    // The store names the extractor before a document is loaded.
    let described = tool(&server, "describe_store", json!({}));
    assert_eq!(described["store"]["extractors"][0]["name"], "words");

    let rows = json!([{ "id": "1", "code": "hammer", "colour": "red" }]);
    let rows = json!({ "source": "shop", "entity_type": "thing", "rows": rows });
    tool(&server, "load_rows", rows);
    tool(&server, "run_match", json!({ "entity_type": "thing" }));
    let documents = json!([
        { "key": "a.txt", "text": "Anvil and Hammer." },
        { "key": "b.txt", "text": "Hammer again." },
    ]);
    let documents = json!({ "source": "notes", "documents": documents });
    tool(&server, "load_documents", documents);

    // The call is an extract run, as the API answers it, and what it found
    // is records with no identity yet. The project's only extractor need
    // not be named.
    let extracted = tool(&server, "run_extraction", json!({}));
    let run = extracted["run"]["run"].as_u64().unwrap();
    assert_eq!(extracted, server.read(&format!("/v1/runs/{run}")));
    assert_eq!(tool(&server, "get_run", json!({ "run": run })), extracted);
    assert_eq!(extracted["run"]["kind"], "extract", "{extracted}");
    assert_eq!(extracted["run"]["status"], "succeeded", "{extracted}");
    assert_eq!(extracted["run"]["extractor"]["name"], "words");
    let report = &extracted["run"]["extraction"];
    assert_eq!(report["documents"], 2, "{extracted}");
    assert_eq!(report["mentions"], 3, "{extracted}");
    assert_eq!(report["inserted"], 3, "{extracted}");
    let found = tool(&server, "search_records", json!({ "source": "notes" }));
    let found = found["records"].as_array().unwrap().clone();
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(found.iter().all(|record| record["identity"].is_null()));

    // A match run places them as it places rows: the hammers of the two
    // notes join the hammer the shop sent.
    let matched = tool(&server, "run_match", json!({ "entity_type": "thing" }));
    let resolution = &matched["run"]["resolution"];
    assert_eq!(resolution["new_records"], 3, "{matched}");
    assert_eq!(resolution["identities_created"], 1, "{matched}");
    assert_eq!(resolution["records_assigned"], 3, "{matched}");
    let hammer = found
        .iter()
        .find(|record| record["evidence"]["key"] == "b.txt")
        .unwrap();
    let key = text(&hammer["key"]);
    let pair = json!({ "a": "shop:1", "b": format!("notes:{key}") });
    let explained = tool(&server, "explain_pair", pair);
    assert_eq!(explained["outcome"], "same_identity", "{explained}");
    // A mention is also named by its place in the document, as the lists of
    // `spool show records` name it.
    let evidence = &hammer["evidence"];
    let place = format!("notes:b.txt[{}-{}]", evidence["start"], evidence["end"]);
    let placed = json!({ "a": place, "b": "shop:1" });
    assert_eq!(tool(&server, "explain_pair", placed), explained);
    let nowhere = json!({ "a": "notes:b.txt[900-906]", "b": "shop:1" });
    refused(&server, "explain_pair", nowhere, "not_found");

    // Documents the extractor has read are not read again unless asked.
    let nothing = tool(&server, "run_extraction", json!({ "extractor": "words" }));
    assert_eq!(nothing["run"]["extraction"]["documents"], 0, "{nothing}");
    let again = json!({ "extractor": "words", "source": "notes", "again": true, "limit": 1 });
    let again = tool(&server, "run_extraction", again);
    assert_eq!(again["run"]["extraction"]["documents"], 1, "{again}");
    assert_eq!(again["run"]["extraction"]["inserted"], 0, "{again}");

    // What an extractor found is taken back as a row is, by its key.
    let taken = json!({
        "source": "notes",
        "entity_type": "thing",
        "keys": [key],
        "reason": "the note was about another hammer",
    });
    let retracted = tool(&server, "retract_rows", taken);
    assert_eq!(
        retracted["run"]["retraction"]["retracted"], 1,
        "{retracted}"
    );
    let record = tool(&server, "get_record", json!({ "record": hammer["id"] }));
    assert!(record["identity"].is_null(), "{record}");
}
