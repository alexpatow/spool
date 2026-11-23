//! Drafts left for review, and runs a caller judges step by step.

use super::*;

#[test]
fn a_draft_left_for_review_is_stored_listed_and_committed_when_approved() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);

    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "thing", "commit": false }),
    );
    let run = &run["run"];
    assert_eq!(run["status"], "drafted", "{run}");
    assert_eq!(run["finished_at"], Value::Null);
    assert_eq!(run["resolution"]["committed"], Value::Null);
    let draft = run["resolution"]["draft"].as_str().unwrap().to_owned();

    // Nothing is committed yet.
    assert_eq!(server.read("/v1/changesets")["head"], 0);
    assert_eq!(unassigned(&server), 3);
    let info = server.read("/v1/info");
    assert_eq!(
        info["store"]["entity_types"][0]["open_draft"],
        draft.as_str()
    );

    let open = server.read("/v1/drafts?status=open&entity_type=thing");
    let [summary] = &open["drafts"].as_array().unwrap()[..] else {
        panic!("one open draft: {open}");
    };
    assert_eq!(summary["id"], draft.as_str());
    assert_eq!(summary["match_run"], run["run"]);
    assert_eq!(summary["identities_created"], 2);
    assert_eq!(summary["records_assigned"], 3);

    // The draft can be read whole before it is decided.
    let record = server.read(&format!("/v1/drafts/{draft}"));
    assert_eq!(record["draft"], *summary);
    assert_eq!(record["changeset"]["id"], draft.as_str());
    assert_eq!(record["changeset"]["ops"].as_array().unwrap().len(), 5);

    let committed = server.call(&format!("/v1/drafts/{draft}/approve"), json!({}));
    assert_eq!(committed["seq"], 1);
    assert_eq!(committed["changeset"], draft.as_str());
    assert_eq!(unassigned(&server), 0);
    assert_eq!(server.read("/v1/drafts?status=open")["drafts"], json!([]));
    let record = server.read(&format!("/v1/drafts/{draft}"));
    assert_eq!(record["draft"]["status"], "committed");
    assert_eq!(record["draft"]["committed_seq"], 1);

    // The run that drafted it has now succeeded, and reports the commit.
    let run = server.read(&format!("/v1/runs/{}", run["run"]));
    assert_eq!(run["run"]["status"], "succeeded");
    assert_eq!(run["run"]["resolution"]["committed"], committed);

    // It cannot be decided twice.
    server
        .post(&format!("/v1/drafts/{draft}/approve"), json!({}))
        .error(409, "conflict");
    server
        .post(&format!("/v1/drafts/{draft}/reject"), json!({}))
        .error(409, "conflict");
}

#[test]
fn a_draft_nobody_commits_does_not_strand_its_records() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);
    let leave = json!({ "entity_type": "thing", "commit": false });

    // Rejected: the next run examines the same records again.
    let first = server.call("/v1/match", leave.clone());
    let first_draft = first["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let rejected = server.call(
        &format!("/v1/drafts/{first_draft}/reject"),
        json!({ "reason": "not yet" }),
    );
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["rejected_reason"], "not yet");
    assert_eq!(unassigned(&server), 3);
    server
        .post(&format!("/v1/drafts/{first_draft}/approve"), json!({}))
        .error(409, "conflict");

    // Left alone: a later run drafts again over everything still unassigned,
    // and the draft it replaces is marked as superseded.
    let second = server.call("/v1/match", leave.clone());
    let second_draft = second["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(second["run"]["resolution"]["new_records"], 3);
    load_things(&server, "shop", &[&["id", "code"], &["4", "hammer"]]);
    let third = server.call("/v1/match", leave);
    let third_draft = third["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let third_reconcile = third["run"]["resolution"]["reconcile_run"].clone();
    assert_eq!(third["run"]["resolution"]["new_records"], 4);
    assert_eq!(third["run"]["resolution"]["records_assigned"], 4);

    let superseded = server.read(&format!("/v1/drafts/{second_draft}"));
    assert_eq!(superseded["draft"]["status"], "superseded");
    assert_eq!(superseded["draft"]["superseded_by"], third_reconcile);
    let message = server
        .post(&format!("/v1/drafts/{second_draft}/approve"), json!({}))
        .error(409, "conflict");
    assert!(message.contains("superseded"), "{message}");
    let open = server.read("/v1/drafts?status=open");
    assert_eq!(open["drafts"].as_array().unwrap().len(), 1, "{open}");
    assert_eq!(open["drafts"][0]["id"], third_draft.as_str());

    // A run that commits takes everything, and leaves no draft open.
    let committed = server.call("/v1/match", json!({ "entity_type": "thing" }));
    assert_eq!(committed["run"]["status"], "succeeded");
    assert_eq!(committed["run"]["resolution"]["records_assigned"], 4);
    assert_eq!(unassigned(&server), 0);
    assert_eq!(server.read("/v1/drafts?status=open")["drafts"], json!([]));
    assert_eq!(
        server.read("/v1/identities")["identities"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn approving_a_draft_that_went_stale_is_rejected_and_names_the_operation() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);

    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "thing", "commit": false }),
    );
    let draft = run["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let record = server.read(&format!("/v1/drafts/{draft}"));

    // Someone commits the same assignments first, in a changeset of their own.
    let minted = server.call("/v1/ids", json!({ "identities": 2, "changesets": 1 }));
    let mut theirs = record["changeset"].clone();
    theirs["id"] = minted["changesets"][0].clone();
    // The author is whoever sends it.
    theirs.as_object_mut().unwrap().remove("actor");
    theirs["run"] = Value::Null;
    let mut renamed = serde_json::to_string(&theirs).unwrap();
    let created: Vec<&str> = record["changeset"]["ops"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|op| op["op"] == "create_identity")
        .map(|op| op["identity"].as_str().unwrap())
        .collect();
    for (index, identity) in created.iter().enumerate() {
        renamed = renamed.replace(identity, minted["identities"][index].as_str().unwrap());
    }
    let theirs: Value = serde_json::from_str(&renamed).unwrap();
    let committed = server.call("/v1/changesets", theirs);
    assert_eq!(committed["seq"], 1);

    let response = server.post(&format!("/v1/drafts/{draft}/approve"), json!({}));
    let message = response.error(409, "rejected");
    assert!(message.contains("changeset rejected"), "{message}");
    let rejections = response.json()["error"]["rejections"].clone();
    let indices: Vec<u64> = rejections
        .as_array()
        .unwrap()
        .iter()
        .map(|rejection| rejection["op_index"].as_u64().unwrap())
        .collect();
    assert!(!indices.is_empty(), "{rejections}");
    for (index, rejection) in indices.iter().zip(rejections.as_array().unwrap()) {
        assert_eq!(
            rejection["op"], record["changeset"]["ops"][*index as usize],
            "{rejections}"
        );
    }

    // Nothing of the draft was applied, and it is still there to reject.
    assert_eq!(server.read("/v1/changesets")["head"], 1);
    assert_eq!(
        server.read(&format!("/v1/drafts/{draft}"))["draft"]["status"],
        "open"
    );
    let rejected = server.call(&format!("/v1/drafts/{draft}/reject"), json!({}));
    assert_eq!(rejected["status"], "rejected");
}

fn text_column(batches: &[arrow::record_batch::RecordBatch], name: &str) -> Vec<Option<String>> {
    let mut values = Vec::new();
    for batch in batches {
        let column = arrow::compute::cast(
            batch
                .column_by_name(name)
                .unwrap_or_else(|| panic!("a `{name}` column")),
            &arrow::datatypes::DataType::Utf8,
        )
        .unwrap();
        let column = column.as_string::<i32>();
        for row in 0..column.len() {
            values.push((!column.is_null(row)).then(|| column.value(row).to_owned()));
        }
    }
    values
}

#[test]
fn a_caller_judges_the_candidates_of_a_run_step_by_step() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    load_things(
        &server,
        "shop",
        &[
            &["id", "code", "colour"],
            &["1", "anvil", "black"],
            &["2", "anvil", "black"],
            &["3", "hammer", "red"],
            &["4", "hammer", "red"],
            &["5", "tongs", "grey"],
        ],
    );

    let strategy = json!({ "name": "coin-toss", "version": "2" });
    server
        .post(
            "/v1/match",
            json!({ "entity_type": "thing", "strategy": strategy, "commit": false }),
        )
        .error(400, "invalid");
    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "thing", "strategy": strategy }),
    );
    assert_eq!(run["run"]["status"], "open", "{run}");
    assert_eq!(run["run"]["strategy"], strategy);
    let run = run["run"]["run"].as_u64().unwrap();

    let (anvils, hammers) = read_candidates(&server, run);

    send_judgements(&server, run, &anvils, &hammers);

    // Reconciling leaves a draft for review; approving it commits.
    let resolution = server.call(
        &format!("/v1/runs/{run}/reconcile"),
        json!({ "commit": false }),
    );
    assert_eq!(resolution["match_run"], run);
    assert_eq!(
        resolution["judgements"],
        json!({ "match": 1, "no_match": 1, "unsure": 0 })
    );
    assert_eq!(resolution["identities_created"], 4);
    assert_eq!(resolution["committed"], Value::Null);
    let draft = resolution["draft"].as_str().unwrap();
    assert_eq!(
        server.read(&format!("/v1/runs/{run}"))["run"]["status"],
        "drafted"
    );
    server.call(&format!("/v1/drafts/{draft}/approve"), json!({}));
    assert_eq!(
        server.read(&format!("/v1/runs/{run}"))["run"]["status"],
        "succeeded"
    );

    let identities = server.call(
        "/v1/records/identities",
        json!({ "records": [anvils.0, anvils.1, hammers.0, hammers.1] }),
    );
    let identities = identities["identities"].as_array().unwrap();
    assert!(identities.iter().all(|identity| identity.is_string()));
    assert_eq!(
        identities[0], identities[1],
        "the anvils were judged the same"
    );
    assert_ne!(
        identities[2], identities[3],
        "the hammers were judged different"
    );

    // The run is over: it takes no more judgements and cannot be cancelled.
    server
        .post(
            &format!("/v1/runs/{run}/judgements"),
            json!({ "judgements": [{ "left_id": anvils.0, "right_id": anvils.1, "verdict": "match" }] }),
        )
        .error(409, "conflict");
    server
        .post(
            &format!("/v1/runs/{run}/judgements"),
            json!({ "judgements": [] }),
        )
        .error(409, "conflict");
    server
        .post("/v1/runs/999/judgements", json!({ "judgements": [] }))
        .error(404, "not_found");
    server
        .post(&format!("/v1/runs/{run}/cancel"), json!({}))
        .error(409, "conflict");
    server
        .post("/v1/runs/999/reconcile", json!({}))
        .error(404, "not_found");
}

#[test]
fn an_open_run_is_reconciled_at_once_by_default_or_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);
    let strategy = json!({ "name": "nobody", "version": "1" });
    let start = json!({ "entity_type": "thing", "strategy": strategy });

    // Cancelled: the run fails and its records wait for another.
    let run = server.call("/v1/match", start.clone())["run"]["run"]
        .as_u64()
        .unwrap();
    let cancelled = server.call(&format!("/v1/runs/{run}/cancel"), json!({}));
    assert_eq!(cancelled["run"]["status"], "failed");
    assert_eq!(cancelled["run"]["error"], "cancelled");
    server
        .post(&format!("/v1/runs/{run}/reconcile"), json!({}))
        .error(409, "conflict");
    assert_eq!(unassigned(&server), 3);

    // Reconciled with no body and no judgements: every record gets an
    // identity of its own, committed.
    let run = server.call("/v1/match", start)["run"]["run"]
        .as_u64()
        .unwrap();
    let resolution = server
        .request("POST", &format!("/v1/runs/{run}/reconcile"))
        .send()
        .ok();
    assert_eq!(resolution["identities_created"], 3);
    assert_eq!(resolution["committed"]["seq"], 1);
    assert_eq!(resolution["draft"], Value::Null);
    assert_eq!(
        server.read(&format!("/v1/runs/{run}"))["run"]["status"],
        "succeeded"
    );
    assert_eq!(unassigned(&server), 0);

    let runs = server.read("/v1/runs?kind=match");
    let statuses: Vec<&str> = runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["succeeded", "failed"], "newest first");
}

/// The candidates of a run, as Arrow and as JSON: the pair of anvils, and
/// the pair of hammers.
fn read_candidates(server: &Server, run: u64) -> ((String, String), (String, String)) {
    let batches = server.get(&format!("/v1/runs/{run}/candidates")).batches();
    let left = text_column(&batches, "left_id");
    let right = text_column(&batches, "right_id");
    let codes = text_column(&batches, "left_code");
    assert_eq!(codes.len(), 2, "one pair of anvils and one of hammers");
    assert_eq!(text_column(&batches, "right_code"), codes);
    let listed = server
        .request("GET", &format!("/v1/runs/{run}/candidates"))
        .header("Accept", "application/json")
        .send()
        .ok();
    let listed = listed["candidates"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    for (index, candidate) in listed.iter().enumerate() {
        assert_eq!(candidate["left_id"].as_str(), left[index].as_deref());
        assert_eq!(candidate["right_id"].as_str(), right[index].as_deref());
        assert_eq!(candidate["left_code"].as_str(), codes[index].as_deref());
        assert_eq!(candidate["left_colour"], candidate["right_colour"]);
    }
    let pair = |code: &str| {
        let index = codes
            .iter()
            .position(|found| found.as_deref() == Some(code))
            .unwrap();
        (left[index].clone().unwrap(), right[index].clone().unwrap())
    };
    (pair("anvil"), pair("hammer"))
}

/// Judgements sent back as JSON or as Arrow, and those refused.
fn send_judgements(
    server: &Server,
    run: u64,
    anvils: &(String, String),
    hammers: &(String, String),
) {
    let stored = server.call(
        &format!("/v1/runs/{run}/judgements"),
        json!({ "judgements": [{
            "left_id": anvils.0,
            "right_id": anvils.1,
            "verdict": "match",
            "score": 0.75,
            "detail": { "why": "heads" },
        }] }),
    );
    assert_eq!(stored, json!({ "stored": 1 }));
    let none = server.call(
        &format!("/v1/runs/{run}/judgements"),
        json!({ "judgements": [] }),
    );
    assert_eq!(none, json!({ "stored": 0 }));
    let stored = server
        .post_arrow(
            &format!("/v1/runs/{run}/judgements"),
            rows(&[
                &["left_id", "right_id", "verdict"],
                &[&hammers.0, &hammers.1, "no_match"],
            ]),
        )
        .ok();
    assert_eq!(stored, json!({ "stored": 1 }));
    server
        .post(
            &format!("/v1/runs/{run}/judgements"),
            json!({ "judgements": [{ "left_id": anvils.0, "right_id": anvils.1, "verdict": "perhaps" }] }),
        )
        .error(400, "invalid");
    let message = server
        .post(
            &format!("/v1/runs/{run}/judgements"),
            json!({ "judgements": [{
                "left_id": anvils.0,
                "right_id": "00000000-0000-7000-8000-000000000000",
                "verdict": "match",
            }] }),
        )
        .error(400, "invalid");
    assert!(message.contains("not a known thing"), "{message}");
}
