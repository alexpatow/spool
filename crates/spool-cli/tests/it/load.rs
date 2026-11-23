//! `spool load`, run against a server over a socket.

use std::io::Write;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::{Value, json};

use crate::support::*;

mod choosing;
mod failures;

/// `spool load <arguments>` in `dir`, which has to succeed: what it printed
/// and what it said beside.
fn load(server: &Server, dir: &Path, arguments: &[&str]) -> (String, String) {
    let output = server
        .spool_in(dir, &[&["load"], arguments].concat())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "spool load {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    (text(&output.stdout), text(&output.stderr))
}

/// What `spool load <arguments>` in `dir` complained of. It has to fail
/// with status 1 and print nothing.
fn refused(server: &Server, dir: &Path, arguments: &[&str]) -> String {
    let output = server
        .spool_in(dir, &[&["load"], arguments].concat())
        .output()
        .unwrap();
    failure(&output, arguments)
}

fn failure(output: &Output, arguments: &[&str]) -> String {
    assert_eq!(
        output.status.code(),
        Some(1),
        "spool load {arguments:?}: {}",
        text(&output.stderr)
    );
    assert_eq!(text(&output.stdout), "", "nothing is printed on a failure");
    let complaint = text(&output.stderr);
    complaint
        .strip_prefix("error: ")
        .unwrap_or_else(|| panic!("{complaint}"))
        .trim_end()
        .to_owned()
}

fn write(file: &Path, contents: &str) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, contents).unwrap();
}

/// The records of a source in the order the server lists them: each one's
/// key and payload.
fn records(server: &Server, source: &str) -> Vec<(Value, Value)> {
    server.read(&format!("/v1/records?source={source}"))["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| (record["key"].clone(), record["payload"].clone()))
        .collect()
}

fn load_runs(server: &Server) -> Vec<Value> {
    server.read("/v1/runs?kind=load")["runs"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn add_returns_the_identity_of_the_exact_row_version() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);
    let output = server
        .spool_in(
            &project,
            &[
                "add",
                "--source",
                "shop",
                "--type",
                "thing",
                r#"{"id":"1","code":"anvil","colour":"black"}"#,
                "--json",
            ],
        )
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let added: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(added["identity"].is_string(), "{added}");
    assert!(added["match_run"].is_number(), "{added}");
    let request = |colour, replace| {
        json!({
            "source": "shop", "entity_type": "thing", "replace": replace,
            "row": { "id": "1", "code": "anvil", "colour": colour }
        })
    };
    server
        .request("POST", "/v1/records")
        .json(&request("red", false))
        .send()
        .error(400, "invalid");
    let red = server.call("/v1/records", request("red", true));
    let black = server.call("/v1/records", request("black", true));
    assert_ne!(black["record"], added["record"], "the row changed back");
    assert_ne!(black["record"], red["record"]);
    assert_eq!(black["identity"], added["identity"]);
    assert_eq!(red["identity"], black["identity"]);
    let retry = server.call("/v1/records", request("black", false));
    assert_eq!(retry["record"], black["record"]);
    assert_eq!(retry["identity"], black["identity"]);
    assert!(retry["match_run"].is_null(), "{retry}");
    assert!(server.call("/v1/match", json!({ "entity_type": "thing" }))["run"].is_null());
}

#[test]
fn load_sends_the_rows_of_a_file_and_says_what_became_of_them() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);

    // The command a new project is left with, as its reader types it.
    let (printed, beside) = load(&server, &project, &["samples/crm-1.csv"]);
    assert_eq!(
        printed,
        "\
loaded  6 rows into source `crm`, type `company`: 6 new
"
    );
    assert_eq!(beside, "next: `spool match` places them in identities\n");

    // Each row is a record, with every value as the text the file has, and
    // the ID the same row has when it comes as JSON.
    let held = records(&server, "crm");
    let keys: Vec<&str> = held.iter().map(|(key, _)| key.as_str().unwrap()).collect();
    assert_eq!(keys, ["c-1", "c-2", "c-3", "c-4", "c-5", "c-6"]);
    assert_eq!(
        held[0].1,
        json!({ "id": "c-1", "name": "Acorn Tools", "country": "GB" })
    );
    let first = server.read("/v1/records?source=crm&key=c-1")["records"][0].clone();
    assert_eq!(first["id"], "ab8e9d59-0aaa-96b9-ddb7-db0bff7a9356");
    assert_eq!(first["identity"], Value::Null, "loading does not match");

    // The same file again adds nothing, and leaves nothing to match.
    let (printed, _) = load(&server, &project, &["samples/crm-1.csv"]);
    assert_eq!(
        printed,
        "loaded  6 rows into source `crm`, type `company`: 0 new, 6 already held\n"
    );

    // A row that changed is a new version of it, beside a row that is new
    // and one that is as it was.
    write(
        &project.join("changed.csv"),
        "id,name,country\nc-1,Acorn Tools Limited,GB\nc-2,ACORN TOOLS,GB\nc-10,Juniper Works,GB\n",
    );
    let (printed, _) = load(&server, &project, &["changed.csv"]);
    assert_eq!(
        printed,
        "\
loaded  3 rows into source `crm`, type `company`: 1 new, 1 new version of a row held before, 1 already held
"
    );
    let versions = server.read("/v1/records?source=crm&key=c-1")["records"].clone();
    assert_eq!(versions.as_array().unwrap().len(), 2, "{versions}");
    assert_eq!(versions[1]["supersedes"], versions[0]["id"]);

    // Named in full, the source and the type are the ones it chose itself.
    let (printed, _) = load(
        &server,
        &project,
        &["samples/crm-2.csv", "--source", "crm", "--type", "company"],
    );
    assert_eq!(
        printed,
        "\
loaded  3 rows into source `crm`, type `company`: 3 new
"
    );

    // With `--json`, the runs the server answered with.
    let (printed, _) = load(&server, &project, &["samples/crm-2.csv", "--json"]);
    let answer: Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(answer["source"], "crm");
    assert_eq!(answer["entity_type"], "company");
    assert_eq!(answer["rows"], 3);
    let runs = answer["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "{answer}");
    assert_eq!(runs[0]["kind"], "load");
    assert_eq!(runs[0]["load"]["duplicates"], 3);
    assert_eq!(
        runs[0],
        server.read(&format!("/v1/runs/{}", runs[0]["run"]))["run"]
    );
}

#[test]
fn a_row_that_is_loaded_as_it_was_before_it_changed_is_that_again() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);
    let named = |reference: &str| -> Value {
        let output = server
            .spool_in(&project, &["show", "records", reference, "--json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output.stderr));
        serde_json::from_str(&text(&output.stdout)).unwrap()
    };
    load(&server, &project, &["samples/crm-1.csv"]);
    server.call("/v1/match", json!({ "entity_type": "company" }));
    let first = named("crm:c-2");

    write(
        &project.join("changed.csv"),
        "id,name,country\nc-2,Acorn Tools of Sheffield,GB\n",
    );
    load(&server, &project, &["changed.csv"]);
    server.call("/v1/match", json!({ "entity_type": "company" }));
    let changed = named("crm:c-2");
    assert_eq!(
        changed["record"]["payload"]["name"],
        "Acorn Tools of Sheffield"
    );

    // The first file again: one of its rows is not what the store holds
    // now, and the others are.
    let (printed, _) = load(&server, &project, &["samples/crm-1.csv"]);
    assert_eq!(
        printed,
        "\
loaded  6 rows into source `crm`, type `company`: 0 new, 1 new version of a row held before, 5 already held
"
    );
    server.call("/v1/match", json!({ "entity_type": "company" }));

    // The key names the row as it is now, which is as it was at first, in
    // the identity it was in all along. Each step is a version.
    let back = named("crm:c-2");
    assert_eq!(back["record"]["payload"], first["record"]["payload"]);
    assert_eq!(back["record"]["supersedes"], changed["record"]["id"]);
    assert_ne!(back["record"]["id"], first["record"]["id"]);
    assert_eq!(back["record"]["identity"], first["record"]["identity"]);
    assert_eq!(changed["record"]["identity"], first["record"]["identity"]);
    let versions = server.read("/v1/records?source=crm&key=c-2")["records"].clone();
    assert_eq!(versions.as_array().unwrap().len(), 3, "{versions}");

    // What the row holds now, sent again, adds nothing.
    let (printed, _) = load(&server, &project, &["samples/crm-1.csv"]);
    assert_eq!(
        printed,
        "loaded  6 rows into source `crm`, type `company`: 0 new, 6 already held\n"
    );
}

#[test]
fn an_empty_field_is_a_value_the_row_does_not_have() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);

    // The second row has no country, in the two ways a file says so.
    write(
        &project.join("gaps.csv"),
        "id,name,country\nc-1,Acorn Tools,GB\nc-2,Acorn Tools,\nc-3,Acorn Tools,\"\"\n",
    );
    load(&server, &project, &["gaps.csv"]);
    let held = records(&server, "crm");
    assert_eq!(held[1].1, json!({ "id": "c-2", "name": "Acorn Tools" }));
    assert_eq!(held[2].1, json!({ "id": "c-3", "name": "Acorn Tools" }));

    // So the rule that keeps two countries apart says nothing about them:
    // with `""` for a country it would hold against every row that has one.
    let matched = server.call("/v1/match", json!({ "entity_type": "company" }));
    assert_eq!(
        matched["run"]["resolution"]["judgements"],
        json!({ "match": 3, "no_match": 0, "unsure": 0 })
    );
    let identities: Vec<Value> = server.read("/v1/records?source=crm")["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["identity"].clone())
        .collect();
    assert!(identities[0].is_string(), "{identities:?}");
    assert_eq!(identities[1], identities[0]);
    assert_eq!(identities[2], identities[0]);
}

#[test]
fn load_reads_standard_input_and_other_characters_between_the_fields() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);

    // Standard input, as a pipeline gives it.
    let mut child = server
        .spool_in(&project, &["load", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"id,name,country\nc-1,\"Acorn, Tools\",GB\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "\
loaded  1 row into source `crm`, type `company`: 1 new
"
    );

    // Tabs, by the name of the file and by the option.
    write(
        &project.join("rows.tsv"),
        "id\tname\tcountry\nc-2\tBirch, Ash and Co\tGB\n",
    );
    load(&server, &project, &["rows.tsv"]);
    write(
        &project.join("tabs.txt"),
        "id\tname\tcountry\nc-3\tCedar Ltd\tGB\n",
    );
    load(&server, &project, &["tabs.txt", "--delimiter", "tab"]);
    // Semicolons, as a spreadsheet writes them where a comma is the
    // decimal mark.
    write(
        &project.join("semicolons.csv"),
        "id;name;country\nc-4;Dogwood, Elm;GB\n",
    );
    load(&server, &project, &["semicolons.csv", "--delimiter", ";"]);

    let names: Vec<Value> = records(&server, "crm")
        .into_iter()
        .map(|(_, payload)| payload["name"].clone())
        .collect();
    assert_eq!(
        names,
        [
            "Acorn, Tools",
            "Birch, Ash and Co",
            "Cedar Ltd",
            "Dogwood, Elm"
        ]
    );

    // Without the option, the first line is one column the source does not
    // list, and the complaint says which option is missing.
    assert_eq!(
        refused(&server, &project, &["semicolons.csv"]),
        "the first line of semicolons.csv is one column, `id;name;country`, and \
         sources/crm.yaml lists `id`, `name`, `country`. If `;` stands between its fields, add \
         `--delimiter ';'`."
    );
    assert_eq!(
        refused(&server, &project, &["tabs.txt"]),
        "the first line of tabs.txt is one column, `id\tname\tcountry`, and sources/crm.yaml \
         lists `id`, `name`, `country`. If a tab stands between its fields, add `--delimiter \
         tab`."
    );
    let output = server
        .spool_in(&project, &["load", "rows.tsv", "--delimiter", "::"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).starts_with(
            "error: `--delimiter` needs one character such as `;`, or `tab`, not `::`\n"
        ),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn a_file_is_sent_in_parts() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);

    // Six rows, at most four in a request. It says how far it is on the way.
    let (printed, beside) = load(&server, &project, &["samples/crm-1.csv", "--batch", "4"]);
    assert_eq!(
        printed,
        "\
loaded  6 rows into source `crm`, type `company`: 6 new
"
    );
    // Piped, no bar is drawn: what it says is the same as for one part.
    assert_eq!(beside, "next: `spool match` places them in identities\n");
    let runs = load_runs(&server);
    let received: Vec<&Value> = runs.iter().map(|run| &run["load"]["received"]).collect();
    assert_eq!(received, [&json!(2), &json!(4)], "the latest run first");
    assert_eq!(records(&server, "crm").len(), 6);

    // The counts are those of every part, and `--json` has a run for each.
    let (printed, beside) = load(
        &server,
        &project,
        &["samples/crm-1.csv", "--batch", "1", "--json"],
    );
    assert_eq!(beside, "");
    let answer: Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(answer["rows"], 6);
    assert_eq!(answer["runs"].as_array().unwrap().len(), 6);

    // A line that cannot be read after rows that could: what was sent
    // before it stays, and the complaint says so.
    write(
        &project.join("broken.csv"),
        "id,name,country\nc-7,Fir Trading,GB\nc-8,Gorse Ltd,GB\nc-9,Hazel,GB,extra\n",
    );
    let complaint = refused(&server, &project, &["broken.csv", "--batch", "2"]);
    let run = load_runs(&server)[0]["run"].clone();
    assert_eq!(
        complaint,
        format!(
            "broken.csv cannot be read as CSV: line 4 has 4 fields, and the first line names 3 \
             columns\nThis was after the first 2 rows of broken.csv were loaded, in run {run}. \
             In what the server says, rows are counted from the one after them. Loading the \
             file again sends them once more and adds nothing."
        )
    );
    assert_eq!(records(&server, "crm").len(), 8);
    // After several parts, the runs are said as the first and the last.
    write(
        &project.join("late.csv"),
        "id,name,country\nc-40,A,GB\nc-41,B,GB\nc-42,C,GB\nc-43,D,GB,extra\n",
    );
    let before = load_runs(&server).len();
    let output = server
        .spool_in(&project, &["load", "late.csv", "--batch", "1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let complaint = text(&output.stderr);
    let runs = load_runs(&server);
    assert_eq!(runs.len(), before + 3);
    // The latest run is listed first.
    let (first, last) = (runs[2]["run"].clone(), runs[0]["run"].clone());
    assert!(
        complaint.contains(&format!(
            "\nThis was after the first 3 rows of late.csv were loaded, in 3 runs, {first} to \
             {last}. "
        )),
        "{complaint}"
    );

    let output = server
        .spool_in(&project, &["load", "samples/crm-1.csv", "--batch", "0"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("invalid value '0' for '--batch <ROWS>': 0 is not in 1.."),
        "{}",
        text(&output.stderr)
    );
}
