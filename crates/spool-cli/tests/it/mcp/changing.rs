//! Tools that change something, held to the rules of the API.

use super::*;

#[test]
fn a_tool_that_changes_something_is_held_to_the_rules_of_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path());
    let before = head(&server);
    let identities = server.read("/v1/identities?q=atlas");
    let [from, into] = [0, 1].map(|index| text(&identities["identities"][index]["id"]).to_owned());
    let merge = json!({ "op": "merge", "from": from, "into": into });

    // A caller does not name the author of a change.
    for name in ["actor", "author"] {
        let mut named = json!({ "ops": [merge], "reason": "the same company" });
        named[name] = json!("user:someone-else");
        let error = refused(&server, "commit_changeset", named, "invalid");
        assert!(text(&error["message"]).contains(name), "{error}");
    }
    // A change says why.
    refused(
        &server,
        "commit_changeset",
        json!({ "ops": [merge] }),
        "invalid",
    );
    // A changeset the server rejects says which operation broke which rule.
    let itself = json!({ "op": "merge", "from": into, "into": into });
    let both = json!({ "ops": [merge, itself], "reason": "the same company" });
    let rejected = refused(&server, "commit_changeset", both, "rejected");
    assert_eq!(rejected["rejections"][0]["op_index"], 1, "{rejected}");
    assert_eq!(
        rejected["rejections"][0]["reason"]["reason"],
        "merge_into_itself"
    );
    // None of that changed anything: a changeset is committed whole or not
    // at all.
    assert_eq!(head(&server), before);

    let committed = tool(
        &server,
        "commit_changeset",
        json!({ "ops": [merge], "reason": "the same company", "base": before }),
    );
    assert_eq!(committed["seq"], before + 1);
    let logged = entry(&server, before + 1);
    assert_eq!(logged["changeset"]["actor"], "agent:api");
    assert_eq!(logged["changeset"]["reason"], "the same company");
    assert_eq!(logged["changeset"]["base"], before);
    assert_eq!(logged["changeset"]["ops"], json!([merge]));
    let merged = tool(&server, "get_identity", json!({ "identity": from }));
    assert_eq!(merged["current"], into.as_str());
    assert_eq!(merged["members"].as_array().unwrap().len(), 4);

    // A change made on what has since changed is refused, as on the API.
    let others = server.read("/v1/identities?q=halden");
    let other = text(&others["identities"][0]["id"]);
    let stale = json!({
        "ops": [{ "op": "merge", "from": other, "into": into }],
        "reason": "read before the merge",
        "base": before,
    });
    refused(&server, "commit_changeset", stale, "rejected");

    // Undoing shows what it would commit, then commits it.
    let undo = json!({ "seq": before + 1, "reason": "two companies after all", "commit": false });
    let shown = tool(&server, "undo_changeset", undo);
    assert!(shown["committed"].is_null(), "{shown}");
    assert_eq!(shown["changeset"]["ops"][0]["op"], "reinstate");
    assert_eq!(head(&server), before + 1);
    let undo = json!({ "seq": before + 1, "reason": "two companies after all" });
    let undone = tool(&server, "undo_changeset", undo);
    assert_eq!(undone["committed"]["seq"], before + 2);
    let logged = entry(&server, before + 2);
    assert_eq!(logged["changeset"]["actor"], "agent:api");
    assert_eq!(logged["changeset"]["reason"], "two companies after all");
    let restored = tool(&server, "get_identity", json!({ "identity": from }));
    assert_eq!(restored["current"], from.as_str());
    assert_eq!(restored["members"].as_array().unwrap().len(), 2);
    refused(
        &server,
        "undo_changeset",
        json!({ "seq": 99, "reason": "no such change" }),
        "not_found",
    );

    // A split moves the records named to a new identity and keeps the two
    // apart.
    let acorn = server.read("/v1/identities?q=acorn");
    let acorn = text(&acorn["identities"][0]["id"]);
    let members = tool(&server, "get_identity", json!({ "identity": acorn }));
    let moved = text(&members["members"][0]["record"]);
    let split = tool(
        &server,
        "split_identity",
        json!({ "identity": acorn, "records": [moved], "reason": "another Acorn", "base": before + 2 }),
    );
    assert_eq!(split["committed"]["seq"], before + 3, "{split}");
    let logged = entry(&server, before + 3);
    assert_eq!(logged["changeset"]["actor"], "agent:api");
    assert_eq!(logged["changeset"]["reason"], "another Acorn");
    let record = tool(&server, "get_record", json!({ "record": moved }));
    assert_ne!(record["identity"], acorn);
    let apart = tool(&server, "list_constraints", json!({ "record": moved }));
    assert_eq!(apart["constraints"].as_array().unwrap().len(), 1, "{apart}");
    assert_eq!(apart["constraints"][0]["kind"], "cannot_link");
    // The server holds a later change to what was decided.
    let back = json!({
        "ops": [{ "op": "assign", "record": moved, "from": record["identity"], "to": acorn }],
        "reason": "put it back",
    });
    let held = refused(&server, "commit_changeset", back, "rejected");
    assert_eq!(held["rejections"][0]["op_index"], 0, "{held}");
    let nothing = json!({ "identity": acorn, "records": [], "reason": "nothing to move" });
    refused(&server, "split_identity", nothing, "conflict");

    dismiss_and_mint(&server);
    assert_eq!(head(&server), before + 3);
}

/// A review item dismissed with who dismissed it and why, and IDs minted a
/// few at a time, neither of which is a change to the log.
fn dismiss_and_mint(server: &Server) {
    // A review item is closed with who closed it and why.
    refused(
        server,
        "dismiss_review_item",
        json!({ "item": 1 }),
        "invalid",
    );
    let dismissed = tool(
        server,
        "dismiss_review_item",
        json!({ "item": 1, "reason": "too little to go on" }),
    );
    assert_eq!(dismissed["status"], "dismissed", "{dismissed}");
    assert_eq!(dismissed["dismissed"]["by"], "agent:api");
    assert_eq!(dismissed["dismissed"]["reason"], "too little to go on");
    assert_eq!(
        tool(server, "list_review_items", json!({})),
        json!({ "items": [], "next": null })
    );
    let again = json!({ "item": 1, "reason": "again" });
    refused(server, "dismiss_review_item", again, "conflict");

    // IDs are minted a few at a time.
    let minted = tool(server, "mint_identity_ids", json!({}));
    assert_eq!(minted["identities"].as_array().unwrap().len(), 1);
    assert_eq!(minted["changesets"], json!([]));
    refused(
        server,
        "mint_identity_ids",
        json!({ "count": 101 }),
        "invalid",
    );
}

#[test]
fn rows_are_loaded_matched_and_taken_back_through_tools() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    let loaded = load_through_a_tool(&server);
    match_through_a_tool(&server, &loaded);
    explain_and_take_back_through_tools(&server);
    replace_rows_through_a_tool(&server);
}

fn count_runs(server: &Server) -> usize {
    server.read("/v1/runs")["runs"].as_array().unwrap().len()
}

/// Rows loaded through `load_rows`, as the API loads them, and the run that
/// loaded them read by `get_run`.
fn load_through_a_tool(server: &Server) -> Value {
    // Before a row is loaded, the store says which sources the project
    // declares and the columns of each.
    let described = tool(server, "describe_store", json!({}));
    assert_eq!(described, server.read("/v1/info"));
    let declared = &described["store"]["sources"];
    assert_eq!(declared.as_array().unwrap().len(), 13, "{declared}");
    let shop = json!({
        "name": "shop",
        "key": "id",
        "evidence": null,
        "columns": ["id", "code", "colour", "name", "weight"],
        "entity_types": ["thing"],
    });
    assert!(declared.as_array().unwrap().contains(&shop), "{declared}");

    // Loading stores the rows and matches nothing.
    let rows = json!([
        { "id": "1", "code": "anvil", "colour": "black" },
        { "id": "2", "code": "anvil", "colour": "black" },
        { "id": "3", "code": "anvil", "colour": "red" },
        { "id": "4", "code": "hammer", "colour": "red" },
    ]);
    let load = json!({ "source": "shop", "entity_type": "thing", "rows": rows });
    let loaded = tool(server, "load_rows", load.clone());
    assert_eq!(loaded["run"]["kind"], "load", "{loaded}");
    assert_eq!(loaded["run"]["status"], "succeeded", "{loaded}");
    assert_eq!(loaded["run"]["load"]["inserted"], 4, "{loaded}");
    assert_eq!(
        server.read("/v1/records?unassigned=true")["records"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(head(server), 0);
    // The same rows again add nothing.
    let again = tool(server, "load_rows", load);
    assert_eq!(again["run"]["load"]["inserted"], 0, "{again}");
    assert_eq!(again["run"]["load"]["duplicates"], 4, "{again}");
    // What the API refuses of a load, the tool refuses.
    for (source, entity_type, row, why) in [
        (
            "shop",
            "thing",
            json!({ "id": "5", "shape": "round" }),
            "a column `shape`",
        ),
        (
            "warehouse",
            "thing",
            json!({ "id": "5" }),
            "`thing` takes rows from `",
        ),
        (
            "shop",
            "widget",
            json!({ "id": "5" }),
            "`widget` is not declared",
        ),
    ] {
        let load = json!({ "source": source, "entity_type": entity_type, "rows": [row] });
        let error = refused(server, "load_rows", load, "invalid");
        assert!(text(&error["message"]).contains(why), "{error}");
    }
    refused(
        server,
        "load_rows",
        json!({ "source": "shop", "entity_type": "thing", "rows": [], "actor": "user:someone" }),
        "invalid",
    );
    // A refused load stores nothing.
    let held = server.read("/v1/info")["store"]["entity_types"][0]["records"].clone();
    assert_eq!(held, 4);

    // A run is read by its number, as the API reads it.
    let run = loaded["run"]["run"].as_u64().unwrap();
    assert_eq!(tool(server, "get_run", json!({ "run": run })), loaded);
    assert_eq!(
        tool(server, "get_run", json!({ "run": run, "wait_ms": 500 })),
        server.read(&format!("/v1/runs/{run}"))
    );
    refused(server, "get_run", json!({ "run": 99 }), "not_found");
    loaded
}

/// A match run started through `run_match`, and what it refuses to start.
fn match_through_a_tool(server: &Server, loaded: &Value) {
    // A match run places what was loaded and commits it.
    let matched = tool(server, "run_match", json!({ "entity_type": "thing" }));
    let resolution = &matched["run"]["resolution"];
    assert_eq!(matched["run"]["kind"], "match", "{matched}");
    assert_eq!(matched["run"]["status"], "succeeded", "{matched}");
    assert_eq!(resolution["new_records"], 4, "{matched}");
    assert_eq!(resolution["identities_created"], 3, "{matched}");
    assert_eq!(resolution["committed"]["seq"], 1, "{matched}");
    assert!(resolution["draft"].is_null(), "{matched}");
    assert_eq!(head(server), 1);
    // A run says who started it. What a match run commits is the engine's
    // in the log, which says who asked beside it.
    assert_eq!(loaded["run"]["started_by"], "agent:api", "{loaded}");
    assert_eq!(matched["run"]["started_by"], "agent:api", "{matched}");
    let placed = tool(server, "list_changesets", json!({ "limit": 1 }));
    assert_eq!(placed["changesets"][0]["changeset"]["actor"], "engine");
    assert_eq!(placed["changesets"][0]["started_by"], "agent:api");
    let run = matched["run"]["run"].as_u64().unwrap();
    assert_eq!(tool(server, "get_run", json!({ "run": run })), matched);
    // With nothing new, no run is started.
    let nothing = tool(server, "run_match", json!({ "entity_type": "thing" }));
    assert_eq!(nothing, json!({ "run": null }));
    // The project's rules are a strategy by name, and a rematch judges
    // what has an identity again and moves nothing.
    let rematch = json!({ "entity_type": "thing", "strategy": "rules", "rematch": true });
    let rematched = tool(server, "run_match", rematch);
    let resolution = &rematched["run"]["resolution"];
    assert_eq!(resolution["candidates"], 3, "{rematched}");
    assert_eq!(
        resolution["suggestions"],
        json!({ "merges": [], "splits": [] })
    );
    assert!(resolution["committed"].is_null(), "{rematched}");
    assert_eq!(head(server), 1);
    // A draft is not left, since no tool approves one, and a strategy the
    // project does not have is not started, since no tool sends judgements.
    let before = count_runs(server);
    for arguments in [
        json!({ "entity_type": "thing", "commit": false }),
        json!({ "entity_type": "thing", "strategy": "mine" }),
        json!({ "entity_type": "widget" }),
    ] {
        refused(server, "run_match", arguments, "invalid");
    }
    assert_eq!(count_runs(server), before);
    assert!(
        server.read("/v1/drafts")["drafts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// A pair explained by source and key, and a row taken back and loaded again.
fn explain_and_take_back_through_tools(server: &Server) {
    // A pair is explained by source and key, as by ID.
    let id = |key: &str| {
        let found = server.read(&format!("/v1/records?source=shop&key={key}"));
        text(&found["records"][0]["id"]).to_owned()
    };
    let by_key = tool(
        server,
        "explain_pair",
        json!({ "a": "shop:1", "b": "shop:3" }),
    );
    assert_eq!(by_key["outcome"], "different_identities", "{by_key}");
    assert_eq!(
        by_key,
        tool(
            server,
            "explain_pair",
            json!({ "a": id("1"), "b": id("3") })
        )
    );
    assert_eq!(by_key, server.read("/v1/explain?a=shop:1&b=shop:3"));
    let unknown = json!({ "a": "shop:1", "b": "shop:9" });
    refused(server, "explain_pair", unknown, "not_found");
    let unnamed = refused(
        server,
        "explain_pair",
        json!({ "a": "1", "b": "shop:3" }),
        "invalid",
    );
    assert!(
        text(&unnamed["message"]).contains("<source>:<key>"),
        "{unnamed}"
    );

    // Taking a row back says why, and is recorded under the caller. The
    // row stays, out of its identity.
    let keys = json!({ "source": "shop", "entity_type": "thing", "keys": ["2", "9"] });
    refused(server, "retract_rows", keys.clone(), "invalid");
    let mut taken = keys;
    taken["reason"] = json!("loaded by mistake");
    let retracted = tool(server, "retract_rows", taken);
    let retraction = &retracted["run"]["retraction"];
    assert_eq!(retracted["run"]["kind"], "retract", "{retracted}");
    assert_eq!(retraction["retracted"], 1, "{retracted}");
    assert_eq!(retraction["unknown"], 1, "{retracted}");
    assert_eq!(retraction["committed"]["seq"], 2, "{retracted}");
    let entry = entry(server, 2);
    assert_eq!(entry["changeset"]["actor"], "agent:api", "{entry}");
    assert_eq!(entry["changeset"]["reason"], "loaded by mistake", "{entry}");
    let record = tool(server, "get_record", json!({ "record": id("2") }));
    assert!(record["identity"].is_null(), "{record}");
    assert_eq!(record["retracted"], retracted["run"]["run"], "{record}");
    // Loading the row again ends the retraction.
    let row = json!({ "id": "2", "code": "anvil", "colour": "black" });
    let back = json!({ "source": "shop", "entity_type": "thing", "rows": [row] });
    let back = tool(server, "load_rows", back);
    assert_eq!(back["run"]["load"]["restored"], 1, "{back}");
    let record = tool(server, "get_record", json!({ "record": id("2") }));
    assert!(record["identity"].is_string(), "{record}");
    assert!(record["retracted"].is_null(), "{record}");
}

/// A row that would replace one the store holds, refused unless the call
/// says to replace it.
fn replace_rows_through_a_tool(server: &Server) {
    // A row under a key the store holds, with other values, would replace
    // that row. The tool refuses it unless the call says to, and the rows
    // that came with it are not loaded either.
    let held = || server.read("/v1/info")["store"]["entity_types"][0]["records"].clone();
    let before = (held(), count_runs(server));
    let rows = json!([
        { "id": "4", "code": "hammer", "colour": "blue" },
        { "id": "1", "code": "anvil", "colour": "black" },
        { "id": "7", "code": "tongs", "colour": "black" },
        { "id": "3", "code": "anvil", "colour": "green" },
    ]);
    let mut change = json!({ "source": "shop", "entity_type": "thing", "rows": rows });
    let error = refused(server, "load_rows", change.clone(), "invalid");
    assert_eq!(
        error["message"],
        "load rejected: 2 rows have keys the store holds with other values: `3`, `4`. \
         Nothing was loaded, because this load was not to replace rows. If the rows are \
         meant to change, load again with `replace` set to true"
    );
    change["replace"] = json!(false);
    refused(server, "load_rows", change.clone(), "invalid");
    assert_eq!(held(), before.0);
    // Each refused load is a run that failed.
    assert_eq!(count_runs(server), before.1 + 2);
    let by_key = server.read("/v1/records?source=shop&key=4");
    assert_eq!(by_key["records"].as_array().unwrap().len(), 1, "{by_key}");
    change["replace"] = json!(true);
    let changed = tool(server, "load_rows", change);
    assert_eq!(changed["run"]["load"]["inserted"], 3, "{changed}");
    assert_eq!(changed["run"]["load"]["superseded"], 2, "{changed}");
    let by_key = server.read("/v1/records?source=shop&key=4");
    assert_eq!(by_key["records"].as_array().unwrap().len(), 2, "{by_key}");
    // The row as it was before is other than it is now, so loading it
    // again is refused the same way, and put back when the call says to.
    let row = json!({ "id": "4", "code": "hammer", "colour": "red" });
    let mut back = json!({ "source": "shop", "entity_type": "thing", "rows": [row] });
    let error = refused(server, "load_rows", back.clone(), "invalid");
    assert!(
        text(&error["message"]).starts_with("load rejected: 1 row has a key"),
        "{error}"
    );
    back["replace"] = json!(true);
    let put_back = tool(server, "load_rows", back);
    assert_eq!(put_back["run"]["load"]["superseded"], 1, "{put_back}");
    let found = tool(
        server,
        "search_records",
        json!({ "source": "shop", "key": "4" }),
    );
    let versions = found["records"].as_array().unwrap();
    assert_eq!(versions.len(), 3, "{found}");
    let current = versions
        .iter()
        .find(|version| version["superseded_by"].is_null())
        .unwrap();
    assert_eq!(current["payload"]["colour"], "red", "{found}");
}

#[test]
fn adding_one_record_places_it_or_returns_review_without_using_up_pending_rows() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    tool(
        &server,
        "load_rows",
        json!({
            "source": "shop", "entity_type": "thing",
            "rows": [{ "id": "pending", "code": "anvil", "colour": "black" }]
        }),
    );
    let add = |row| {
        tool(
            &server,
            "add_record",
            json!({
                "source": "shop", "entity_type": "thing", "row": row
            }),
        )
    };
    let black = add(json!({ "id": "black", "code": "anvil", "colour": "black" }));
    let red = add(json!({ "id": "red", "code": "anvil", "colour": "red" }));
    assert!(black["identity"].is_string(), "{black}");
    assert!(red["identity"].is_string(), "{red}");
    assert_ne!(black["identity"], red["identity"]);
    assert!(black["review"].is_null());
    let run = server.read(&format!("/v1/runs/{}", black["match_run"]));
    assert_eq!(run["run"]["resolution"]["new_records"], 1);
    assert!(server.read("/v1/records?source=shop&key=pending")["records"][0]["identity"].is_null());

    let batch = tool(&server, "run_match", json!({ "entity_type": "thing" }));
    assert_eq!(batch["run"]["resolution"]["new_records"], 1, "{batch}");
    assert_eq!(
        server.read("/v1/records?source=shop&key=pending")["records"][0]["identity"],
        black["identity"]
    );

    let row = json!({ "id": "either", "code": "anvil", "colour": "" });
    let either = add(row.clone());
    assert!(either["identity"].is_null(), "{either}");
    assert_eq!(either["review"]["kind"], "possible_merge");
    assert_eq!(either["review"]["status"], "open");
    let candidates = either["review"]["identities"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.contains(&black["identity"]));
    assert!(candidates.contains(&red["identity"]));
    let again = add(row);
    assert_eq!(again["record"], either["record"]);
    assert_eq!(again["review"]["id"], either["review"]["id"]);
    assert!(again["match_run"].is_null(), "{again}");
    assert_eq!(
        server.read("/v1/identities")["identities"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn an_empty_value_is_one_the_row_does_not_have() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    let load = |rows: Value| {
        let rows = json!({ "source": "shop", "entity_type": "thing", "rows": rows });
        tool(&server, "load_rows", rows)
    };
    // Two anvils of different colours are two things.
    load(json!([
        { "id": "1", "code": "anvil", "colour": "black" },
        { "id": "2", "code": "anvil", "colour": "red" },
    ]));
    let placed = tool(&server, "run_match", json!({ "entity_type": "thing" }));
    assert_eq!(placed["run"]["resolution"]["identities_created"], 2);

    // An empty field of a file comes as empty text. The row is stored
    // without the column, as the same line of CSV is.
    let loaded = load(json!([{ "id": "3", "code": "anvil", "colour": "", "name": "" }]));
    assert_eq!(loaded["run"]["load"]["inserted"], 1, "{loaded}");
    let found = tool(
        &server,
        "search_records",
        json!({ "source": "shop", "key": "3" }),
    );
    let record = &found["records"][0];
    assert_eq!(record["payload"], json!({ "id": "3", "code": "anvil" }));
    let csv = server
        .request("POST", "/v1/load?source=shop&entity_type=thing")
        .header("Content-Type", "text/csv")
        .body(b"id,code,colour,name\n3,anvil,,\n".as_slice())
        .send()
        .ok();
    assert_eq!(csv["run"]["load"]["duplicates"], 1, "{csv}");
    assert_eq!(csv["run"]["load"]["inserted"], 0, "{csv}");

    // So no rule reads a colour it does not have. As empty text it would
    // differ from black and from red, and the anvil would be a third thing
    // without a word. With no colour it matches both, which someone has to
    // decide.
    let matched = tool(&server, "run_match", json!({ "entity_type": "thing" }));
    let resolution = &matched["run"]["resolution"];
    assert_eq!(resolution["identities_created"], 0, "{matched}");
    assert_eq!(resolution["records_unassigned"], 1, "{matched}");
    let items = tool(&server, "list_review_items", json!({}));
    let items = items["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(
        items[0].to_string().contains(text(&record["id"])),
        "{items:?}"
    );

    // The column is still one the row has, so one the source does not list
    // is refused as before, and the row is not stored.
    let unlisted = json!({ "source": "shop", "entity_type": "thing",
        "rows": [{ "id": "4", "code": "saw", "shape": "" }] });
    let error = refused(&server, "load_rows", unlisted, "invalid");
    assert!(
        text(&error["message"]).contains("a column `shape`"),
        "{error}"
    );
    // A value that is only spaces is a value.
    let spaces = load(json!([{ "id": "4", "code": "saw", "colour": " " }]));
    assert_eq!(spaces["run"]["load"]["inserted"], 1, "{spaces}");
    let found = tool(
        &server,
        "search_records",
        json!({ "source": "shop", "key": "4" }),
    );
    assert_eq!(found["records"][0]["payload"]["colour"], " ", "{found}");
}
