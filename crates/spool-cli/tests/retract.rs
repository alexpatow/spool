//! Rows a source takes back, over the API and with `spool retract`.

mod support;

use std::path::Path;
use std::process::Output;

use serde_json::{Value, json};
use support::*;

fn load(server: &Server, table: &[&[&str]]) -> Value {
    let response = server.post(
        "/v1/load",
        json!({
            "source": "shop",
            "entity_type": "thing",
            "rows": json_rows(rows(table)),
        }),
    );
    let run = response.ok()["run"].clone();
    assert_eq!(run["status"], "succeeded");
    run
}

fn resolve(server: &Server) -> Value {
    let run = server.call("/v1/match", json!({ "entity_type": "thing" }))["run"].clone();
    assert_eq!(run["status"], "succeeded");
    run
}

fn record(server: &Server, key: &str) -> Value {
    server.read(&format!("/v1/records?source=shop&key={key}"))["records"][0].clone()
}

/// Two anvils, which are one thing, and a hammer, all with an identity.
fn workshop(dir: &Path) -> Server {
    let project = dir.join("project");
    write_things(&project);
    let server = Server::serve(&project, dir, 1);
    load(
        &server,
        &[
            &["id", "code"],
            &["1", "anvil"],
            &["2", "anvil"],
            &["3", "hammer"],
        ],
    );
    resolve(&server);
    server
}

/// `spool <arguments>` against `server`, with its token.
fn ask(server: &Server, arguments: &[&str]) -> Output {
    spool()
        .args(arguments)
        .args(["--url", &format!("http://{}", server.address)])
        .env("SPOOL_TOKEN", server.token())
        .output()
        .unwrap()
}

/// What a command that has to succeed printed.
fn said(server: &Server, arguments: &[&str]) -> String {
    let output = ask(server, arguments);
    assert!(
        output.status.success(),
        "spool {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    text(&output.stdout)
}

/// The exit code and the complaint of a command that has to fail.
fn refused(server: &Server, arguments: &[&str]) -> (Option<i32>, String) {
    let output = ask(server, arguments);
    assert!(!output.status.success(), "spool {arguments:?} succeeded");
    (output.status.code(), text(&output.stderr))
}

/// A line of what was printed, whatever the widths of its columns.
fn has(printed: &str, line: &str) {
    assert!(
        printed
            .lines()
            .any(|printed| printed.split_whitespace().eq(line.split_whitespace())),
        "no line `{line}` in:\n{printed}"
    );
}

#[test]
fn a_row_is_taken_back_over_the_api_and_comes_back_when_it_is_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());
    let before = record(&server, "2");
    let identity = before["identity"].as_str().unwrap().to_owned();
    assert_eq!(record(&server, "1")["identity"], identity.as_str());

    let run = server.call(
        "/v1/retract",
        json!({
            "source": "shop",
            "entity_type": "thing",
            "keys": ["2", "9"],
            "reason": "the shop sold it",
        }),
    )["run"]
        .clone();
    assert_eq!(run["kind"], "retract");
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["entity_type"], "thing");
    let report = &run["retraction"];
    assert_eq!(report["run"], run["run"]);
    assert_eq!(report["received"], 2);
    assert_eq!(report["unknown"], 1);
    assert_eq!(report["retracted"], 1);
    assert_eq!(report["detached"], 1);
    assert_eq!(
        server.read(&format!("/v1/runs/{}", run["run"]))["run"],
        run,
        "the run is read as it was answered"
    );
    assert_eq!(
        server.read("/v1/runs?kind=retract")["runs"],
        json!([run.clone()])
    );

    // The record is still there, with what the source said, and has no
    // identity. The identity keeps its other member.
    let after = record(&server, "2");
    assert_eq!(after["id"], before["id"]);
    assert_eq!(after["payload"], before["payload"]);
    assert_eq!(after["identity"], Value::Null);
    assert_eq!(after["retracted"], run["run"]);
    assert_eq!(record(&server, "1")["identity"], identity.as_str());
    assert_eq!(record(&server, "1")["retracted"], Value::Null);

    // It left by a changeset of the caller, and it is not waiting for an
    // identity.
    let seq = report["committed"]["seq"].as_u64().unwrap();
    let log = server.read("/v1/changesets")["changesets"].clone();
    let entry = log
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["seq"] == seq)
        .unwrap();
    assert_eq!(entry["changeset"]["actor"], "agent:api");
    assert_eq!(entry["changeset"]["reason"], "the shop sold it");
    // No reconcile run drafted it: the retract run names it instead.
    assert_eq!(entry["changeset"]["run"], Value::Null);
    assert_eq!(
        entry["changeset"]["ops"],
        json!([{ "op": "assign", "record": before["id"], "from": identity, "to": null }])
    );
    assert_eq!(
        server.read("/v1/records?unassigned=true")["records"],
        json!([])
    );
    let info = server.read("/v1/info");
    let thing = &info["store"]["entity_types"][0];
    assert_eq!(thing["records"], 3);
    assert_eq!(thing["unassigned"], 0);
    assert_eq!(thing["retracted"], 1);

    // It is no work for a run: none is started for it.
    assert_eq!(
        server.call("/v1/match", json!({ "entity_type": "thing" }))["run"],
        Value::Null
    );

    // Sent again, it is back in the identity it left, by a changeset of
    // the engine's that the load run names.
    let again = load(&server, &[&["id", "code"], &["2", "anvil"]]);
    assert_eq!(again["load"]["inserted"], 0);
    assert_eq!(again["load"]["duplicates"], 1);
    assert_eq!(again["load"]["restored"], 1);
    assert_eq!(again["load"]["returned"], 1);
    assert_eq!(record(&server, "2")["retracted"], Value::Null);
    assert_eq!(record(&server, "2")["identity"], identity.as_str());
    let log = server.read("/v1/changesets")["changesets"].clone();
    let entry = log.as_array().unwrap().last().unwrap();
    assert_eq!(entry["seq"], again["load"]["committed"]["seq"]);
    assert_eq!(entry["changeset"]["actor"], "engine");
    assert_eq!(
        entry["changeset"]["reason"],
        "source `shop` sent the rows again"
    );
    assert_eq!(
        entry["changeset"]["ops"],
        json!([{ "op": "assign", "record": before["id"], "from": null, "to": identity }])
    );
    // Reading the run again says the same.
    let read = server.read(&format!("/v1/runs/{}", again["run"]));
    assert_eq!(read["run"]["load"], again["load"]);
    // It has its identity, so no run is started for it.
    assert_eq!(
        server.call("/v1/match", json!({ "entity_type": "thing" }))["run"],
        Value::Null
    );
}

#[test]
fn a_retraction_that_cannot_be_made_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());
    let request = |change: fn(&mut Value)| {
        let mut body = json!({ "source": "shop", "entity_type": "thing", "keys": ["2"] });
        change(&mut body);
        server.post("/v1/retract", body)
    };

    let message = request(|body| body["keys"] = json!([])).error(400, "invalid");
    assert!(message.contains("no keys"), "{message}");
    let message = request(|body| body["source"] = json!("nowhere")).error(400, "invalid");
    assert!(message.contains("nowhere"), "{message}");
    let message = request(|body| body["entity_type"] = json!("gadget")).error(400, "invalid");
    assert!(message.contains("gadget"), "{message}");
    request(|body| body["rows"] = json!([])).error(400, "invalid");
    request(|body| body["keys"] = json!("2")).error(400, "invalid");

    // Nothing was started, and nothing changed.
    assert_eq!(server.read("/v1/runs?kind=retract")["runs"], json!([]));
    assert!(record(&server, "2")["identity"].is_string());

    // A retraction needs a token, as every call does.
    let response = server
        .request("POST", "/v1/retract")
        .without("Authorization")
        .json(&json!({ "source": "shop", "entity_type": "thing", "keys": ["2"] }))
        .send();
    response.error(401, "unauthorized");
    assert!(record(&server, "2")["identity"].is_string());
}

#[test]
fn a_record_that_was_taken_back_is_not_given_an_identity_over_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());
    let taken = record(&server, "2");
    let identity = taken["identity"].clone();
    server.call(
        "/v1/retract",
        json!({ "source": "shop", "entity_type": "thing", "keys": ["2"] }),
    );

    let head = server.read("/v1/info")["store"]["head"].clone();
    let id = server.call("/v1/ids", json!({ "changesets": 1 }))["changesets"][0].clone();
    let response = server.post(
        "/v1/changesets",
        json!({
            "id": id,
            "base": head,
            "ops": [{ "op": "assign", "record": taken["id"], "from": null, "to": identity }],
        }),
    );
    let message = response.error(409, "conflict");
    assert!(message.contains("taken back by its source"), "{message}");
    assert_eq!(record(&server, "2")["identity"], Value::Null);
}

#[test]
fn spool_retract_takes_rows_back_on_a_running_server() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());
    let identity = record(&server, "1")["identity"].clone();

    let printed = said(
        &server,
        &[
            "retract", "--source", "shop", "--type", "thing", "2", "9", "--reason", "sold",
        ],
    );
    let run = server.read("/v1/runs?kind=retract")["runs"][0].clone();
    let seq = &run["retraction"]["committed"]["seq"];
    assert_eq!(
        printed,
        format!(
            "run {}: 2 keys named, 1 record taken back\n\
             1 record left an identity, by the changeset at log position {seq}\n\
             1 key was unknown: `shop` has no `thing` row under it\n",
            run["run"]
        )
    );
    assert_eq!(record(&server, "2")["identity"], Value::Null);
    assert_eq!(record(&server, "2")["retracted"], run["run"]);
    assert_eq!(record(&server, "1")["identity"], identity);
    let log = server.read("/v1/changesets")["changesets"].clone();
    let entry = log.as_array().unwrap().last().unwrap().clone();
    assert_eq!(entry["seq"], *seq);
    assert_eq!(entry["changeset"]["reason"], "sold");
    assert_eq!(entry["changeset"]["actor"], "agent:api");

    // Sent again, the same keys change nothing.
    let printed = said(
        &server,
        &["retract", "--source", "shop", "--type", "thing", "2"],
    );
    assert!(
        printed.ends_with(": 1 key named, 0 records taken back\nnone of them had an identity\n"),
        "{printed}"
    );

    // Keys come from a file as well, one on a line, and the answer is the
    // run as the server gave it.
    let keys = dir.path().join("keys.txt");
    std::fs::write(&keys, "3\r\n\n1\n").unwrap();
    let printed = said(
        &server,
        &[
            "retract",
            "--source",
            "shop",
            "--type",
            "thing",
            "--keys",
            path(&keys),
            "--json",
        ],
    );
    let answer: Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(answer["run"]["retraction"]["received"], 2);
    assert_eq!(answer["run"]["retraction"]["retracted"], 2);
    assert_eq!(answer["run"]["retraction"]["detached"], 2);
    assert_eq!(
        answer,
        server.read(&format!("/v1/runs/{}", answer["run"]["run"]))
    );
    assert_eq!(
        server.read("/v1/info")["store"]["entity_types"][0]["retracted"],
        3
    );
}

#[test]
fn spool_retract_says_what_it_needs_and_what_the_server_refused() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());

    for (arguments, expected) in [
        (
            &["retract", "--type", "thing", "2"][..],
            "`--source` is needed",
        ),
        (&["retract", "--source", "shop", "2"], "`--type` is needed"),
        (
            &["retract", "--source", "shop", "--type", "thing"],
            "name the keys of the rows to take back",
        ),
        (
            &["retract", "--source", "shop", "--type", "thing", "--all"],
            "unknown option `--all`",
        ),
    ] {
        let (code, complaint) = refused(&server, arguments);
        assert_eq!(code, Some(2), "{arguments:?}");
        assert!(complaint.contains(expected), "{arguments:?}: {complaint}");
        assert!(complaint.contains("Usage: spool retract"), "{complaint}");
    }

    let (code, complaint) = refused(
        &server,
        &["retract", "--source", "nowhere", "--type", "thing", "2"],
    );
    assert_eq!(code, Some(1));
    assert!(complaint.contains("nowhere"), "{complaint}");

    let missing = dir.path().join("no-such-file");
    let (code, complaint) = refused(
        &server,
        &[
            "retract",
            "--source",
            "shop",
            "--type",
            "thing",
            "--keys",
            path(&missing),
        ],
    );
    assert_eq!(code, Some(1));
    assert!(complaint.contains("no-such-file"), "{complaint}");

    let output = spool()
        .args(["retract", "--source", "shop", "--type", "thing", "2"])
        .args(["--url", &format!("http://{}", server.address)])
        .env_remove("SPOOL_TOKEN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("asks for a token"),
        "{}",
        text(&output.stderr)
    );

    assert_eq!(server.read("/v1/runs?kind=retract")["runs"], json!([]));
}

#[test]
fn spool_show_says_what_was_taken_back() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());
    said(
        &server,
        &["retract", "--source", "shop", "--type", "thing", "2", "9"],
    );
    let run = server.read("/v1/runs?kind=retract")["runs"][0].clone();
    let number = run["run"].as_u64().unwrap();

    let overview = said(&server, &["show"]);
    has(
        &overview,
        "records     3, 0 without an identity, 1 taken back by their source",
    );

    let runs = said(&server, &["show", "runs", "--kind", "retract"]);
    assert_eq!(runs.lines().count(), 2, "{runs}");
    assert!(
        runs.contains(&format!("\n{number}    retract  succeeded  thing  ")),
        "{runs}"
    );

    let shown = said(&server, &["show", "run", &number.to_string()]);
    has(&shown, "kind        retract");
    has(&shown, "keys        2 named, 1 unknown");
    has(&shown, "taken back  1 record, 1 of them left an identity");
    assert!(
        shown.contains(&format!(
            "\ncommitted   log position {}, changeset ",
            run["retraction"]["committed"]["seq"]
        )),
        "{shown}"
    );

    let shown = said(&server, &["show", "record", "shop:2"]);
    has(
        &shown,
        &format!("identity  none: its source took it back in run {number}"),
    );
    let unassigned = said(&server, &["show", "records", "--unassigned"]);
    assert_eq!(unassigned, "no records\n");

    let run = load(&server, &[&["id", "code"], &["2", "anvil"]]);
    let shown = said(&server, &["show", "run", &run["run"].to_string()]);
    has(
        &shown,
        "came back  1 record the source had taken back, 1 of them to the identity they left",
    );
    assert!(
        shown.contains(&format!(
            "\ncommitted  log position {}, changeset ",
            run["load"]["committed"]["seq"]
        )),
        "{shown}"
    );
    let overview = said(&server, &["show"]);
    has(&overview, "records     3, 0 without an identity");
}

/// An anvil and a mallet that share an identity only through a third row,
/// which has the code of one and the colour of the other.
fn held_by_one(dir: &Path) -> Server {
    let project = dir.join("project");
    write_things(&project);
    for part in ["block", "match"] {
        std::fs::write(
            project.join(format!("entities/thing/rules/{part}/same_colour.sql")),
            "l.colour = r.colour\n",
        )
        .unwrap();
    }
    let server = Server::serve(&project, dir, 1);
    load(
        &server,
        &[
            &["id", "code", "colour"],
            &["1", "anvil", ""],
            &["2", "mallet", "blue"],
            &["3", "anvil", "blue"],
        ],
    );
    resolve(&server);
    server
}

#[test]
fn an_identity_that_a_retraction_left_without_what_held_it_together_is_shown_for_review() {
    let dir = tempfile::tempdir().unwrap();
    let server = held_by_one(dir.path());
    let identity = record(&server, "1")["identity"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(record(&server, "2")["identity"], identity.as_str());
    let id = |key: &str| record(&server, key)["id"].as_str().unwrap().to_owned();

    let printed = said(
        &server,
        &["retract", "--source", "shop", "--type", "thing", "3"],
    );
    let run = server.read("/v1/runs?kind=retract")["runs"][0].clone();
    assert_eq!(run["retraction"]["possible_splits"], 1);
    let number = run["run"].as_u64().unwrap();
    assert_eq!(
        printed.lines().last().unwrap(),
        "1 identity was left for review: what held its records together went through a record that was taken back. See `spool show review --status open`."
    );
    let shown = said(&server, &["show", "run", &number.to_string()]);
    has(
        &shown,
        "left for review  1 identity with records that nothing holds together now (`possible_split`)",
    );

    let list = said(&server, &["show", "review", "--status", "open"]);
    has(
        &list,
        &format!(
            "1 possible_split open thing run {number} 2 records of one identity in 2 groups that nothing holds together"
        ),
    );
    let item = said(&server, &["show", "review", "1"]);
    let (first, second) = match id("1") < id("2") {
        true => (("1", id("1")), ("2", id("2"))),
        false => (("2", id("2")), ("1", id("1"))),
    };
    assert_eq!(
        item,
        format!(
            "review item  1
kind         possible_split
status       open
type         thing
raised by    run {number}
identity     {identity}

Are these records still one thing?
They share an identity, and a record that held them together was taken back by its source.
Each group below still holds together. Nothing the store knows joins one group to another:
no match found by a run, no `must_link`, and no decision by a reviewer or an agent.

group 1
  {}  shop:{}  still in the identity

group 2
  {}  shop:{}  still in the identity

taken back by their source, and no longer in the identity
  {}  shop:3  in run {number}

what answers it
  they are different things  split the identity: POST /v1/identities/{identity}/split with the records of a group
  they are one thing         say so with a `must_link` between a record of each group: POST /v1/changesets
  leave it as it is          dismiss the item: POST /v1/review/1/dismiss
The review screen of the studio does each of these.
",
            first.1,
            first.0,
            second.1,
            second.0,
            id("3"),
        )
    );
    let bundle: Value =
        serde_json::from_str(&said(&server, &["show", "review", "1", "--json"])).unwrap();
    assert_eq!(bundle["item"], server.read("/v1/review/1"));
    assert_eq!(bundle["taken_back"][0]["id"], id("3").as_str());

    has(
        &said(&server, &["show", "record", "shop:1"]),
        "review item 1 (possible_split, open)",
    );
    let explanation = said(&server, &["explain", "shop:1", "shop:2"]);
    assert!(
        explanation.contains(
            "\nReview item 1 (`possible_split`) is open and names `shop:1`, `shop:2`: they share an identity, and what held them together went through a record that its source took back. Nothing the store knows says they are one thing now. Whether to split the identity is left to a reviewer.\n"
        ),
        "{explanation}"
    );

    // Splitting the identity answers the item, and nothing is left to do.
    let split = server.call(
        &format!("/v1/identities/{identity}/split"),
        json!({ "records": [second.1] }),
    );
    let item = said(&server, &["show", "review", "1"]);
    has(&item, "status       resolved");
    has(
        &item,
        &format!("resolved at  log position {}", split["committed"]["seq"]),
    );
    assert!(!item.contains("what answers it"), "{item}");
    has(
        &item,
        &format!(
            "{}  shop:{}  now in identity {}",
            second.1,
            second.0,
            record(&server, second.0)["identity"].as_str().unwrap()
        ),
    );
    assert_eq!(
        said(&server, &["show", "review", "--status", "open"]),
        "no review items\n"
    );
}

#[test]
fn a_row_that_comes_back_where_what_held_it_is_still_gone_is_left_for_review() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    for part in ["block", "match"] {
        std::fs::write(
            project.join(format!("entities/thing/rules/{part}/same_colour.sql")),
            "l.colour = r.colour\n",
        )
        .unwrap();
    }
    let server = Server::serve(&project, dir.path(), 1);
    // Two anvils, a blue anvil, and a blue mallet: only the blue anvil says
    // the mallet belongs with the anvils.
    load(
        &server,
        &[
            &["id", "code", "colour"],
            &["1", "anvil", ""],
            &["2", "anvil", ""],
            &["3", "anvil", "blue"],
            &["4", "mallet", "blue"],
        ],
    );
    resolve(&server);
    let identity = record(&server, "1")["identity"].clone();
    assert_eq!(record(&server, "4")["identity"], identity);

    for key in ["4", "3"] {
        let run = server.call(
            "/v1/retract",
            json!({ "source": "shop", "entity_type": "thing", "keys": [key] }),
        )["run"]
            .clone();
        assert_eq!(run["retraction"]["possible_splits"], 0);
    }
    assert_eq!(server.read("/v1/review?status=open")["items"], json!([]));

    // The mallet is sent again and goes back to the anvils, where nothing
    // holds it now.
    let run = load(
        &server,
        &[&["id", "code", "colour"], &["4", "mallet", "blue"]],
    );
    assert_eq!(run["load"]["returned"], 1);
    assert_eq!(run["load"]["possible_splits"], 1);
    assert_eq!(record(&server, "4")["identity"], identity);
    let shown = said(&server, &["show", "run", &run["run"].to_string()]);
    has(
        &shown,
        "left for review  1 identity with records that nothing holds together now (`possible_split`)",
    );
    let item = said(&server, &["show", "review", "1"]);
    has(&item, &format!("raised by  run {}", run["run"]));
    has(
        &item,
        &format!(
            "{}  shop:4  still in the identity",
            record(&server, "4")["id"].as_str().unwrap()
        ),
    );
    has(
        &item,
        &format!(
            "{}  shop:3  in run {}",
            record(&server, "3")["id"].as_str().unwrap(),
            record(&server, "3")["retracted"]
        ),
    );
}
