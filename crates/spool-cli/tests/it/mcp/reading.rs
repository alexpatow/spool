//! Tools that look something up, and answer what the API answers.

use super::*;

#[test]
#[expect(clippy::too_many_lines)]
fn a_tool_that_looks_something_up_answers_what_the_api_answers() {
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path());

    assert_eq!(
        tool(&server, "describe_store", json!({})),
        server.read("/v1/info")
    );

    let found = tool(&server, "search_identities", json!({ "q": "acorn" }));
    assert_eq!(found, server.read("/v1/identities?q=acorn&limit=20"));
    assert_eq!(found["identities"][0]["label"], "Acorn Tools");
    assert_eq!(
        found["identities"][0]["match"],
        json!({ "kind": "label", "value": "Acorn Tools" })
    );
    let acorn = text(&found["identities"][0]["id"]);

    let alias = tool(
        &server,
        "search_identities",
        json!({ "ids": [acorn], "q": "ACORN TOOL COMPANY LTD" }),
    );
    let alias = &alias["identities"][0]["match"];
    assert_eq!(alias["kind"], "alias");
    assert_eq!(alias["value"], "ACORN TOOL COMPANY LTD");
    assert!(alias["record"].is_string());

    let field = tool(
        &server,
        "search_identities",
        json!({ "ids": [acorn], "q": "Sheffield" }),
    );
    let field = &field["identities"][0]["match"];
    assert_eq!(field["kind"], "record");
    assert_eq!(field["field"], "city");
    assert!(field["record"].is_string());

    // A list is short unless the call asks for more.
    let page = tool(
        &server,
        "search_identities",
        json!({ "entity_type": "company" }),
    );
    assert_eq!(
        page,
        server.read("/v1/identities?entity_type=company&limit=20")
    );
    let page = tool(&server, "search_identities", json!({ "limit": 2 }));
    assert_eq!(page["identities"].as_array().unwrap().len(), 2);
    let second = text(&page["identities"][1]["id"]);
    let ids = format!("{acorn},{second}");
    assert_eq!(
        tool(
            &server,
            "search_identities",
            json!({ "ids": [acorn, second] })
        ),
        server.read(&format!("/v1/identities?ids={ids}&limit=20"))
    );
    assert_eq!(
        tool(
            &server,
            "search_identities",
            json!({ "after": page["next"], "limit": 3 })
        ),
        server.read(&format!(
            "/v1/identities?after={}&limit=3",
            text(&page["next"])
        ))
    );

    let identity = tool(&server, "get_identity", json!({ "identity": acorn }));
    assert_eq!(identity, server.read(&format!("/v1/identities/{acorn}")));
    assert_eq!(identity["members"].as_array().unwrap().len(), 3);
    assert_eq!(
        tool(
            &server,
            "get_identity_history",
            json!({ "identity": acorn })
        ),
        server.read(&format!("/v1/identities/{acorn}/history"))
    );

    let records = tool(&server, "search_records", json!({ "identity": acorn }));
    assert_eq!(
        records,
        server.read(&format!("/v1/records?identity={acorn}&limit=20"))
    );
    let a = text(&records["records"][0]["id"]);
    let b = text(&records["records"][1]["id"]);
    assert_eq!(
        tool(
            &server,
            "search_records",
            json!({ "source": "crm", "key": "c-102" })
        ),
        server.read("/v1/records?source=crm&key=c-102&limit=20")
    );
    let unplaced = tool(&server, "search_records", json!({ "unassigned": true }));
    assert_eq!(
        unplaced,
        server.read("/v1/records?unassigned=true&limit=20")
    );
    assert_eq!(unplaced["records"].as_array().unwrap().len(), 1);
    assert_eq!(
        tool(&server, "get_record", json!({ "record": a })),
        server.read(&format!("/v1/records/{a}"))
    );
    assert_eq!(
        tool(&server, "explain_pair", json!({ "a": a, "b": b })),
        server.read(&format!("/v1/explain?a={a}&b={b}"))
    );

    let open = tool(&server, "list_review_items", json!({}));
    assert_eq!(open, server.read("/v1/review?status=open"));
    assert_eq!(open["items"].as_array().unwrap().len(), 1);
    let none = tool(
        &server,
        "list_review_items",
        json!({ "status": "dismissed" }),
    );
    assert_eq!(none, json!({ "items": [], "next": null }));
    assert_eq!(
        tool(&server, "get_review_case", json!({ "item": 1 })),
        server.read("/v1/review/1/case")
    );
    assert_eq!(
        tool(&server, "list_constraints", json!({})),
        server.read("/v1/constraints")
    );
    let mut log = server.read("/v1/changesets?after=1&limit=20");
    log["next"] = Value::Null;
    for entry in log["changesets"].as_array_mut().unwrap() {
        let ops = entry["changeset"]
            .as_object_mut()
            .unwrap()
            .remove("ops")
            .unwrap();
        entry["operation_count"] = json!(ops.as_array().unwrap().len());
    }
    assert_eq!(tool(&server, "list_changesets", json!({ "after": 1 })), log);

    // What the API does not find, or cannot read, the tool says as the API
    // does, in an answer the model can read.
    let unused = "01a1ee50-0000-7000-8000-000000000000";
    let missing = refused(
        &server,
        "get_identity",
        json!({ "identity": unused }),
        "not_found",
    );
    assert_eq!(
        missing["message"].as_str(),
        Some(
            server
                .get(&format!("/v1/identities/{unused}"))
                .error(404, "not_found")
                .as_str()
        )
    );
    refused(
        &server,
        "get_review_case",
        json!({ "item": 99 }),
        "not_found",
    );
    let unread = refused(
        &server,
        "get_identity",
        json!({ "identity": "acorn" }),
        "invalid",
    );
    assert!(
        text(&unread["message"]).contains("get_identity"),
        "{unread}"
    );
    refused(&server, "get_identity", json!({}), "invalid");
    refused(
        &server,
        "search_records",
        json!({ "limit": "ten" }),
        "invalid",
    );
    refused(
        &server,
        "describe_store",
        json!({ "verbose": true }),
        "invalid",
    );
    refused(
        &server,
        "list_review_items",
        json!({ "status": "pending" }),
        "invalid",
    );
    // Looking things up changed nothing.
    assert_eq!(head(&server), 2);
}

#[test]
#[expect(clippy::too_many_lines)]
fn identity_snapshots_keep_historical_members_and_merge_redirects() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    server.call(
        "/v1/load",
        json!({
            "source": "shop", "entity_type": "thing", "wait_ms": 5000,
            "rows": [
                { "id": "a", "code": "anvil" }, { "id": "move", "code": "vise" },
                { "id": "b", "code": "bolt" }, { "id": "c", "code": "chisel" },
            ],
        }),
    );
    let record = |key| {
        server.read(&format!("/v1/records?source=shop&key={key}"))["records"][0]["id"].clone()
    };
    let [anvil, vise, bolt, chisel] = ["a", "move", "b", "c"].map(record);
    let minted = server.call("/v1/ids", json!({ "identities": 3 }));
    let [a, b, c] = [0, 1, 2].map(|i| text(&minted["identities"][i]));
    let commit = |ops| {
        tool(
            &server,
            "commit_changeset",
            json!({ "ops": ops, "reason": "historical example" }),
        )["seq"]
            .as_u64()
            .unwrap()
    };
    let at = |id, seq| {
        let http = server.read(&format!("/v1/identities/{id}/at?seq={seq}"));
        assert_eq!(
            http,
            tool(
                &server,
                "get_identity_at",
                json!({ "identity": id, "seq": seq })
            )
        );
        assert_eq!(http["as_of_seq"], seq);
        assert_eq!(http["id"], id);
        http
    };

    assert_eq!(
        commit(
            [a, b, c]
                .map(
                    |id| json!({ "op": "create_identity", "identity": id, "entity_type": "thing" })
                )
                .to_vec()
        ),
        1
    );
    let empty = at(a, 1);
    assert_eq!(empty["created"], 1);
    assert_eq!(empty["members"], json!([]));
    assert!(empty["label"].is_null());
    let before_creation = server
        .get(&format!("/v1/identities/{a}/at?seq=0"))
        .error(404, "not_found");
    assert!(
        before_creation.contains("created at 1"),
        "{before_creation}"
    );
    assert_eq!(
        refused(
            &server,
            "get_identity_at",
            json!({ "identity": a, "seq": 0 }),
            "not_found"
        )["message"],
        before_creation
    );

    let assignments = [(&anvil, a), (&vise, a), (&bolt, b), (&chisel, c)]
        .map(|(record, to)| json!({ "op": "assign", "record": record, "from": null, "to": to }));
    assert_eq!(commit(assignments.to_vec()), 2);
    let assigned = at(a, 2);
    let members = assigned["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert!(members.iter().all(|member| member["since"] == 2));
    assert_eq!(at(b, 2)["label"], "bolt");
    assert_eq!(at(c, 2)["current"], c);

    let split = tool(
        &server,
        "split_identity",
        json!({ "identity": a, "records": [vise], "reason": "separate tools" }),
    );
    let split_id = text(&split["identity"]);
    assert_eq!(split["kept"], a);
    assert_eq!(split["committed"]["seq"], 3);
    assert_eq!(
        at(a, 3)["members"][0],
        json!({ "record": anvil, "source": "shop", "key": "a", "since": 2 })
    );
    assert_eq!(at(split_id, 3)["members"][0]["since"], 3);
    assert_eq!(at(split_id, 3)["label"], "vise");
    let constraints = tool(&server, "list_constraints", json!({ "record": vise }));
    assert_eq!(constraints["constraints"][0]["kind"], "cannot_link");
    assert!(
        server
            .get(&format!("/v1/identities/{split_id}/at?seq=2"))
            .error(404, "not_found")
            .contains("created at 3")
    );

    assert_eq!(
        commit(vec![json!({ "op": "merge", "from": a, "into": b })]),
        4
    );
    let first_merge = at(a, 4);
    assert_eq!(first_merge["created"], 1);
    assert_eq!(first_merge["current"], b);
    assert_eq!(first_merge["members"], at(b, 4)["members"]);
    let moved = first_merge["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|member| member["record"] == anvil)
        .unwrap();
    assert_eq!(moved["since"], 4);
    assert_eq!(
        commit(vec![json!({ "op": "merge", "from": b, "into": c })]),
        5
    );
    let second_merge = at(a, 5);
    assert_eq!(second_merge["current"], c);
    assert_eq!(second_merge["members"].as_array().unwrap().len(), 3);

    let undo = |seq| {
        tool(
            &server,
            "undo_changeset",
            json!({ "seq": seq, "reason": "restore the separate tools" }),
        )["committed"]["seq"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(undo(5), 6);
    assert_eq!(at(a, 6)["current"], b);
    assert_eq!(at(a, 5), second_merge);
    assert_eq!(undo(4), 7);
    assert_eq!(at(a, 7)["current"], a);
    assert_eq!(at(a, 4), first_merge);
    assert_eq!(at(a, 2), assigned);

    let newer = server.call(
        "/v1/records",
        json!({
            "source": "shop", "entity_type": "thing",
            "row": { "id": "a", "code": "new anvil" }, "replace": true,
        }),
    );
    let versioned = at(a, 8);
    assert_ne!(newer["record"], anvil);
    assert_eq!(at(a, 7)["members"][0]["record"], anvil);
    let versions = versioned["members"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    assert!(versions.iter().any(|member| member["record"] == anvil));
    assert!(
        versions
            .iter()
            .any(|member| member["record"] == newer["record"] && member["since"] == 8)
    );
    let retracted = server.call(
        "/v1/retract",
        json!({ "source": "shop", "entity_type": "thing", "keys": ["a"], "wait_ms": 5000 }),
    );
    assert_eq!(retracted["run"]["retraction"]["committed"]["seq"], 9);
    assert_eq!(at(a, 9)["members"], json!([]));
    assert_eq!(at(a, 8), versioned);
    assert_eq!(at(a, 2), assigned);
    assert_eq!(at(a, 1), empty);

    let future = server
        .get(&format!("/v1/identities/{a}/at?seq=10"))
        .error(400, "invalid");
    assert!(future.contains("store head at 9"), "{future}");
    assert_eq!(
        refused(
            &server,
            "get_identity_at",
            json!({ "identity": a, "seq": 10 }),
            "invalid"
        )["message"],
        future
    );
    let unknown = "01a1ee50-0000-7000-8000-000000000000";
    for (id, seq, status, code) in [
        (unknown, 9, 404, "not_found"),
        ("invalid-uuid", 2, 400, "invalid"),
        (a, -1, 400, "invalid"),
    ] {
        server
            .get(&format!("/v1/identities/{id}/at?seq={seq}"))
            .error(status, code);
        refused(
            &server,
            "get_identity_at",
            json!({ "identity": id, "seq": seq }),
            code,
        );
    }
    assert_eq!(head(&server), 9);
}
