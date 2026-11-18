//! `spool load`, run against a server over a socket.

mod support;

use std::io::Write;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::{Value, json};
use support::*;

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
        .strip_prefix("spool: ")
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
next    `spool match` places them in identities
"
    );
    assert_eq!(beside, "");

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
next    `spool match` places them in identities
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
next    `spool match` places them in identities
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
next    `spool match` places it in an identity
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
            "spool: `--delimiter` needs one character such as `;`, or `tab`, not `::`\n"
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
next    `spool match` places them in identities
"
    );
    assert_eq!(beside, "4 rows sent\n");
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
        text(&output.stderr).starts_with("spool: `--batch` needs at least 1\n"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn load_says_what_is_wrong_with_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);
    let refused = |arguments: &[&str]| refused(&server, &project, arguments);

    assert_eq!(
        refused(&["samples/crm-3.csv"]),
        "there is no file `samples/crm-3.csv`"
    );
    assert_eq!(
        refused(&["samples"]),
        "`samples` is a directory: name a file of rows"
    );

    write(&project.join("empty.csv"), "");
    assert_eq!(
        refused(&["empty.csv"]),
        "empty.csv cannot be read as CSV: it is empty: the first line has to name the columns"
    );
    write(
        &project.join("ragged.csv"),
        "id,name,country\nc-1,Acorn Tools,GB\nc-2,Birch\n",
    );
    assert_eq!(
        refused(&["ragged.csv"]),
        "ragged.csv cannot be read as CSV: line 3 has 2 fields, and the first line names 3 \
         columns"
    );
    std::fs::write(
        project.join("latin.csv"),
        b"id,name,country\nc-1,Caf\xe9 Ltd,FR\n",
    )
    .unwrap();
    assert_eq!(
        refused(&["latin.csv"]),
        "latin.csv cannot be read as CSV: line 2: the text is not UTF-8. Save the file as UTF-8 \
         and send it again"
    );
    write(
        &project.join("ragged.tsv"),
        "id\tname\nc-1\tAcorn Tools\textra\n",
    );
    assert_eq!(
        refused(&["ragged.tsv"]),
        "ragged.tsv cannot be read as tab-separated text: line 2 has 3 fields, and the first \
         line names 2 columns"
    );

    // A column the source does not list, which is what a file of one's own
    // has first.
    write(
        &project.join("mine.csv"),
        "id,name,email\nc-1,Acorn Tools,hello@acorn.example\n",
    );
    assert_eq!(
        refused(&["mine.csv"]),
        "mine.csv has a column `email`, which sources/crm.yaml does not list. It lists `id`, \
         `name`, `country`. Add the column to that file to keep it, or leave it out of the rows."
    );
    write(
        &project.join("mine.csv"),
        "ID,name,email\nc-1,Acorn Tools,hello@acorn.example\n",
    );
    assert_eq!(
        refused(&["mine.csv"]),
        "mine.csv has the columns `ID`, `email`, which sources/crm.yaml does not list. It lists \
         `id`, `name`, `country`. Add them to that file to keep them, or leave them out of the \
         rows."
    );

    // None of that started a run or stored a row.
    assert_eq!(load_runs(&server), Vec::<Value>::new());
    assert_eq!(records(&server, "crm"), Vec::new());

    // A file with the first line alone has nothing to load, and that is no
    // failure: an export can be empty.
    write(&project.join("none.csv"), "id,name,country\n");
    let (printed, _) = load(&server, &project, &["none.csv"]);
    assert_eq!(
        printed,
        "loaded  0 rows: none.csv has no rows after its first line\n"
    );
    assert_eq!(load_runs(&server), Vec::<Value>::new());

    // Rows without the column that is the source's key are kept, and the
    // reader is told what they lack.
    write(
        &project.join("keyless.csv"),
        "name,country\nAcorn Tools,GB\n",
    );
    let (printed, beside) = load(&server, &project, &["keyless.csv"]);
    assert!(
        printed.starts_with("loaded  1 row into source `crm`, type `company`: 1 new\n"),
        "{printed}"
    );
    assert_eq!(
        beside,
        "note: keyless.csv has no column `id`, which is the key of source `crm`. Its rows are \
         kept without a key: none can be named as crm:<key>, and a changed row does not replace \
         the one loaded before.\n"
    );
    assert_eq!(records(&server, "crm")[0].0, Value::Null);

    // No file at all is a mistake in the command.
    let output = server.spool_in(&project, &["load"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).starts_with("spool: name the file to load\n\nUsage: spool load"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn load_says_when_no_server_answers_or_it_asks_for_a_token() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());

    // A port nothing listens on: the one a listener had a moment ago.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let output = spool()
        .current_dir(&project)
        .args(["load", "samples/crm-1.csv", "--url", &url])
        .output()
        .unwrap();
    let complaint = failure(&output, &["samples/crm-1.csv"]);
    assert!(
        complaint.starts_with(&format!("the server at {url} cannot be reached: ")),
        "{complaint}"
    );
    assert!(
        complaint
            .ends_with("\nStart one with `spool dev`, or name another with `--url` or SPOOL_URL."),
        "{complaint}"
    );

    // What is wrong with the file is said first: it needs no server.
    let output = spool()
        .current_dir(&project)
        .args(["load", "samples/crm-9.csv", "--url", &url])
        .output()
        .unwrap();
    assert_eq!(
        failure(&output, &["samples/crm-9.csv"]),
        "there is no file `samples/crm-9.csv`"
    );

    // A server that asks for a token, and gets none or a wrong one.
    let things = dir.path().join("things");
    write_things(&things);
    let server = Server::serve(&things, dir.path(), 1);
    write(&things.join("rows.csv"), "id,code\n1,anvil\n");
    let url = format!("http://{}", server.address);
    let without = spool()
        .current_dir(&things)
        .args(["load", "rows.csv", "--source", "shop", "--url", &url])
        .output()
        .unwrap();
    let complaint = failure(&without, &["rows.csv"]);
    assert!(
        complaint.starts_with(&format!("the server at {url} asks for a token: ")),
        "{complaint}"
    );
    let wrong = spool()
        .current_dir(&things)
        .args(["load", "rows.csv", "--source", "shop", "--url", &url])
        .args(["--token", "not-one"])
        .output()
        .unwrap();
    let complaint = failure(&wrong, &["rows.csv"]);
    assert!(
        complaint.starts_with(&format!("the server at {url} did not accept the token: ")),
        "{complaint}"
    );
    // With its token, in the environment or as an option.
    let (printed, _) = load(&server, &things, &["rows.csv", "--source", "shop"]);
    assert_eq!(
        printed,
        "\
loaded  1 row into source `shop`, type `thing`: 1 new
next    `spool match` places it in an identity
"
    );
    let named = spool()
        .current_dir(&things)
        .args(["load", "rows.csv", "--source", "shop", "--url", &url])
        .args(["--token", server.token()])
        .output()
        .unwrap();
    assert!(named.status.success(), "{}", text(&named.stderr));
    assert_eq!(
        text(&named.stdout),
        "loaded  1 row into source `shop`, type `thing`: 0 new, 1 already held\n"
    );
}

#[test]
fn load_asks_which_source_and_type_when_the_project_has_several() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_two_of_each(&project);
    let server = Server::serve(&project, dir.path(), 1);
    write(&project.join("rows.csv"), "id,name\n1,Ada\n");
    let refused = |arguments: &[&str]| refused(&server, &project, arguments);

    assert_eq!(
        refused(&["rows.csv"]),
        "the project has the sources `crm`, `registry` and `spare`, and rows.csv is named after \
         none of them: say which its rows come from with `--source`"
    );
    assert_eq!(
        refused(&["rows.csv", "--source", "crm"]),
        "say which entity type the rows are records of with `--type`: source `crm` is read by \
         `company`, `person`"
    );
    assert_eq!(
        refused(&["rows.csv", "--type", "company"]),
        "`company` reads the sources `crm` and `registry`, and rows.csv is named after neither: \
         say which its rows come from with `--source`"
    );
    // Standard input has no name to go by.
    let mut child = server
        .spool_in(&project, &["load", "-", "--type", "company"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"id,name\n1,Ada\n")
        .unwrap();
    assert_eq!(
        failure(&child.wait_with_output().unwrap(), &["-"]),
        "`company` reads the sources `crm` and `registry`: say which the rows come from with \
         `--source`"
    );

    // Names the project does not have, and a pair that does not go together.
    assert_eq!(
        refused(&["rows.csv", "--source", "erp"]),
        "the project has no source `erp`: there is no sources/erp.yaml. It has `crm`, \
         `registry`, `spare`."
    );
    assert_eq!(
        refused(&["rows.csv", "--type", "place"]),
        "the project has no entity type `place`. It has `company`, `person`."
    );
    assert_eq!(
        refused(&["rows.csv", "--source", "registry", "--type", "person"]),
        "the mapping of `person` has nothing for source `registry`, so its rows cannot be \
         `person` records. They can be loaded as `company`."
    );
    assert_eq!(
        refused(&["rows.csv", "--source", "spare"]),
        "no entity type reads source `spare`, so its rows cannot be loaded yet. Add the source \
         to the `mapping` in an entities/<type>/entity.yaml."
    );
    assert_eq!(
        refused(&["rows.csv", "--source", "spare", "--type", "person"]),
        "the mapping of `person` has nothing for source `spare`: add it to \
         entities/person/entity.yaml"
    );
    assert_eq!(load_runs(&server), Vec::<Value>::new());

    // One of the two is enough where it leaves one choice of the other, and
    // the next step names the type, since `spool match` would ask.
    let (printed, _) = load(&server, &project, &["rows.csv", "--type", "person"]);
    assert_eq!(
        printed,
        "\
loaded  1 row into source `crm`, type `person`: 1 new
next    `spool match --type person` places it in an identity
"
    );
    let (printed, _) = load(&server, &project, &["rows.csv", "--source", "registry"]);
    assert_eq!(
        printed,
        "\
loaded  1 row into source `registry`, type `company`: 1 new
next    `spool match --type company` places it in an identity
"
    );
    // A file named after a source holds that source's rows: the name up to
    // the first character that is no letter or digit, wherever the file is.
    write(
        &project.join("exports/registry-2024.csv"),
        "id,name\n7,Lin\n",
    );
    let (printed, _) = load(&server, &project, &["exports/registry-2024.csv"]);
    assert!(
        printed.starts_with("loaded  1 row into source `registry`, type `company`: 1 new\n"),
        "{printed}"
    );
    // The name says the source and nothing of the type, and a name that
    // only begins like a source's is not after it.
    write(&project.join("crm.csv"), "id,name\n8,Kay\n");
    assert_eq!(
        refused(&["crm.csv"]),
        "say which entity type the rows are records of with `--type`: source `crm` is read by \
         `company`, `person`"
    );
    write(&project.join("crms.csv"), "id,name\n8,Kay\n");
    assert_eq!(
        refused(&["crms.csv", "--type", "company"]),
        "`company` reads the sources `crm` and `registry`, and crms.csv is named after neither: \
         say which its rows come from with `--source`"
    );
    // What is asked for is what is done, whatever the file is called.
    let (printed, _) = load(&server, &project, &["crm.csv", "--source", "registry"]);
    assert!(
        printed.starts_with("loaded  1 row into source `registry`, type `company`: 1 new\n"),
        "{printed}"
    );

    write(&project.join("more.csv"), "id,name\n2,Grace\n");
    let (printed, _) = load(
        &server,
        &project,
        &["more.csv", "--source", "crm", "--type", "company"],
    );
    assert!(
        printed.starts_with("loaded  1 row into source `crm`, type `company`: 1 new\n"),
        "{printed}"
    );

    // What the server refuses is said as it says it: a row is a record of
    // one entity type.
    assert_eq!(
        refused(&["rows.csv", "--source", "crm", "--type", "company"]),
        "load rejected: a row with the same source, key and content is already stored as \
         entity type `person`"
    );
}

/// A new project with a second source, of what is found in documents, and a
/// document: a project whose rows and whose text are both records.
fn companies_with_notes(dir: &Path) -> std::path::PathBuf {
    let project = init_companies(dir);
    write(
        &project.join("sources/notes.yaml"),
        "# What a record found in a note may say.\ncolumns: [name]\n",
    );
    let entity = project.join("entities/company/entity.yaml");
    let mut yaml = std::fs::read_to_string(&entity).unwrap();
    yaml += "  notes:\n    name: name\n    name_key: { from: name, transform: name_key }\n";
    write(&entity, &yaml);
    write(
        &project.join("samples/notes-1.txt"),
        "Call with Dana Reyes on 12 November. She has moved to Acorn Tools Ltd and runs\n\
         purchasing there. They now ship through Larkfield Logistics Ltd, and she asked\n\
         whether we work with Brightwater Foods Ltd, their new supplier.\n",
    );
    project
}

#[test]
fn a_project_with_a_source_of_documents_loads_its_rows_by_the_name_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let project = companies_with_notes(dir.path());
    let server = Server::dev(&project);
    let refused = |arguments: &[&str]| refused(&server, &project, arguments);

    // The sample rows need no `--source`: the files are named after theirs.
    for (file, rows) in [("samples/crm-1.csv", 6), ("samples/crm-2.csv", 3)] {
        let (printed, _) = load(&server, &project, &[file]);
        assert_eq!(
            printed,
            format!(
                "\
loaded  {rows} rows into source `crm`, type `company`: {rows} new
next    `spool match` places them in identities
"
            )
        );
    }
    assert_eq!(records(&server, "crm").len(), 9);
    assert_eq!(records(&server, "notes"), Vec::new());

    // A document is not rows, and another command reads it.
    assert_eq!(
        refused(&["samples/notes-1.txt"]),
        "samples/notes-1.txt does not look like rows: its first line names no column that a \
         source of the project lists. `spool load` reads CSV, with the names of the columns on \
         its first line.\n\
         If it is a document, `spool extract samples/notes-1.txt` reads it: an extractor finds \
         the records in its text."
    );
    write(
        &project.join("call.md"),
        "# A call\n\nWith Acorn Tools Ltd.\n",
    );
    assert!(
        refused(&["call.md"]).ends_with("\nIf it is a document, `spool extract call.md` reads it: an extractor finds the records in its text."),
    );
    // A first line that cannot name columns is said as it is of any file,
    // with where a document goes.
    write(&project.join("letter.txt"), "Dear Dana,\n\nThank you.\n");
    assert_eq!(
        refused(&["letter.txt"]),
        "letter.txt cannot be read as CSV: line 1: column 2 has no name. The first line has to \
         name the columns\n\
         If it is a document, `spool extract letter.txt` reads it: an extractor finds the \
         records in its text."
    );
    // Nothing is said of documents where they are not the question: a file
    // with nothing in it, a file of rows, and one said to hold rows.
    write(&project.join("empty.txt"), "");
    assert_eq!(
        refused(&["empty.txt"]),
        "empty.txt cannot be read as CSV: it is empty: the first line has to name the columns"
    );
    assert_eq!(
        refused(&["samples/notes-1.txt", "--delimiter", ","]),
        "samples/notes-1.txt has a column `Call with Dana Reyes on 12 November. She has moved \
         to Acorn Tools Ltd and runs`, which sources/notes.yaml does not list. It lists `name`. \
         Add the column to that file to keep it, or leave it out of the rows."
    );
    assert_eq!(load_runs(&server).len(), 2);

    // Rows can be kept in a text file, and rows of the other source are
    // loaded as any are.
    write(
        &project.join("crm-export.txt"),
        "id,name\nc-30,Zennor Glass\n",
    );
    let (printed, _) = load(&server, &project, &["crm-export.txt"]);
    assert!(
        printed.starts_with("loaded  1 row into source `crm`, type `company`: 1 new\n"),
        "{printed}"
    );
    write(&project.join("notes.csv"), "name\nAcorn Tools Ltd\n");
    let (printed, _) = load(&server, &project, &["notes.csv"]);
    assert!(
        printed.starts_with("loaded  1 row into source `notes`, type `company`: 1 new\n"),
        "{printed}"
    );

    // A file named after neither source needs to be told.
    write(
        &project.join("more.csv"),
        "id,name,country\nc-31,Yarrow Mills,GB\n",
    );
    assert_eq!(
        refused(&["more.csv"]),
        "the project has the sources `crm` and `notes`, and more.csv is named after neither: \
         say which its rows come from with `--source`"
    );
    let (printed, _) = load(&server, &project, &["more.csv", "--source", "crm"]);
    assert!(
        printed.starts_with("loaded  1 row into source `crm`, type `company`: 1 new\n"),
        "{printed}"
    );
}
