//! Answers too large for a model, cut to what it can read.

use super::*;

#[test]
fn a_run_too_large_for_a_model_is_answered_without_its_lists() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    // A thing in black and one in red under each code are two identities.
    let things = 2000;
    let colours: Vec<Value> = (0..things)
        .flat_map(|code| {
            ["black", "red"].map(|colour| {
                json!({ "id": format!("{code}-{colour}"), "code": format!("c-{code}"), "colour": colour })
            })
        })
        .collect();
    server.call(
        "/v1/load",
        json!({ "source": "shop", "entity_type": "thing", "rows": colours }),
    );
    let placed = tool(&server, "run_match", json!({ "entity_type": "thing" }));
    assert_eq!(
        placed["run"]["resolution"]["identities_created"],
        2 * things
    );
    assert!(placed["left_out"].is_null(), "{placed}");

    // One of no colour under each code fits both, so each is left for
    // review, and the run lists them all: more than a tool returns.
    let plain: Vec<Value> = (0..things)
        .map(|code| json!({ "id": format!("{code}"), "code": format!("c-{code}") }))
        .collect();
    server.call(
        "/v1/load",
        json!({ "source": "bulk", "entity_type": "thing", "rows": plain }),
    );
    let matched = tool(&server, "run_match", json!({ "entity_type": "thing" }));
    let run = matched["run"]["run"].as_u64().unwrap();
    let whole = server.get(&format!("/v1/runs/{run}"));
    assert!(whole.body.len() > 256 * 1024, "{}", whole.body.len());
    // The run was made, so it is answered as made, with its counts.
    for answer in [matched, tool(&server, "get_run", json!({ "run": run }))] {
        assert_eq!(
            answer["left_out"],
            json!(["run.resolution.review"]),
            "{answer}"
        );
        assert!(
            text(&answer["note"]).contains("list_review_items"),
            "{answer}"
        );
        let resolution = &answer["run"]["resolution"];
        assert_eq!(answer["run"]["status"], "succeeded", "{answer}");
        assert_eq!(resolution["new_records"], things, "{answer}");
        assert_eq!(resolution["records_unassigned"], things, "{answer}");
        assert!(resolution["review"].is_null(), "{answer}");
        assert!(answer.to_string().len() < 8 * 1024, "{answer}");
    }
    let open = server.read("/v1/review?status=open");
    assert_eq!(open["items"].as_array().unwrap().len(), things as usize);
}

#[test]
fn an_answer_too_large_for_a_model_is_refused_and_says_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    let name = "a name that takes some room ".repeat(20);
    let rows: Vec<Value> = (0..600)
        .map(|row| json!({ "id": format!("k-{row}"), "code": format!("c-{row}"), "name": name }))
        .collect();
    server.call(
        "/v1/load",
        json!({ "source": "bulk", "entity_type": "thing", "rows": rows }),
    );

    let few = tool(&server, "search_records", json!({}));
    assert_eq!(few["records"].as_array().unwrap().len(), 20);
    assert!(few["next"].is_string(), "{few}");
    let all = refused(
        &server,
        "search_records",
        json!({ "limit": 600 }),
        "too_large",
    );
    assert!(text(&all["message"]).contains("`limit`"), "{all}");
}

#[test]
fn large_match_changesets_can_be_audited_one_page_at_a_time() {
    const MAX_RESULT_BYTES: usize = 256 * 1024;
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    assert_eq!(
        tool(&server, "list_changesets", json!({})),
        json!({ "changesets": [], "head": 0, "next": null })
    );
    for batch in 0..2 {
        let rows: Vec<Value> = (0..1500)
            .map(|row| json!({ "id": format!("{batch}-{row}"), "code": format!("{batch}-{row}") }))
            .collect();
        server.call(
            "/v1/load",
            json!({ "source": "bulk", "entity_type": "thing", "rows": rows }),
        );
        let matched = tool(&server, "run_match", json!({ "entity_type": "thing" }));
        assert_eq!(matched["run"]["status"], "succeeded", "{matched}");
    }

    let summaries = tool(&server, "list_changesets", json!({ "limit": 2 }));
    assert_eq!(summaries["head"], 2);
    assert_eq!(summaries["next"], Value::Null);
    assert_eq!(summaries["changesets"].as_array().unwrap().len(), 2);
    assert!(summaries.to_string().len() < MAX_RESULT_BYTES);

    let mut after = Value::Null;
    for seq in 1..=2 {
        let log = tool(
            &server,
            "list_changesets",
            json!({ "after": after, "limit": 1 }),
        );
        let summary = &log["changesets"][0];
        assert_eq!(summary, &summaries["changesets"][seq as usize - 1]);
        let full = entry(&server, seq);
        let expected = full["changeset"]["ops"].as_array().unwrap();
        assert!(full["changeset"]["ops"].to_string().len() > MAX_RESULT_BYTES);
        assert_eq!(summary["seq"], seq);
        assert_eq!(summary["content_hash"], full["content_hash"]);
        assert_eq!(summary["changeset"]["run"], full["changeset"]["run"]);
        assert_eq!(summary["operation_count"], expected.len());
        assert!(summary["changeset"].get("ops").is_none());
        assert_eq!(summary["changeset"]["actor"], "engine");
        assert_eq!(summary["started_by"], "agent:api");

        let mut offset = 0;
        let mut audited = Vec::new();
        loop {
            let page = tool(
                &server,
                "list_changeset_ops",
                json!({ "seq": seq, "offset": offset, "limit": u64::MAX }),
            );
            assert!(page.to_string().len() < MAX_RESULT_BYTES);
            assert_eq!(page["seq"], seq);
            assert_eq!(page["offset"], offset);
            let ops = page["ops"].as_array().unwrap();
            assert!(!ops.is_empty() && ops.len() <= 1000);
            audited.extend(ops.iter().cloned());
            let Some(next) = page["next"].as_u64() else {
                break;
            };
            assert_eq!(next, audited.len() as u64);
            offset = next;
        }
        assert_eq!(&audited, expected, "operations at {seq}");
        after = log["next"].clone();
        assert_eq!(after, if seq == 1 { json!(seq) } else { Value::Null });
    }
    assert_eq!(
        tool(
            &server,
            "list_changeset_ops",
            json!({ "seq": 2, "offset": u64::MAX })
        ),
        json!({ "seq": 2, "offset": u64::MAX, "ops": [], "next": null })
    );
    refused(
        &server,
        "list_changeset_ops",
        json!({ "seq": 3 }),
        "not_found",
    );
}

#[test]
fn a_change_whose_answer_is_too_large_is_still_answered_as_made() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    let rows: Vec<Value> = (0..5000)
        .map(|row| json!({ "id": format!("k-{row}"), "code": format!("c-{row}") }))
        .collect();
    server.call(
        "/v1/load",
        json!({ "source": "bulk", "entity_type": "thing", "rows": rows }),
    );
    let mut records: Vec<Value> = Vec::new();
    let mut after = String::new();
    loop {
        let page = server.read(&format!("/v1/records?limit=1000{after}"));
        records.extend(
            page["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|record| record["id"].clone()),
        );
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => break,
        }
    }
    assert_eq!(records.len(), 5000);

    // One identity holds them all.
    let minted = tool(&server, "mint_identity_ids", json!({}));
    let whole = minted["identities"][0].clone();
    let mut ops =
        vec![json!({ "op": "create_identity", "identity": whole, "entity_type": "thing" })];
    ops.extend(
        records
            .iter()
            .map(|record| json!({ "op": "assign", "record": record, "from": null, "to": whole })),
    );
    let placed = tool(
        &server,
        "commit_changeset",
        json!({ "ops": ops, "reason": "all one thing" }),
    );
    assert_eq!(placed["seq"], 1);

    // Splitting it answers with the changeset that did, which is more than
    // a tool returns. The split was made, so the answer says so and keeps
    // what the caller needs: where it was committed and the new identity.
    let side = &records[..2000];
    let split = tool(
        &server,
        "split_identity",
        json!({ "identity": whole, "records": side, "reason": "two things", "keep_apart": false }),
    );
    assert_eq!(split["committed"]["seq"], 2, "{split}");
    assert_eq!(split["kept"], whole);
    assert_eq!(
        split["left_out"],
        json!(["changeset.ops", "moved"]),
        "{split}"
    );
    assert_eq!(split["changeset"]["reason"], "two things", "{split}");
    assert!(text(&split["note"]).contains("succeeded"), "{split}");
    assert_eq!(head(&server), 2);
    let new = tool(
        &server,
        "get_identity",
        json!({ "identity": split["identity"] }),
    );
    assert_eq!(new["members"].as_array().unwrap().len(), 2000);

    // So does taking it back, whether shown or committed.
    let shown = tool(
        &server,
        "undo_changeset",
        json!({ "seq": 2, "reason": "one thing after all", "commit": false }),
    );
    assert!(shown["committed"].is_null(), "{shown}");
    assert_eq!(shown["left_out"], json!(["changeset.ops"]));
    assert_eq!(head(&server), 2);
    let undone = tool(
        &server,
        "undo_changeset",
        json!({ "seq": 2, "reason": "one thing after all" }),
    );
    assert_eq!(undone["committed"]["seq"], 3, "{undone}");
    assert_eq!(undone["left_out"], json!(["changeset.ops"]));
    assert_eq!(head(&server), 3);
}

#[test]
fn a_refusal_too_large_for_a_model_gives_the_first_of_its_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    let ops: Vec<Value> = (0..5000)
        .map(|op| {
            let identity = format!("01a1ee5d-3154-710c-8a15-{op:012x}");
            json!({ "op": "merge", "from": identity, "into": identity })
        })
        .collect();

    // The API answers with every operation that broke a rule, which is more
    // than a tool returns.
    let whole = server
        .request("POST", "/v1/changesets")
        .json(&json!({ "ops": ops, "reason": "each into itself" }))
        .send();
    assert_eq!(whole.status, 409);
    assert!(whole.body.len() > 256 * 1024, "{}", whole.body.len());

    let error = refused(
        &server,
        "commit_changeset",
        json!({ "ops": ops, "reason": "each into itself" }),
        "rejected",
    );
    // With what the error is wrapped in, it is no more than a tool returns.
    let size = json!({ "error": error }).to_string().len();
    assert!(size <= 256 * 1024, "{size}");
    let given = error["rejections"].as_array().unwrap();
    assert!(!given.is_empty() && given.len() < 5000, "{}", given.len());
    for (index, rejection) in given.iter().enumerate() {
        assert_eq!(rejection["op_index"], index, "{rejection}");
    }
    let said = text(&error["message"]);
    let count = format!("the first {}", given.len());
    assert!(said.contains(&count), "{said}");
    assert!(said.contains("Nothing was changed"), "{said}");
    assert_eq!(head(&server), 0);

    // An error that repeats what the caller sent repeats the beginning of
    // it: an argument the tool does not take, and a tool there is not.
    let long = "x".repeat(300 * 1024);
    let mut named = json!({ "ops": [], "reason": "nothing" });
    named[&long] = json!(true);
    let error = refused(&server, "commit_changeset", named, "invalid");
    let said = text(&error["message"]);
    assert!(said.contains("`commit_changeset`"), "{said}");
    assert!(said.len() <= 4 * 1024, "{}", said.len());
    let unknown = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": long, "arguments": {} },
    });
    let answer = message(&server, &unknown).send().json();
    assert_eq!(
        answer["error"]["code"], -32602,
        "{}",
        answer["error"]["code"]
    );
    let said = text(&answer["error"]["message"]);
    assert!(said.starts_with("there is no tool called `xxx"), "{said}");
    assert!(said.len() <= 5 * 1024, "{}", said.len());
}
