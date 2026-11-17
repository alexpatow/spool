//! The HTTP API, called over a socket against the real binary.

mod support;

use std::path::{Path, PathBuf};

use arrow::array::{Array, AsArray};
use serde_json::{Value, json};
use spool::core::Seq;
use spool::{Engine, IdentityFilter, Options, ReviewFilter, ReviewStatus, Rows, Scenario};
use support::*;

fn companies() -> PathBuf {
    benchmarks().join("identity/companies-small")
}

/// The files of the company scenario, in the order it ingests them, in two
/// steps: `(file, source, key column)`.
const STEPS: [[(&str, &str, &str); 2]; 2] = [
    [
        ("batch-1-crm.csv", "crm", "id"),
        ("batch-1-registry.csv", "registry", "number"),
    ],
    [
        ("batch-2-crm.csv", "crm", "id"),
        ("batch-2-registry.csv", "registry", "number"),
    ],
];

/// The company scenario run in this process, with the library.
fn library(seed: u64) -> Engine {
    let engine = Engine::open_in_memory(Options::seeded(seed)).unwrap();
    Scenario::load(companies())
        .unwrap()
        .run(&engine, |_| Ok(()))
        .unwrap();
    engine
}

#[derive(Clone, Copy, Debug)]
enum Encoding {
    Arrow,
    Json,
}

/// A server on the company project and an empty store.
fn company_server(dir: &Path, seed: u64) -> Server {
    let project = dir.join("project");
    copy_project(&companies(), &project);
    Server::serve(&project, dir, seed)
}

/// The company scenario run through the server: the same calls the library
/// makes, in the same order.
fn run_companies(server: &Server, encoding: Encoding) {
    for step in STEPS {
        for (file, source, key) in step {
            let rows = Rows::from_csv(companies().join(file)).unwrap();
            let response = match encoding {
                Encoding::Arrow => server.post_arrow(
                    &format!("/v1/ingest?source={source}&entity_type=company&key={key}"),
                    rows,
                ),
                Encoding::Json => server.post(
                    "/v1/ingest",
                    json!({
                        "source": source,
                        "entity_type": "company",
                        "key": key,
                        "rows": json_rows(rows),
                    }),
                ),
            };
            assert_eq!(response.ok()["run"]["status"], "succeeded");
        }
        let run = server.call("/v1/runs", json!({ "entity_type": "company" }));
        assert_eq!(run["run"]["status"], "succeeded", "{run}");
    }
}

fn hashes(log: &Value) -> Vec<String> {
    log["changesets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["content_hash"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_same_seed_gives_the_same_changesets_through_the_library_and_the_server() {
    let library = library(11);
    let expected: Vec<String> = library
        .log_after(Seq(0), 1_000)
        .unwrap()
        .iter()
        .map(|entry| entry.content_hash.to_string())
        .collect();
    assert_eq!(expected.len(), 2, "one changeset per step");

    for encoding in [Encoding::Arrow, Encoding::Json] {
        let dir = tempfile::tempdir().unwrap();
        let server = company_server(dir.path(), 11);
        run_companies(&server, encoding);

        let log = server.read("/v1/changesets");
        assert_eq!(hashes(&log), expected, "{encoding:?}");
        assert_eq!(log["head"], 2);
        assert_eq!(
            log["changesets"],
            serde_json::to_value(library.log_after(Seq(0), 1_000).unwrap()).unwrap(),
            "{encoding:?}"
        );

        // The state the log leaves is the same too.
        let identities = server.read("/v1/identities?limit=1000");
        assert_eq!(
            identities["identities"],
            serde_json::to_value(library.identities(&IdentityFilter::default()).unwrap()).unwrap(),
            "{encoding:?}"
        );
        assert_eq!(
            server.read("/v1/info")["store"],
            serde_json::to_value(library.info().unwrap()).unwrap(),
            "{encoding:?}"
        );
    }

    // Another seed gives other IDs, and so other hashes.
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 12);
    run_companies(&server, Encoding::Arrow);
    assert_ne!(hashes(&server.read("/v1/changesets")), expected);
}

#[test]
fn every_call_under_v1_needs_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    // Whether the server is up is not a secret.
    let health = server
        .request("GET", "/healthz")
        .without("Authorization")
        .send();
    assert_eq!(health.ok(), json!({ "status": "ok" }));

    for path in ["/v1/info", "/v1/identities", "/v1/no-such-path"] {
        for authorization in [
            None,
            Some("Bearer wrong"),
            Some("Bearer test-token-and-more"),
            Some("Bearer test-toke"),
            Some("Basic test-token"),
            Some("test-token"),
            Some("Bearer "),
        ] {
            let request = server.request("GET", path).without("Authorization");
            let response = match authorization {
                Some(value) => request.header("Authorization", value),
                None => request,
            }
            .send();
            let message = response.error(401, "unauthorized");
            assert!(message.contains("bearer token"), "{message}");
            assert_eq!(response.header("www-authenticate"), Some("Bearer"));
        }
    }
    let refused = server
        .request("POST", "/v1/ingest")
        .without("Authorization")
        .json(&json!({ "source": "s", "entity_type": "thing", "rows": [{ "code": "a" }] }))
        .send();
    refused.error(401, "unauthorized");
    assert_eq!(
        server.read("/v1/info")["store"]["entity_types"][0]["observations"],
        0
    );

    // The scheme's name is not case-sensitive.
    let response = server
        .request("GET", "/v1/info")
        .header("Authorization", "bearer test-token")
        .send();
    assert_eq!(response.ok()["api"], "v1");
}

#[test]
fn failures_are_described_the_same_way_whatever_went_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    server.get("/v1/no-such-path").error(404, "not_found");
    server.get("/elsewhere").error(404, "not_found");
    server.get("/v1/runs/999").error(404, "not_found");
    server
        .get("/v1/identities/00000000-0000-7000-8000-000000000000")
        .error(404, "not_found");
    server
        .get("/v1/observations/00000000-0000-7000-8000-000000000000")
        .error(404, "not_found");
    server
        .get("/v1/drafts/00000000-0000-7000-8000-000000000000")
        .error(404, "not_found");

    let message = server.get("/v1/runs/first").error(400, "invalid");
    assert!(message.contains("`first` is not a run ID"), "{message}");
    server.get("/v1/identities/nonsense").error(400, "invalid");
    server.get("/v1/runs?status=asleep").error(400, "invalid");
    server.get("/v1/runs?stauts=open").error(400, "invalid");
    server
        .request("DELETE", "/v1/runs")
        .send()
        .error(405, "invalid");

    let message = server
        .request("POST", "/v1/runs")
        .body(b"{ not json")
        .send()
        .error(400, "invalid");
    assert!(message.contains("JSON"), "{message}");
    // A misspelt field is refused, not ignored: `comit` would otherwise commit.
    let message = server
        .post(
            "/v1/runs",
            json!({ "entity_type": "thing", "comit": false }),
        )
        .error(400, "invalid");
    assert!(message.contains("comit"), "{message}");
    let message = server
        .post("/v1/runs", json!({ "entity_type": "gadget" }))
        .error(400, "invalid");
    assert!(message.contains("`gadget` is not declared"), "{message}");
    let message = server
        .post(
            "/v1/ingest",
            json!({ "source": "s", "entity_type": "thing", "rows": [{ "code": { "nested": 1 } }] }),
        )
        .error(400, "invalid");
    assert!(message.contains("code"), "{message}");
    // An integer that would be stored as another number is refused.
    let message = server
        .post(
            "/v1/ingest",
            json!({
                "source": "s",
                "entity_type": "thing",
                "rows": [{ "weight": 0.5 }, { "weight": 9007199254740993u64 }],
            }),
        )
        .error(400, "invalid");
    assert!(message.contains("row 2"), "{message}");
    // A row that says nothing is still a row, and is counted.
    let report = server.call(
        "/v1/ingest",
        json!({ "source": "s", "entity_type": "thing", "rows": [{}, { "name": null }] }),
    );
    assert_eq!(
        (
            &report["run"]["ingest"]["received"],
            &report["run"]["ingest"]["inserted"]
        ),
        (&json!(2), &json!(1))
    );
    server
        .request("POST", "/v1/ingest?source=s&entity_type=thing")
        .header("Content-Type", ARROW)
        .body(b"not an arrow stream")
        .send()
        .error(400, "invalid");
    server
        .post("/v1/ids", json!({ "identities": 10_001 }))
        .error(400, "invalid");

    // A changeset that breaks a rule comes back with the operations at fault.
    let minted = server.call("/v1/ids", json!({ "changesets": 1 }));
    let response = server.post(
        "/v1/changesets",
        json!({
            "id": minted["changesets"][0],
            "base": 0,
            "actor": "user:test",
            "reason": null,
            "run": null,
            "ops": [
                { "op": "create_identity", "identity": "00000000-0000-7000-8000-000000000001", "entity_type": "thing" },
                { "op": "assign", "observation": "00000000-0000-7000-8000-000000000002", "from": null, "to": "00000000-0000-7000-8000-000000000001" },
            ],
        }),
    );
    response.error(409, "rejected");
    let rejections = &response.json()["error"]["rejections"];
    assert_eq!(rejections.as_array().unwrap().len(), 1, "{rejections}");
    assert_eq!(rejections[0]["op_index"], 1, "{rejections}");
    assert_eq!(server.read("/v1/changesets")["head"], 0);
}

#[test]
fn a_body_over_the_limit_is_refused() {
    use spool_cli::server::{ServerOptions, serve};

    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let options = ServerOptions {
        max_body_bytes: 1_000,
        ..ServerOptions::new(TOKEN)
    };
    runtime.spawn(async move {
        serve(
            listener,
            std::sync::Arc::new(engine),
            &options,
            std::future::pending(),
        )
        .await
    });

    let send = |body: &Value| {
        Request::to(&address, "POST", "/v1/ids")
            .header("Authorization", &format!("Bearer {TOKEN}"))
            .json(body)
            .send()
    };
    assert_eq!(
        send(&json!({ "identities": 1 })).ok()["identities"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let padded = json!({ "identities": 1, "padding": "x".repeat(2_000) });
    send(&padded).error(413, "too_large");
}

/// Many rows of the `thing` type, each with a code of its own.
fn many_things(count: usize) -> Rows {
    let ids: Vec<String> = (0..count).map(|index| index.to_string()).collect();
    let rows: Vec<Vec<Option<&str>>> = ids
        .iter()
        .map(|id| vec![Some(id.as_str()), Some(id.as_str())])
        .collect();
    Rows::from_text(&["id", "code"], &rows).unwrap()
}

#[test]
fn a_call_that_outlasts_its_wait_answers_202_and_the_run_is_read_until_it_ends() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    let response = server.post_arrow(
        "/v1/ingest?source=bulk&entity_type=thing&key=id&wait_ms=0",
        many_things(30_000),
    );
    let accepted = response.expect(202);
    let run = accepted["run"]["run"].as_u64().unwrap();
    assert_eq!(accepted["run"]["kind"], "ingest");
    assert_eq!(accepted["run"]["status"], "running", "{accepted}");
    assert_eq!(accepted["run"]["finished_at"], Value::Null);

    // Reading does not wait for the writer: the run in progress is visible,
    // and so is committed state.
    assert_eq!(
        server.read("/v1/runs?status=running")["runs"][0]["run"],
        run
    );
    assert_eq!(server.read("/v1/info")["store"]["head"], 0);

    // A read with `wait_ms` answers when the run has ended.
    let finished = server.read(&format!("/v1/runs/{run}?wait_ms=120000"));
    assert_eq!(finished["run"]["status"], "succeeded", "{finished}");
    assert_eq!(finished["run"]["ingest"]["inserted"], 30_000);
    assert_eq!(server.read("/v1/runs?status=running")["runs"], json!([]));

    // The same for a resolve: with no wait it answers at once with the match
    // run, which goes on to commit.
    let response = server.post("/v1/runs", json!({ "entity_type": "thing", "wait_ms": 0 }));
    let accepted = response.expect(202);
    let run = accepted["run"]["run"].as_u64().unwrap();
    assert_eq!(accepted["run"]["kind"], "match");
    assert_eq!(accepted["run"]["status"], "running", "{accepted}");
    let finished = server.read(&format!("/v1/runs/{run}?wait_ms=120000"));
    assert_eq!(finished["run"]["status"], "succeeded", "{finished}");
    assert_eq!(finished["run"]["resolution"]["identities_created"], 30_000);
    assert_eq!(finished["run"]["resolution"]["committed"]["seq"], 1);
    assert_eq!(server.read("/v1/info")["store"]["head"], 1);

    // A call that is given the time answers 200 with the finished run.
    let response = server.post_arrow(
        "/v1/ingest?source=more&entity_type=thing&key=id",
        many_things(10),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
}

fn ingest_things(server: &Server, source: &str, table: &[&[&str]]) {
    let response = server.post(
        "/v1/ingest",
        json!({
            "source": source,
            "entity_type": "thing",
            "key": "id",
            "rows": json_rows(rows(table)),
        }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
}

fn two_anvils_and_a_hammer(server: &Server) {
    ingest_things(
        server,
        "shop",
        &[
            &["id", "code"],
            &["1", "anvil"],
            &["2", "anvil"],
            &["3", "hammer"],
        ],
    );
}

fn unassigned(server: &Server) -> usize {
    server.read("/v1/observations?unassigned=true")["observations"]
        .as_array()
        .unwrap()
        .len()
}

fn things_server(dir: &Path) -> Server {
    let project = dir.join("project");
    write_things(&project);
    Server::serve(&project, dir, 1)
}

#[test]
fn a_draft_left_for_review_is_stored_listed_and_committed_when_approved() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);

    let run = server.call(
        "/v1/runs",
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
    assert_eq!(summary["observations_assigned"], 3);

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
fn a_draft_nobody_commits_does_not_strand_its_observations() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);
    let leave = json!({ "entity_type": "thing", "commit": false });

    // Rejected: the next run examines the same observations again.
    let first = server.call("/v1/runs", leave.clone());
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
    let second = server.call("/v1/runs", leave.clone());
    let second_draft = second["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(second["run"]["resolution"]["new_observations"], 3);
    ingest_things(&server, "shop", &[&["id", "code"], &["4", "hammer"]]);
    let third = server.call("/v1/runs", leave);
    let third_draft = third["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let third_reconcile = third["run"]["resolution"]["reconcile_run"].clone();
    assert_eq!(third["run"]["resolution"]["new_observations"], 4);
    assert_eq!(third["run"]["resolution"]["observations_assigned"], 4);

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
    let committed = server.call("/v1/runs", json!({ "entity_type": "thing" }));
    assert_eq!(committed["run"]["status"], "succeeded");
    assert_eq!(committed["run"]["resolution"]["observations_assigned"], 4);
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
        "/v1/runs",
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
    theirs["actor"] = json!("user:someone");
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
    ingest_things(
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
            "/v1/runs",
            json!({ "entity_type": "thing", "strategy": strategy, "commit": false }),
        )
        .error(400, "invalid");
    let run = server.call(
        "/v1/runs",
        json!({ "entity_type": "thing", "strategy": strategy }),
    );
    assert_eq!(run["run"]["status"], "open", "{run}");
    assert_eq!(run["run"]["strategy"], strategy);
    let run = run["run"]["run"].as_u64().unwrap();

    // The candidates come as Arrow, and as JSON when that is asked for.
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
    let (anvils, hammers) = (pair("anvil"), pair("hammer"));

    // Judgements go back as JSON or as Arrow.
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
        "/v1/observations/identities",
        json!({ "observations": [anvils.0, anvils.1, hammers.0, hammers.1] }),
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

    // Cancelled: the run fails and its observations wait for another.
    let run = server.call("/v1/runs", start.clone())["run"]["run"]
        .as_u64()
        .unwrap();
    let cancelled = server.call(&format!("/v1/runs/{run}/cancel"), json!({}));
    assert_eq!(cancelled["run"]["status"], "failed");
    assert_eq!(cancelled["run"]["error"], "cancelled");
    server
        .post(&format!("/v1/runs/{run}/reconcile"), json!({}))
        .error(409, "conflict");
    assert_eq!(unassigned(&server), 3);

    // Reconciled with no body and no judgements: every observation gets an
    // identity of its own, committed.
    let run = server.call("/v1/runs", start)["run"]["run"]
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

#[test]
fn identities_observations_and_review_items_are_looked_up() {
    let library = library(5);
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 5);
    run_companies(&server, Encoding::Arrow);

    // An observation is found by its source and key, and names its identity.
    let found = server.read("/v1/observations?source=crm&key=c-101");
    let [observation] = &found["observations"].as_array().unwrap()[..] else {
        panic!("one observation: {found}");
    };
    assert_eq!(observation["source"], "crm");
    assert_eq!(observation["key"], "c-101");
    assert_eq!(observation["entity_type"], "company");
    assert_eq!(observation["payload"]["id"], "c-101");
    let id = observation["id"].as_str().unwrap();
    let identity = observation["identity"].as_str().unwrap();
    assert_eq!(server.read(&format!("/v1/observations/{id}")), *observation);

    // The identity lists its members, and its history names the changesets.
    let view = server.read(&format!("/v1/identities/{identity}"));
    assert_eq!(
        view,
        serde_json::to_value(library.identity(identity.parse().unwrap()).unwrap()).unwrap()
    );
    assert_eq!(view["current"], identity);
    let members = view["members"].as_array().unwrap();
    assert!(members.len() > 1, "{view}");
    assert!(members.iter().any(|member| member["observation"] == id));
    let history = server.read(&format!("/v1/identities/{identity}/history"));
    assert_eq!(
        history["history"],
        serde_json::to_value(library.history(identity.parse().unwrap()).unwrap()).unwrap()
    );
    assert_eq!(history["history"][0]["seq"], 1);
    assert_eq!(history["history"][0]["ops"][0]["op"], "create_identity");

    // Many observations are looked up in one call; an unknown one has none.
    let unknown = "00000000-0000-7000-8000-000000000000";
    let identities = server.call(
        "/v1/observations/identities",
        json!({ "observations": [id, unknown] }),
    );
    assert_eq!(identities, json!({ "identities": [identity, null] }));

    // Lists come in pages.
    let total = server.read("/v1/info")["store"]["entity_types"][0]["identities"]
        .as_u64()
        .unwrap();
    let mut listed = Vec::new();
    let mut after = String::new();
    loop {
        let page = server.read(&format!(
            "/v1/identities?entity_type=company&limit=4{after}"
        ));
        let identities = page["identities"].as_array().unwrap();
        assert!(identities.len() <= 4);
        listed.extend(identities.iter().map(|identity| identity["id"].clone()));
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => break,
        }
    }
    assert_eq!(listed.len() as u64, total);
    assert!(listed.contains(&json!(identity)));
    let first = server.read("/v1/observations?source=registry&limit=3");
    assert_eq!(first["observations"].as_array().unwrap().len(), 3);
    let next = first["next"].as_str().unwrap();
    let second = server.read(&format!(
        "/v1/observations?source=registry&limit=3&after={next}"
    ));
    assert_ne!(
        second["observations"][0]["id"],
        first["observations"][2]["id"]
    );

    // The record that fits two identities is unassigned, and its review item
    // comes back typed, the same as the library reads it.
    let open = server.read("/v1/review?status=open");
    assert_eq!(
        open["items"],
        serde_json::to_value(
            library
                .review_items(&ReviewFilter {
                    status: Some(ReviewStatus::Open),
                    ..ReviewFilter::default()
                })
                .unwrap()
        )
        .unwrap()
    );
    let [item] = &open["items"].as_array().unwrap()[..] else {
        panic!("one open review item: {open}");
    };
    assert_eq!(item["entity_type"], "company");
    assert_eq!(item["status"], "open");
    assert_eq!(item["identities"].as_array().unwrap().len(), 2);
    let stuck = server.read("/v1/observations?source=crm&key=c-117");
    assert_eq!(stuck["observations"][0]["identity"], Value::Null);
    assert_eq!(
        item["observations"],
        json!([stuck["observations"][0]["id"]])
    );
    let waiting = server.read("/v1/observations?unassigned=true");
    assert_eq!(waiting["observations"], stuck["observations"]);
    assert_eq!(
        server.read("/v1/info")["store"]["entity_types"][0]["open_review_items"],
        1
    );
    assert_eq!(
        server.read("/v1/review?entity_type=company")["items"],
        open["items"]
    );
}

#[test]
fn what_is_read_says_what_things_are_called_where_they_came_from_and_what_waits() {
    let library = library(5);
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 5);
    run_companies(&server, Encoding::Json);

    // A record is called what its payload names it, whatever the field.
    let crm = server.read("/v1/observations?source=crm&key=c-102");
    assert_eq!(crm["observations"][0]["label"], "Acorn Tools");
    let registry = server.read("/v1/observations?source=registry&key=X0000002");
    assert_eq!(
        registry["observations"][0]["label"],
        "ACORN TOOL COMPANY LTD"
    );
    let identity = crm["observations"][0]["identity"].as_str().unwrap();
    assert_eq!(registry["observations"][0]["identity"], identity);

    // An identity is called what most of its records are, and says how many
    // each source gave.
    let view = server.read(&format!("/v1/identities/{identity}"));
    assert_eq!(view["label"], "Acorn Tools");
    let page = server.read("/v1/identities?limit=1000");
    assert_eq!(
        page["identities"],
        serde_json::to_value(
            library
                .identities(&IdentityFilter {
                    limit: 1000,
                    ..IdentityFilter::default()
                })
                .unwrap()
        )
        .unwrap()
    );
    let listed = page["identities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["id"] == identity)
        .unwrap();
    assert_eq!(listed["label"], "Acorn Tools");
    assert_eq!(
        listed["sources"],
        json!([
            { "source": "crm", "observations": 2 },
            { "source": "registry", "observations": 1 },
        ])
    );

    // Its members are read as observations, with what each of them says.
    let members = server.read(&format!("/v1/observations?identity={identity}"));
    let mut labels: Vec<&str> = members["observations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["label"].as_str().unwrap())
        .collect();
    labels.sort();
    assert_eq!(
        labels,
        ["ACORN TOOL COMPANY LTD", "Acorn Tool Co.", "Acorn Tools"]
    );
    server
        .get("/v1/observations?identity=acorn")
        .error(400, "invalid");

    // Records and identities are found by the words they hold.
    let found = server.read("/v1/identities?q=acorn%20sheffield");
    assert_eq!(found["identities"].as_array().unwrap().len(), 1);
    assert_eq!(found["identities"][0]["id"], identity);
    let found = server.read("/v1/observations?q=ACORN&source=registry");
    assert_eq!(found["observations"], registry["observations"]);
    assert_eq!(
        server.read("/v1/observations?q=no-such-company")["observations"],
        json!([])
    );

    // The store says what each source gave.
    let info = server.read("/v1/info");
    let sources = info["store"]["entity_types"][0]["sources"]
        .as_array()
        .unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| (
                source["source"].as_str().unwrap(),
                source["observations"].as_u64().unwrap(),
                source["unassigned"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("crm", 23, 1), ("registry", 16, 0)]
    );
    assert_eq!(
        info["store"],
        serde_json::to_value(library.info().unwrap()).unwrap()
    );
    assert_eq!(
        sources[0]["observations"].as_u64().unwrap() + sources[1]["observations"].as_u64().unwrap(),
        info["store"]["entity_types"][0]["observations"]
    );

    // A review item is read with the records it is about: the one that
    // waits, the identities it matched with their members, and the
    // judgements that say so.
    let case = server.read("/v1/review/1/case");
    assert_eq!(
        case,
        serde_json::to_value(library.review_case(1).unwrap()).unwrap()
    );
    assert_eq!(case["item"], server.read("/v1/review/1"));
    assert_eq!(case["observations"][0]["key"], "c-117");
    assert!(case["observations"][0]["label"].is_string());
    let candidates = case["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    for candidate in candidates {
        assert!(candidate["identity"]["label"].is_string(), "{candidate}");
        assert_eq!(
            candidate["members"].as_array().unwrap().len() as u64,
            candidate["identity"]["members"].as_u64().unwrap()
        );
    }
    let judgements = case["judgements"].as_array().unwrap();
    assert!(judgements.len() >= 2, "{case}");
    assert!(
        judgements
            .iter()
            .all(|judgement| judgement["observation"] == case["observations"][0]["id"])
    );
    assert!(
        judgements
            .iter()
            .any(|judgement| judgement["verdict"] == "match")
    );
    server.get("/v1/review/99/case").error(404, "not_found");
    server.get("/v1/review/x/case").error(400, "invalid");
}

#[test]
fn a_pair_of_observations_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    ingest_things(
        &server,
        "shop",
        &[
            &["id", "code", "colour"],
            &["1", "anvil", "black"],
            &["2", "anvil", "black"],
            &["3", "anvil", "red"],
        ],
    );
    server.call("/v1/runs", json!({ "entity_type": "thing" }));
    let id = |key: &str| {
        server.read(&format!("/v1/observations?source=shop&key={key}"))["observations"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (first, second, red) = (id("1"), id("2"), id("3"));

    let explanation = server.read(&format!("/v1/explain?a={first}&b={second}"));
    assert_eq!(explanation["outcome"], "same_identity");
    assert_eq!(
        explanation["left"]["features"],
        json!({ "code": "anvil", "colour": "black" })
    );
    assert_eq!(explanation["comparisons"][0]["strategy"]["name"], "rules");
    assert_eq!(
        explanation["comparisons"][0]["judgements"][0]["verdict"],
        "match"
    );
    assert_eq!(explanation["rules"]["verdict"], "match");
    assert_eq!(explanation["path"][0]["kind"], "match");
    assert!(
        explanation["summary"][0]
            .as_str()
            .unwrap()
            .contains("belong to the same identity"),
        "{explanation}"
    );
    // The two can be named in either order.
    assert_eq!(
        server.read(&format!("/v1/explain?a={second}&b={first}")),
        explanation
    );

    let apart = server.read(&format!("/v1/explain?a={first}&b={red}"));
    assert_eq!(apart["outcome"], "different_identities");
    assert_eq!(apart["rules"]["verdict"], "no_match");
    assert_eq!(
        apart["rules"]["no_match"],
        json!([{ "name": "other_colour", "predicate": "l.colour <> r.colour", "holds": true }])
    );
    assert_eq!(apart["path"], json!([]));

    let unknown = "00000000-0000-7000-8000-000000000000";
    let message = server
        .get(&format!("/v1/explain?a={first}&b={unknown}"))
        .error(404, "not_found");
    assert_eq!(message, format!("there is no observation {unknown}"));
    let message = server
        .get(&format!("/v1/explain?a={first}&b={first}"))
        .error(400, "invalid");
    assert!(message.contains("named twice"), "{message}");
    server
        .get(&format!("/v1/explain?a={first}"))
        .error(400, "invalid");
    server
        .get(&format!("/v1/explain?a={first}&b=nothing"))
        .error(400, "invalid");

    // Review items are listed by an observation they name.
    assert_eq!(
        server.read(&format!("/v1/review?observation={first}"))["items"],
        json!([])
    );
    server
        .get("/v1/review?observation=nothing")
        .error(400, "invalid");
}

#[test]
fn calls_made_at_once_are_all_served_and_writes_take_turns() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());

    std::thread::scope(|scope| {
        let server = &server;
        let writers: Vec<_> = (0..8)
            .map(|writer| {
                scope.spawn(move || {
                    let rows: Vec<Value> = (0..50)
                        .map(|row| json!({ "id": format!("{writer}-{row}"), "code": format!("{row}") }))
                        .collect();
                    let response = server.post(
                        "/v1/ingest",
                        json!({
                            "source": format!("source-{writer}"),
                            "entity_type": "thing",
                            "key": "id",
                            "rows": rows,
                        }),
                    );
                    assert_eq!(response.ok()["run"]["ingest"]["inserted"], 50);
                    let run = server.call("/v1/runs", json!({ "entity_type": "thing" }));
                    assert_ne!(run["run"]["status"], "failed", "{run}");
                })
            })
            .collect();
        let readers: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(move || {
                    for _ in 0..20 {
                        assert_eq!(server.read("/v1/info")["api"], "v1");
                        server.read("/v1/identities?limit=5");
                        server.read("/v1/runs?limit=5");
                    }
                })
            })
            .collect();
        for thread in writers.into_iter().chain(readers) {
            thread.join().unwrap();
        }
    });

    // Whatever order the writers came in, every observation ends up in one
    // of fifty identities: one per code.
    server.call("/v1/runs", json!({ "entity_type": "thing" }));
    let info = server.read("/v1/info");
    let thing = &info["store"]["entity_types"][0];
    assert_eq!(thing["observations"], 400);
    assert_eq!(thing["unassigned"], 0, "{info}");
    assert_eq!(thing["identities"], 50, "{info}");
    assert_eq!(thing["open_review_items"], 0, "{info}");
    let log = server.read("/v1/changesets?limit=1000");
    let seqs: Vec<u64> = log["changesets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    assert_eq!(log["head"], seqs.len() as u64);
}
