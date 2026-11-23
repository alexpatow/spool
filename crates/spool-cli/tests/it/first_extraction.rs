//! The first extraction of a new project: `spool init` writes a note and an
//! extractor that finds company names in it with a small model, and
//! `spool extract` has the server read the note. The same step over HTTP,
//! what a reader sees while a first run fetches what it needs, and what a
//! reader is told when a step cannot be taken.
//!
//! An extractor is a file `uv` or `bun` runs, so the tests that run one are
//! ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool-cli --test it -- --include-ignored first_extraction::
//! ```
//!
//! Those put a stand-in in the sample extractor's place, which finds the
//! same names in the sample note and needs no model. They are what CI runs.
//!
//! Two tests run the sample extractor as `spool init` writes it, with its
//! model, which runs on this machine and costs nothing. The model is about
//! 330 MB to fetch with what runs it, so these two are run by hand, and do
//! nothing unless `SPOOL_TEST_MODEL` is set:
//!
//! ```sh
//! SPOOL_TEST_MODEL=1 cargo test -p spool-cli --test it -- --include-ignored sample_model
//! ```
//!
//! One of the two fetches the model each time it runs, into caches of its
//! own.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::support::*;

mod batches;
mod models;
mod over_http;
mod taken_back;

const NOTE: &str = "\
Call with Dana Reyes on 12 November. She has moved to Acorn Tools Ltd and runs
purchasing there. They now ship through Larkfield Logistics Ltd, and she asked
whether we work with Brightwater Foods Ltd, their new supplier.
";

/// What `spool extract samples/notes-1.txt` prints in a new project.
const FIRST: &str = "\
source     notes
read       1 document from samples/notes-1.txt
loaded     1 new, 0 already held, 0 new versions
extractor  companies
found      3 records in 1 document: 3 new, 0 already held

  company  Acorn Tools Ltd          notes:notes-1.txt[54-69]
  company  Larkfield Logistics Ltd  notes:notes-1.txt[119-142]
  company  Brightwater Foods Ltd    notes:notes-1.txt[179-200]
";

/// Stands in for the sample extractor where a test is not about its model:
/// it finds what ends in a legal form, which in the sample note is what the
/// model finds, and has nothing to fetch but what every Python file needs.
const BY_LEGAL_FORM: &str = r#"import re

from spool import extractor

NAME = re.compile(r"(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\b")

@extractor
def find(document):
    for found in NAME.finditer(document.text):
        yield document.mention("company", found.start(), found.end(), name=found.group())
"#;

/// Whether the tests that run the sample extractor's model were asked for.
/// They fetch about 330 MB, so `--include-ignored` alone does not run them:
/// see the top of this file.
fn model_asked_for() -> bool {
    let asked = std::env::var_os("SPOOL_TEST_MODEL").is_some_and(|value| !value.is_empty());
    if !asked {
        eprintln!("not run: this test runs a model, which SPOOL_TEST_MODEL=1 asks for");
    }
    asked
}

/// A new project whose extractor is the stand-in.
fn init_with_stand_in(dir: &Path) -> PathBuf {
    let project = init_companies(dir);
    write(&project.join("extractors/companies.py"), BY_LEGAL_FORM);
    project
}

/// What `spool <arguments>` printed in `dir`: its result, then what it said
/// beside on standard error. It has to succeed.
fn shown(server: &Server, dir: &Path, arguments: &[&str]) -> String {
    let output = server.spool_in(dir, arguments).output().unwrap();
    assert!(
        output.status.success(),
        "spool {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    format!("{}{}", text(&output.stdout), text(&output.stderr))
}

/// What `spool extract <arguments>` printed, which may have taken long
/// enough for it to say on standard error what is running, and how many
/// documents its batches kept. It has to succeed.
fn extracted(server: &Server, dir: &Path, arguments: &[&str]) -> String {
    let output = server
        .spool_in(dir, &[&["extract"], arguments].concat())
        .output()
        .unwrap();
    let said = text(&output.stderr);
    assert!(
        output.status.success(),
        "spool extract {arguments:?} failed:\n{}{said}",
        text(&output.stdout),
    );
    // All it says there, with no bar drawn where the output is piped, is
    // what the extractor said, how long a slow start took, what the
    // batches came to, and what to run next.
    assert!(
        said.lines().all(|line| {
            line.starts_with("             ")
                || line.starts_with("    Prepared ")
                || line.starts_with("   Extracted ")
                || line.starts_with("next: ")
        }),
        "{said}"
    );
    text(&output.stdout)
}

/// What `spool extract <arguments>` complained of. It has to fail with
/// status 1.
fn refused(server: &Server, dir: &Path, arguments: &[&str]) -> String {
    let output = server
        .spool_in(dir, &[&["extract"], arguments].concat())
        .output()
        .unwrap();
    complaint(&output, arguments)
}

fn complaint(output: &std::process::Output, arguments: &[&str]) -> String {
    assert_eq!(
        output.status.code(),
        Some(1),
        "spool extract {arguments:?}: {}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
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

/// The records of the source `notes`, in the order the server lists them.
fn found(server: &Server) -> Vec<Value> {
    server.read("/v1/records?source=notes")["records"]
        .as_array()
        .unwrap()
        .clone()
}

/// The identity of a record, by its source and key or by a mention's name.
fn identity_of(records: &[Value], text: &str) -> String {
    records
        .iter()
        .find(|record| record["evidence"]["text"] == text)
        .and_then(|record| record["identity"].as_str())
        .unwrap_or_else(|| panic!("`{text}` has no identity in {records:?}"))
        .to_owned()
}

fn runs(server: &Server, kind: &str) -> Vec<Value> {
    server.read(&format!("/v1/runs?kind={kind}"))["runs"]
        .as_array()
        .unwrap()
        .clone()
}

/// The sample extractor is a file a reader can run as it is and change:
/// what it depends on is said in the file, and so is what it looks for.
#[test]
fn the_extractor_of_a_new_project_names_what_it_needs_and_what_it_finds() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    assert_eq!(
        std::fs::read_to_string(project.join("samples/notes-1.txt")).unwrap(),
        NOTE
    );
    let extractor = std::fs::read_to_string(project.join("extractors/companies.py")).unwrap();
    // `uv` reads what to install from the top of the file. The Python is
    // one the model's library is built for, whichever is newest here.
    assert!(
        extractor.starts_with(
            "\
# /// script
# requires-python = \">=3.10,<3.13\"
# dependencies = [\"fast-gliner>=0.4.1,<0.5\"]
# ///
\"\"\"Finds the companies a document names, with a small model that runs here.
"
        ),
        "{extractor}"
    );
    for line in [
        // What to change to find something else.
        "LOOK_FOR = {\"company\": \"company\"}",
        // The model is named with the version it was tried with, and is
        // loaded as the file starts, where `spool build --install` starts it.
        "MODEL = \"onnx-community/gliner_small-v2.1\"",
        "WEIGHTS = \"onnx/model_q4f16.onnx\"",
        "VERSION = \"8142fb00740ccea973e64b1272949ff48653df5e\"",
        "model = load()",
        "@extractor",
        "def find(document):",
    ] {
        assert!(
            extractor.lines().any(|found| found == line),
            "no `{line}` in:\n{extractor}"
        );
    }
    // It says what its first run fetches, as `spool init` does.
    assert!(
        extractor.contains("The first run fetches about 330 MB"),
        "{extractor}"
    );
    // No line is longer than an editor shows.
    let longest = extractor.lines().map(|line| line.chars().count()).max();
    assert!(longest <= Some(100), "{longest:?}");

    let source = std::fs::read_to_string(project.join("sources/notes.yaml")).unwrap();
    assert!(source.ends_with("\ncolumns: [name]\n"), "{source}");
    let listing = succeed(&["info", path(&project)]);
    assert!(
        listing.contains("extractor `companies` (python, extractors/companies.py)"),
        "{listing}"
    );
    assert!(listing.contains("sources   crm, notes"), "{listing}");
}

/// The steps a reader takes after `spool init`, with what each prints: the
/// text the README and the site show. Here the extractor is the stand-in,
/// which finds in the note what the model finds.
#[test]
#[ignore = "runs uv"]
fn the_note_of_a_new_project_is_read_and_what_it_names_joins_the_rows() {
    let dir = tempfile::tempdir().unwrap();
    the_steps_after_init(&init_with_stand_in(dir.path()));
}

/// The project's `.env` reaches the extractor, under what the server was
/// started with, and no value of it is printed.
#[test]
#[ignore = "runs uv"]
fn an_extractor_is_given_what_the_projects_env_file_sets() {
    const BY_FORM_FROM_ENV: &str = r#"import os
import re

from spool import extractor

if os.environ.get("SPOOL_TEST_SECRET") != "not-for-the-log":
    raise SystemExit("SPOOL_TEST_SECRET is not set")
NAME = re.compile(r"(?:[A-Z][A-Za-z]+ )+" + re.escape(os.environ["LEGAL_FORM"]) + r"\b")

@extractor
def find(document):
    for found in NAME.finditer(document.text):
        yield document.mention("company", found.start(), found.end(), name=found.group())
"#;
    const ENV: &str =
        "# The form that names end in.\nLEGAL_FORM=Ltd\nSPOOL_TEST_SECRET='not-for-the-log'\n";
    let dir = tempfile::tempdir().unwrap();

    let project = init_companies(&dir.path().join("first"));
    write(&project.join("extractors/companies.py"), BY_FORM_FROM_ENV);
    write(&project.join(".env"), ENV);
    let server = Server::dev(&project);
    let output = extracted(&server, &project, &["samples/notes-1.txt"]);
    assert!(
        output.contains("\nfound      3 records in 1 document"),
        "{output}"
    );
    for line in server.stopped() {
        assert!(!line.contains("not-for-the-log"), "{line}");
    }

    // A variable the server was started with keeps its value.
    let project = init_companies(&dir.path().join("second"));
    write(&project.join("extractors/companies.py"), BY_FORM_FROM_ENV);
    write(&project.join(".env"), ENV);
    let server = Server::start_in(
        &[
            "dev",
            path(&project),
            "--listen",
            "127.0.0.1:0",
            "--seed",
            "1",
        ],
        &[("LEGAL_FORM", "GmbH")],
    )
    .unwrap_or_else(|stopped| panic!("the server did not start:\n{}", stopped.output));
    let output = extracted(&server, &project, &["samples/notes-1.txt"]);
    assert!(
        output.contains("\nfound      0 records in 1 document"),
        "{output}"
    );
    server.stop();
}

/// The same steps with the extractor `spool init` wrote, whose model reads
/// the note. By hand: see the top of this file.
#[test]
#[ignore = "runs the sample extractor's model, and only with SPOOL_TEST_MODEL=1"]
fn the_sample_model_reads_the_note_and_what_it_names_joins_the_rows() {
    if !model_asked_for() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    the_steps_after_init(&init_companies(dir.path()));
}

#[expect(clippy::too_many_lines)]
fn the_steps_after_init(project: &Path) {
    let server = Server::dev(project);
    let shown = |arguments: &[&str]| shown(&server, project, arguments);

    // The rows first, as the reader has them by now.
    shown(&["load", "samples/crm-1.csv"]);
    shown(&["match"]);
    let rows = server.read("/v1/records?source=crm")["records"].clone();
    let row = |key: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|record| record["key"] == key)
            .and_then(|record| record["identity"].as_str())
            .unwrap()
            .to_owned()
    };

    // The step: one command, which needs no source and no extractor named.
    // With the sample extractor, on a machine that has not run it before,
    // this is where the model is fetched.
    assert_eq!(extracted(&server, project, &["samples/notes-1.txt"]), FIRST);
    // Nobody signs in to a development server, so what the command started
    // is the `dev` account's, as the load and the match before it are: the
    // load of the note, the extraction, and the load of what it found.
    let dev = server
        .request("GET", "/auth/session")
        .without("Authorization")
        .send()
        .ok();
    let dev = format!("user:{}", dev["person"]["id"].as_str().unwrap());
    let started: Vec<(Value, Value)> = server.read("/v1/runs")["runs"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .map(|run| (run["kind"].clone(), run["started_by"].clone()))
        .collect();
    assert_eq!(
        started,
        ["load", "match", "reconcile", "load", "extract", "load"]
            .map(|kind| (json!(kind), json!(dev)))
    );
    let run = shown(&[
        "show",
        "runs",
        &runs(&server, "extract")[0]["run"].to_string(),
    ]);
    assert!(run.contains(&format!("\nstarted by  {dev}\n")), "{run}");
    // Each record says where it was found, and the place is the text.
    let records = found(&server);
    let note: Vec<char> = NOTE.chars().collect();
    for (record, (start, end, name)) in records.iter().zip([
        (54, 69, "Acorn Tools Ltd"),
        (119, 142, "Larkfield Logistics Ltd"),
        (179, 200, "Brightwater Foods Ltd"),
    ]) {
        assert_eq!(record["payload"], json!({ "name": name }));
        assert_eq!(record["label"], name);
        assert_eq!(record["evidence"]["key"], "notes-1.txt");
        assert_eq!(record["evidence"]["extractor"], "companies");
        assert_eq!(
            (&record["evidence"]["start"], &record["evidence"]["end"]),
            (&json!(start), &json!(end))
        );
        assert_eq!(note[start..end].iter().collect::<String>(), name);
        assert_eq!(record["identity"], Value::Null);
    }

    // The next step is the one the rows took.
    let matched = shown(&["match"]);
    assert!(
        matched.ends_with(
            "\
examined    3 records, 4 candidate pairs
judgements  4 match, 0 no_match
placed      3 records, 1 identity created
"
        ),
        "{matched}"
    );
    // Two of the companies were known from the rows, under names that
    // differ from the note's by a legal form, and the note's records are in
    // the identities the rows made. The third is new.
    let records = found(&server);
    assert_eq!(identity_of(&records, "Acorn Tools Ltd"), row("c-1"));
    assert_eq!(identity_of(&records, "Larkfield Logistics Ltd"), row("c-4"));
    let brightwater = identity_of(&records, "Brightwater Foods Ltd");
    assert!(!["c-1", "c-4", "c-5", "c-6"].map(row).contains(&brightwater));
    // And the rows are where they were.
    assert_eq!(server.read("/v1/records?source=crm")["records"], rows);

    let listed = shown(&["show", "records", "--source", "notes"]);
    let lines: Vec<&str> = listed.lines().collect();
    assert!(
        lines[1].starts_with("notes:notes-1.txt[54-69]    Acorn Tools Ltd "),
        "{listed}"
    );
    assert!(lines[1].ends_with(&row("c-1")), "{listed}");
    // An identity lists a record of a note by its place in the note.
    let acorn = shown(&["show", "identities", &row("c-1")]);
    for member in ["  notes:notes-1.txt[54-69]  ", "  crm:c-1  ", "  crm:c-3  "] {
        assert!(acorn.contains(member), "no `{member}` in:\n{acorn}");
    }
    assert!(!acorn.contains('#'), "{acorn}");

    // The same command again has nothing to do, and says how to have it
    // done regardless. Reading the note again finds what was found.
    assert_eq!(
        shown(&["extract", "samples/notes-1.txt"]),
        "\
source     notes
read       1 document from samples/notes-1.txt
loaded     0 new, 1 already held, 0 new versions
extractor  companies
found      0 records: `companies` has read every document. `--again` reads them again.
"
    );
    assert_eq!(
        extracted(&server, project, &["samples/notes-1.txt", "--again"]),
        "\
source     notes
read       1 document from samples/notes-1.txt
loaded     0 new, 1 already held, 0 new versions
extractor  companies
found      3 records in 1 document: 0 new, 3 already held

  company  Acorn Tools Ltd          notes:notes-1.txt[54-69]
  company  Larkfield Logistics Ltd  notes:notes-1.txt[119-142]
  company  Brightwater Foods Ltd    notes:notes-1.txt[179-200]
"
    );
    // The same records, each found once more.
    let without_readings = |records: Vec<Value>| -> Vec<Value> {
        records
            .into_iter()
            .map(|mut record| {
                record.as_object_mut().unwrap().remove("found_by");
                record
            })
            .collect()
    };
    let again = found(&server);
    assert!(
        again
            .iter()
            .all(|record| record["found_by"].as_array().unwrap().len() == 2),
        "{again:?}"
    );
    assert_eq!(without_readings(again), without_readings(records));

    // A document longer than the sample's model reads at once is read in
    // pieces, and a name is found wherever it is: here after two thousand
    // words, with letters that are more than one byte before it.
    let long = format!(
        "{}Zoë Müller confirmed that Halden Group Inc had signed.\n",
        "Nothing of note was said in the weekly call. ".repeat(250)
    );
    write(&project.join("notes-long.txt"), &long);
    let read = extracted(&server, project, &["notes-long.txt"]);
    let start = long.chars().count() - "Halden Group Inc had signed.\n".chars().count();
    assert!(
        read.contains(&format!(
            "\n  company  Halden Group Inc  notes:notes-long.txt[{start}-{}]\n",
            start + "Halden Group Inc".chars().count()
        )),
        "{read}"
    );
    assert!(
        read.contains("found      1 record in 1 document: 1 new, 0 already held"),
        "{read}"
    );
    assert!(
        runs(&server, "extract")
            .iter()
            .all(|run| run["status"] == "succeeded")
    );
}

/// What `spool extract` does with a note that changed, with a document
/// that came another way, and with `--json`.
#[test]
#[ignore = "runs uv"]
fn extract_reads_a_changed_note_and_what_the_server_holds() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_with_stand_in(dir.path());
    let server = Server::dev(&project);
    assert_eq!(
        extracted(&server, &project, &["samples/notes-1.txt"]),
        FIRST
    );

    // A note that was edited is the next version of its document: what it
    // names now is new, and what it named at other places is superseded by
    // nothing, since a place is of one version.
    write(
        &project.join("samples/notes-1.txt"),
        &NOTE.replace("Brightwater Foods Ltd", "Brightwater Foods GmbH"),
    );
    let edited = extracted(&server, &project, &["samples/notes-1.txt"]);
    assert!(
        edited.starts_with(
            "\
source     notes
read       1 document from samples/notes-1.txt
loaded     0 new, 0 already held, 1 new version
extractor  companies
found      3 records in 1 document: 3 new, 0 already held

  company  Acorn Tools Ltd          notes:notes-1.txt[54-69]
  company  Larkfield Logistics Ltd  notes:notes-1.txt[119-142]
  company  Brightwater Foods GmbH   notes:notes-1.txt[179-201]
"
        ),
        "{edited}"
    );

    // With no file, the extractor reads what the server holds and it has
    // not read: here a document that came over HTTP.
    let sent = server
        .request("POST", "/v1/documents?source=notes&key=visit.txt")
        .header("Content-Type", "text/plain")
        .body(b"We visited Tidewater Marine LLC.")
        .send()
        .ok();
    assert_eq!(sent["run"]["load"]["inserted"], 1, "{sent}");
    assert_eq!(
        extracted(&server, &project, &[]),
        "\
extractor  companies
found      1 record in 1 document: 1 new, 0 already held
"
    );

    // `--json` prints the runs as the server answered them: those that
    // stored the documents, and one for each batch.
    write(&project.join("notes-2.txt"), "Acorn Tools Ltd rang back.\n");
    let answered: Value =
        serde_json::from_str(&extracted(&server, &project, &["notes-2.txt", "--json"])).unwrap();
    let [stored] = &answered["documents"].as_array().unwrap()[..] else {
        panic!("one run stored the documents: {answered}");
    };
    assert_eq!(stored["kind"], "load", "{answered}");
    assert_eq!(stored["load"]["inserted"], 1);
    let [batch] = &answered["extractions"].as_array().unwrap()[..] else {
        panic!("one batch: {answered}");
    };
    assert_eq!(batch["kind"], "extract");
    assert_eq!(batch["status"], "succeeded");
    assert_eq!(batch["extractor"]["name"], "companies");
    assert_eq!(batch["extraction"]["inserted"], 1);
    assert_eq!(batch["extraction"]["left"], 0);
    // A run that is over says nothing more.
    assert_eq!(batch["said"], Value::Null);

    // A file of the reader's own is read once it is told its source.
    write(&project.join("call.txt"), "We met Acorn Tools Ltd.\n");
    let read = extracted(&server, &project, &["call.txt", "--source", "notes"]);
    assert!(
        read.contains("\n  company  Acorn Tools Ltd  notes:call.txt[7-22]\n"),
        "{read}"
    );
    assert!(
        runs(&server, "extract")
            .iter()
            .all(|run| run["status"] == "succeeded")
    );
}

/// Writes `count` notes under `folder`, each naming one company, with
/// `slow` in the note numbered `slow`. Their names sort as their numbers.
fn notes(folder: &Path, count: usize, slow: Option<usize>) {
    for note in 1..=count {
        let word = if Some(note) == slow { "slow" } else { "quick" };
        write(
            &folder.join(format!("note-{note:02}.txt")),
            &format!("Note {note}, {word}: Acorn Tools Ltd called."),
        );
    }
}

/// Where a command is on the search path of this test.
fn which(command: &str) -> String {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(command))
        .find(|file| file.is_file())
        .unwrap_or_else(|| panic!("`{command}` is not on the search path"))
        .to_string_lossy()
        .into_owned()
}

/// A request a model's API was sent.
struct Asked {
    path: String,
    authorization: String,
    body: Value,
}

/// Stands in for a model's API: answers each chat completion with the next
/// of `answers` as the content of the model's message, and every one after
/// the last with the last. It reports what it was asked. No test asks a
/// model.
fn model(answers: Vec<Value>) -> (u16, mpsc::Receiver<Asked>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (asked, requests) = mpsc::channel();
    std::thread::spawn(move || {
        let mut answers = answers.into_iter().peekable();
        for stream in listener.incoming() {
            let answer = match answers.next() {
                Some(answer) if answers.peek().is_none() => {
                    answers = vec![answer.clone()].into_iter().peekable();
                    answer
                }
                Some(answer) => answer,
                None => Value::Null,
            };
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
            let (mut length, mut authorization) = (0, String::new());
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                let Some((name, value)) = line.trim_end().split_once(':') else {
                    break;
                };
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap(),
                    "authorization" => authorization = value.trim().to_owned(),
                    _ => {}
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let reply = json!({
                "choices": [{"message": {"role": "assistant", "content": answer}}]
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
            let sent = asked.send(Asked {
                path,
                authorization,
                body: serde_json::from_slice(&body).unwrap(),
            });
            if sent.is_err() {
                break;
            }
        }
    });
    (port, requests)
}
