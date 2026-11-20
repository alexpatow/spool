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

/// `spool dev` over a project, on a machine with nothing installed: the
/// search path is an empty directory, and so is the home directory, where
/// the installers of `uv` and `bun` put them.
fn dev_with_nothing_installed(project: &Path, dir: &Path) -> Server {
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    Server::start_in(
        &[
            "dev",
            path(project),
            "--listen",
            "127.0.0.1:0",
            "--seed",
            "1",
        ],
        &[
            ("PATH", path(&empty)),
            ("HOME", path(&empty)),
            ("SPOOL_UV", ""),
            ("SPOOL_BUN", ""),
        ],
    )
    .unwrap_or_else(|stopped| panic!("the server did not start:\n{}", stopped.output))
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

/// The directories of tasks that were kept under a project's `.spool`.
fn kept_tasks(project: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(project.join(".spool"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .flat_map(|dir| std::fs::read_dir(dir).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("run-"))
        })
        .collect()
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

/// A first run fetches what the extractor depends on, which can take
/// minutes, and the reader is shown what the file says meanwhile.
#[test]
#[ignore = "runs uv"]
fn a_run_that_takes_a_while_says_what_its_extractor_says() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    // Says what it fetches, as the sample does, and then shows a bar that
    // writes over itself, as the libraries that fetch models do.
    write(
        &project.join("extractors/companies.py"),
        &BY_LEGAL_FORM.replacen(
            "import re\n",
            "import re\nimport sys\nimport time\n\n\
             print(\"Fetching the model (245 MB), once.\", file=sys.stderr, flush=True)\n\
             for done in range(60):\n    \
                 print(f\"\\r{done} of 60\", end=\"\", file=sys.stderr, flush=True)\n    \
                 time.sleep(0.1)\n",
            1,
        ),
    );
    let server = Server::dev(&project);

    // The command says which file is running once the run has taken a
    // while, and under it what the file said last: its own line, and not a
    // bar that is still moving. The answer is on standard output as ever.
    let started = Instant::now();
    let output = server
        .spool_in(&project, &["extract", "samples/notes-1.txt"])
        .output()
        .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(6));
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), FIRST);
    // Where the output is piped, what the file said is said once the run
    // has ended, and then how long it took to start.
    let said = text(&output.stderr);
    let lines: Vec<&str> = said
        .lines()
        .filter(|line| !line.starts_with("next: "))
        .collect();
    assert_eq!(
        lines[0], "             Fetching the model (245 MB), once.",
        "{said}"
    );
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with("    Prepared extractors/companies.py in "),
        "{said}"
    );
    // A line is said once, however often it was the last one.
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("Fetching"))
            .count(),
        1,
        "{said}"
    );
    assert!(!said.contains("of 60"), "{said}");

    // Over HTTP the run answers at once when it is asked to, and says the
    // same under `said` while it runs.
    let started = server
        .request("POST", "/v1/extractions")
        .json(&json!({ "again": true, "wait_ms": 0 }))
        .send()
        .expect(202);
    assert_eq!(started["run"]["status"], "running", "{started}");
    assert_eq!(started["run"]["kind"], "extract");
    let run = started["run"]["run"].clone();
    let asked = Instant::now();
    let finished = loop {
        let entry = server.read(&format!("/v1/runs/{run}?wait_ms=200"))["run"].clone();
        if entry["status"] != "running" {
            break entry;
        }
        if entry["said"] != Value::Null {
            assert_eq!(entry["said"], "Fetching the model (245 MB), once.");
        }
        assert!(asked.elapsed() < Duration::from_secs(120), "{entry}");
    };
    assert_eq!(finished["status"], "succeeded", "{finished}");
    assert_eq!(finished["said"], Value::Null, "{finished}");
    assert_eq!(finished["extraction"]["mentions"], 3, "{finished}");
}

/// The stand-in, made to take `seconds` over each document, and longer
/// over one whose text has `slow` in it: as long as `slow_seconds`, after
/// it writes a file `slow-started` in the project.
fn slow_stand_in(seconds: f64, slow_seconds: u32) -> String {
    BY_LEGAL_FORM
        .replacen("import re\n", "import re\nimport time\n", 1)
        .replacen(
            "    for found in",
            &format!(
                "    if \"slow\" in document.text:\n        \
                 open(\"slow-started\", \"w\").close()\n        \
                 time.sleep({slow_seconds})\n    \
                 time.sleep({seconds})\n    for found in"
            ),
            1,
        )
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

/// A folder is read in batches, each kept as it ends, and the command says
/// how far it is. The files under the folder are its documents, by their
/// paths from it, and what starts with a dot is left out.
#[test]
#[ignore = "runs uv"]
fn a_folder_is_read_in_batches_that_say_how_far_they_are() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_with_stand_in(dir.path());
    let documents = project.join("documents");
    notes(&documents.join("dev"), 6, None);
    notes(&documents.join("train"), 6, None);
    write(&documents.join(".DS_Store"), "\0");
    write(&documents.join("train/.hidden/note-99.txt"), "Kestrel Ltd.");
    let server = Server::dev(&project);

    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let printed = text(&output.stdout);
    assert!(
        printed.starts_with(
            "\
source     notes
read       12 documents from documents
loaded     12 new, 0 already held, 0 new versions
extractor  companies
found      12 records in 12 documents: 12 new, 0 already held

  company  Acorn Tools Ltd  notes:dev/note-01.txt["
        ),
        "{printed}"
    );
    // The first batch is five documents, and the next is made larger.
    let said = text(&output.stderr);
    let lines: Vec<&str> = said
        .lines()
        .filter(|line| !line.starts_with("next: "))
        .collect();
    assert_eq!(lines.len(), 1, "{said}");
    assert!(
        lines[0].starts_with("   Extracted 12 documents in ")
            && lines[0].ends_with(": 12 records, 0 failed"),
        "{said}"
    );
    let batches = runs(&server, "extract");
    assert!(batches.len() >= 2, "{batches:?}");
    assert!(batches.len() < 12, "{batches:?}");
    let keys: Vec<String> = server.read("/v1/documents?source=notes&limit=100")["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|document| document["key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(keys.len(), 12, "{keys:?}");
    assert!(keys.contains(&"train/note-06.txt".to_owned()), "{keys:?}");

    // The same command again has nothing left to read.
    let again = extracted(&server, &project, &["documents", "--source", "notes"]);
    assert!(
        again.contains(
            "loaded     0 new, 12 already held, 0 new versions\nextractor  companies\n\
             found      0 records: `companies` has read every document."
        ),
        "{again}"
    );

    // `--again` reads each document once more, in batches too.
    let again = extracted(&server, &project, &["--again", "--source", "notes"]);
    assert!(
        again.contains("found      12 records in 12 documents: 0 new, 12 already held"),
        "{again}"
    );
    let read: Vec<usize> = server.read("/v1/documents?source=notes&limit=100")["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|document| document["extractions"].as_array().unwrap().len())
        .collect();
    assert_eq!(read, vec![2; 12]);
}

/// A batch that fails, here one stopped for taking longer than Spool lets
/// user code run, keeps nothing, and the batches before it are kept. The
/// message says what the limit is, what raises it, and that the same
/// command reads the rest.
#[test]
#[ignore = "runs uv"]
fn a_batch_that_is_stopped_keeps_the_ones_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    write(
        &project.join("extractors/companies.py"),
        &slow_stand_in(0.05, 60),
    );
    let documents = project.join("documents");
    notes(&documents, 12, Some(9));
    let server = Server::start(&[
        "dev",
        path(&project),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
        "--task-timeout",
        "20",
    ]);

    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    let said = text(&output.stderr);
    let lines: Vec<&str> = said.lines().collect();
    let kept: usize = lines[lines.len() - 2]
        .strip_prefix("        Kept ")
        .and_then(|line| line.split_once(" documents read before run "))
        .and_then(|(kept, rest)| {
            rest.ends_with(": the same command again reads the rest")
                .then(|| kept.parse().ok())?
        })
        .unwrap_or_else(|| panic!("{said}"));
    assert!((5..9).contains(&kept), "{said}");
    let failed = lines.last().unwrap();
    assert!(
        failed.contains(
            "failed: extractor `companies`: it was stopped after 20 seconds without finishing. \
             That is the longest a strategy or an extractor may run here, and \
             `--task-timeout <seconds>` sets it, on `spool dev`, `spool serve` or `spool bench`"
        ),
        "{said}"
    );
    assert_eq!(found(&server).len(), kept);

    // Without the slow note's wait, the same command reads what is left.
    drop(server);
    write(
        &project.join("extractors/companies.py"),
        &slow_stand_in(0.05, 0),
    );
    let server = Server::dev(&project);
    let read = extracted(&server, &project, &["documents", "--source", "notes"]);
    assert!(
        read.contains(&format!(
            "found      {} records in {} documents: {} new, 0 already held",
            12 - kept,
            12 - kept,
            12 - kept
        )),
        "{read}"
    );
}

/// A batch whose extractor fails at once is refused within the wait, and
/// the command still says what the batches before it kept, and prints them
/// with `--json`.
#[test]
#[ignore = "runs uv"]
fn a_batch_that_fails_at_once_keeps_the_ones_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    // It fails for every document of a batch that holds the slow note:
    // what fails a run, and not one document.
    write(
        &project.join("extractors/companies.py"),
        r#"import re

from spool import extractor

NAME = re.compile(r"(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\b")


@extractor
def find(documents):
    if any("slow" in document.text for document in documents):
        raise RuntimeError("cannot read this one")
    for document in documents:
        for found in NAME.finditer(document.text):
            yield document.mention("company", found.start(), found.end(), name=found.group())
"#,
    );
    notes(&project.join("documents"), 8, Some(7));
    let server = Server::dev(&project);

    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    let said = text(&output.stderr);
    assert!(
        said.contains(
            "        Kept 5 documents read before the batch that failed: the same command again \
             reads the rest\n"
        ),
        "{said}"
    );
    assert!(said.contains("cannot read this one"), "{said}");
    assert_eq!(found(&server).len(), 5);

    // As JSON, from the start in a project of its own: the run that stored
    // the documents, and the batch that was kept.
    drop(server);
    let dir = tempfile::tempdir().unwrap();
    let other = init_companies(dir.path());
    std::fs::copy(
        project.join("extractors/companies.py"),
        other.join("extractors/companies.py"),
    )
    .unwrap();
    notes(&other.join("documents"), 8, Some(7));
    let server = Server::dev(&other);
    let output = server
        .spool_in(
            &other,
            &["extract", "documents", "--source", "notes", "--json"],
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let answered: Value = serde_json::from_str(&text(&output.stdout))
        .unwrap_or_else(|_| panic!("{}{}", text(&output.stdout), text(&output.stderr)));
    assert_eq!(
        answered["documents"].as_array().unwrap().len(),
        1,
        "{answered}"
    );
    assert_eq!(
        answered["documents"][0]["load"]["inserted"], 8,
        "{answered}"
    );
    let [kept] = &answered["extractions"].as_array().unwrap()[..] else {
        panic!("one batch kept: {answered}");
    };
    assert_eq!(kept["status"], "succeeded", "{answered}");
    assert_eq!(kept["extraction"]["documents"], 5, "{answered}");
}

/// The stand-in, made to fail with `RuntimeError("cannot read this one")`
/// for a note with `slow` in it, or for every note.
fn failing_stand_in(every: bool) -> String {
    let when = if every {
        "True"
    } else {
        "\"slow\" in document.text"
    };
    BY_LEGAL_FORM.replacen(
        "    for found in",
        &format!(
            "    if {when}:\n        raise RuntimeError(\"cannot read this one\")\n    for found in"
        ),
        1,
    )
}

/// A document the extractor cannot read is failed, with why: the command
/// lists it, `spool show documents` says so, and the same command reads it
/// alone once the extractor can. A batch none of whose documents could be
/// read stops the command.
#[test]
#[ignore = "runs uv"]
fn a_document_that_cannot_be_read_is_failed_and_read_again_alone() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    notes(&project.join("documents"), 8, Some(7));

    // Every document fails: a key that is wrong, say. The first batch
    // says so, and nothing more is tried.
    write(
        &project.join("extractors/companies.py"),
        &failing_stand_in(true),
    );
    let server = Server::dev(&project);
    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let said = text(&output.stderr);
    assert!(
        said.trim_end().ends_with(
            "could not read any of its 5 documents, as notes:note-01.txt says: RuntimeError: \
             cannot read this one. They are not read, and the same command reads them again."
        ),
        "{said}"
    );
    assert_eq!(runs(&server, "extract").len(), 1);

    // One document fails: the others are read, and it is listed.
    drop(server);
    write(
        &project.join("extractors/companies.py"),
        &failing_stand_in(false),
    );
    let server = Server::dev(&project);
    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let printed = text(&output.stdout);
    assert!(
        printed.contains(
            "found      7 records in 7 documents: 7 new, 0 already held\n\
             failed     1 document: they are not read, and the same command reads them again\n  \
             notes:note-07.txt  RuntimeError: cannot read this one\n"
        ),
        "{printed}"
    );
    // What the helper wrote of it reaches the terminal, after its batch.
    let said = text(&output.stderr);
    assert!(
        said.contains("             notes:note-07.txt: RuntimeError: cannot read this one\n"),
        "{said}"
    );
    assert!(
        said.contains("   Extracted 7 documents in ") && said.contains(": 7 records, 1 failed\n"),
        "{said}"
    );
    let listed = shown(
        &server,
        &project,
        &["show", "documents", "--source", "notes"],
    );
    let line = listed
        .lines()
        .find(|line| line.contains("notes:note-07.txt"))
        .unwrap_or_else(|| panic!("{listed}"));
    assert!(
        line.contains("companies failed in run ")
            && line.ends_with(": RuntimeError: cannot read this one"),
        "{listed}"
    );

    // Once it can be read, the same command reads it and nothing else.
    drop(server);
    write(&project.join("extractors/companies.py"), BY_LEGAL_FORM);
    let server = Server::dev(&project);
    let read = extracted(&server, &project, &["documents", "--source", "notes"]);
    assert!(
        read.contains("found      1 record in 1 document: 1 new, 0 already held"),
        "{read}"
    );
}

/// What an extractor writes to standard error reaches whoever runs
/// `spool extract`, however short the run, and the terminal of `spool dev`.
#[test]
#[ignore = "runs uv"]
fn what_an_extractor_says_reaches_both_terminals_on_a_short_run() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    write(
        &project.join("extractors/companies.py"),
        &BY_LEGAL_FORM
            .replacen("import re\n", "import re\nimport sys\n", 1)
            .replacen(
                "    for found in",
                "    print(f\"{document.key}: dropped a quote the text does not have\", file=sys.stderr)\n    for found in",
                1,
            ),
    );
    let server = Server::dev(&project);
    let output = server
        .spool_in(&project, &["extract", "samples/notes-1.txt"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), FIRST);
    assert_eq!(
        text(&output.stderr),
        "             notes-1.txt: dropped a quote the text does not have\nnext: `spool match` \
         places them in identities\n"
    );
    let run = runs(&server, "extract")[0]["run"].as_u64().unwrap();
    assert_eq!(
        server.printed("extractor `companies`"),
        format!("extractor `companies`, run {run}:")
    );
    assert_eq!(
        server.printed("dropped a quote").trim(),
        "notes-1.txt: dropped a quote the text does not have"
    );
}

/// A mention a changed extractor no longer finds is taken back: the
/// command says so, the record says why it has no identity, and a record
/// says which versions of which extractor found it.
#[test]
#[ignore = "runs uv"]
fn a_mention_no_extractor_finds_any_more_is_taken_back() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_with_stand_in(dir.path());
    let server = Server::dev(&project);
    assert_eq!(
        extracted(&server, &project, &["samples/notes-1.txt"]),
        FIRST
    );
    shown(&server, &project, &["match"]);

    // Now it finds what ends in `Ltd` only when it is not Brightwater.
    drop(server);
    write(
        &project.join("extractors/companies.py"),
        &BY_LEGAL_FORM.replacen(
            "        yield document.mention",
            "        if found.group().startswith(\"Brightwater\"):\n            continue\n        yield document.mention",
            1,
        ),
    );
    let server = Server::dev(&project);
    let again = extracted(&server, &project, &["--again"]);
    assert!(
        again.contains(
            "found      2 records in 1 document: 0 new, 2 already held\n\
             taken back 1 record: no extractor finds it any more\n"
        ),
        "{again}"
    );
    let records = found(&server);
    let taken = records
        .iter()
        .find(|record| record["payload"]["name"] == "Brightwater Foods Ltd")
        .unwrap();
    let shown_taken = shown(
        &server,
        &project,
        &["show", "records", taken["id"].as_str().unwrap()],
    );
    assert!(
        shown_taken.contains("identity  none: taken back in run "),
        "{shown_taken}"
    );
    assert!(
        shown_taken.contains(", as no extractor finds it any more\n"),
        "{shown_taken}"
    );
    let kept = records
        .iter()
        .find(|record| record["payload"]["name"] == "Acorn Tools Ltd")
        .unwrap();
    assert_eq!(kept["found_by"].as_array().unwrap().len(), 2, "{kept}");
    assert_ne!(
        kept["found_by"][0]["version"],
        kept["found_by"][1]["version"]
    );
    let shown_kept = shown(
        &server,
        &project,
        &["show", "records", kept["id"].as_str().unwrap()],
    );
    assert!(
        shown_kept.contains("found by  companies version "),
        "{shown_kept}"
    );
}

/// An extractor that finds nothing at all takes back what was found
/// before, as a broken one would, and the command says so out loud.
#[test]
#[ignore = "runs uv"]
fn a_reading_that_finds_nothing_at_all_says_what_it_took_back() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_with_stand_in(dir.path());
    let server = Server::dev(&project);
    assert_eq!(
        extracted(&server, &project, &["samples/notes-1.txt"]),
        FIRST
    );

    drop(server);
    write(
        &project.join("extractors/companies.py"),
        "from spool import extractor\n\n\n@extractor\ndef find(document):\n    return []\n",
    );
    let server = Server::dev(&project);
    let output = server
        .spool_in(&project, &["extract", "--again"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let run = runs(&server, "extract")
        .into_iter()
        .map(|run| run["run"].as_u64().unwrap())
        .max()
        .unwrap();
    assert_eq!(
        text(&output.stderr),
        format!(
            "warning: took back 3 records that run {run} found nowhere, though it found nothing \
             at all: if the extractor is wrong, mend it and run `spool extract --again`, and what \
             it finds again goes back to the identity it left\n"
        )
    );
    assert!(
        text(&output.stdout).contains("taken back 3 records: no extractor finds them any more"),
        "{}",
        text(&output.stdout)
    );
}

/// Ctrl-C cancels the batch that is running: the server stops the
/// extractor and keeps nothing of the batch, the batches before it are
/// kept, and the command says so. While the batch runs, its documents say
/// they are being read.
#[test]
#[ignore = "runs uv"]
fn ctrl_c_cancels_the_batch_that_is_running_and_keeps_the_ones_before() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    write(
        &project.join("extractors/companies.py"),
        &slow_stand_in(0.0, 120),
    );
    let documents = project.join("documents");
    notes(&documents, 10, Some(8));
    let server = Server::dev(&project);

    let mut command = server.spool_in(&project, &["extract", "documents", "--source", "notes"]);
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = project.join("slow-started");
    let deadline = Instant::now() + Duration::from_secs(120);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the slow note was never read");
        assert!(child.try_wait().unwrap().is_none(), "the command ended");
        std::thread::sleep(Duration::from_millis(50));
    }

    // The documents of the batch say which run reads them.
    let running: Vec<Value> = runs(&server, "extract")
        .into_iter()
        .filter(|run| run["status"] == "running")
        .collect();
    let [running] = &running[..] else {
        panic!("one batch runs: {running:?}");
    };
    let run = running["run"].as_u64().unwrap();
    let listed = shown(
        &server,
        &project,
        &["show", "documents", "--source", "notes"],
    );
    let state = |listed: &str, key: &str| {
        listed
            .lines()
            .find(|line| line.contains(&format!("notes:{key}")))
            .unwrap_or_else(|| panic!("{listed}"))
            .split("  ")
            .filter(|cell| !cell.is_empty())
            .last()
            .unwrap()
            .trim()
            .to_owned()
    };
    assert_eq!(
        state(&listed, "note-08.txt"),
        format!("being read by companies in run {run}"),
        "{listed}"
    );
    assert!(
        state(&listed, "note-01.txt").starts_with("companies in run "),
        "{listed}"
    );

    let interrupted = Instant::now();
    let killed = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let output = child.wait_with_output().unwrap();
    assert!(interrupted.elapsed() < Duration::from_secs(20));
    assert_eq!(output.status.code(), Some(130), "{}", text(&output.stderr));
    let said = text(&output.stderr);
    assert!(
        said.contains(&format!("    Stopping run {run}, the batch in progress")),
        "{said}"
    );
    assert!(
        said.trim_end().ends_with(&format!(
            "error: stopped: run {run} was cancelled, and nothing it read is kept. The 5 \
             documents read before it are kept, and the same command again reads the rest."
        )),
        "{said}"
    );

    // The server stopped the extractor, which would have slept two minutes,
    // and the run failed. Its documents are not read.
    let ended = server.read(&format!("/v1/runs/{run}"))["run"].clone();
    assert_eq!(ended["status"], "failed", "{ended}");
    assert!(
        ended["error"]
            .as_str()
            .unwrap()
            .contains("it was cancelled"),
        "{ended}"
    );
    assert_eq!(found(&server).len(), 5);
    let listed = shown(
        &server,
        &project,
        &["show", "documents", "--source", "notes"],
    );
    assert_eq!(state(&listed, "note-08.txt"), "not read yet", "{listed}");

    // The same command reads what is left.
    drop(server);
    write(
        &project.join("extractors/companies.py"),
        &slow_stand_in(0.0, 0),
    );
    let server = Server::dev(&project);
    let read = extracted(&server, &project, &["documents", "--source", "notes"]);
    assert!(
        read.contains("found      5 records in 5 documents: 5 new, 0 already held"),
        "{read}"
    );
}

/// The step as a program takes it: two requests, with no JSON to write.
#[test]
#[ignore = "runs uv"]
#[expect(clippy::too_many_lines)]
fn the_same_step_over_http_sends_the_file_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_with_stand_in(dir.path());
    let server = Server::dev(&project);
    let text_plain = |path: &str, body: &[u8]| {
        server
            .request("POST", path)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(body)
            .send()
    };

    // `curl --data-binary @samples/notes-1.txt -H 'content-type: text/plain'`.
    let stored = text_plain(
        "/v1/documents?source=notes&key=notes-1.txt",
        NOTE.as_bytes(),
    )
    .ok();
    assert_eq!(stored["run"]["status"], "succeeded", "{stored}");
    assert_eq!(stored["run"]["load"]["inserted"], 1, "{stored}");
    // A document sent as text is loaded by its caller, as one sent as JSON.
    let dev = server
        .request("GET", "/auth/session")
        .without("Authorization")
        .send()
        .ok();
    let dev = format!("user:{}", dev["person"]["id"].as_str().unwrap());
    assert_eq!(stored["run"]["started_by"], dev.as_str(), "{stored}");
    let documents = server.read("/v1/documents?source=notes");
    let document = &documents["documents"][0];
    assert_eq!(document["key"], "notes-1.txt");
    assert_eq!(document["length"], NOTE.chars().count());
    let held = server.read(&format!(
        "/v1/documents/{}",
        document["id"].as_str().unwrap()
    ));
    assert_eq!(held["text"], NOTE, "{held}");

    // `curl -X POST .../v1/extractions`: the project has one extractor, so
    // there is nothing to say.
    let read = server.request("POST", "/v1/extractions").send().ok();
    assert_eq!(read["run"]["status"], "succeeded", "{read}");
    assert_eq!(read["run"]["extractor"]["name"], "companies");
    assert_eq!(read["run"]["started_by"], dev.as_str(), "{read}");
    let report = &read["run"]["extraction"];
    assert_eq!(
        (
            &report["documents"],
            &report["mentions"],
            &report["inserted"]
        ),
        (&json!(1), &json!(3), &json!(3))
    );
    let names: Vec<Value> = found(&server)
        .iter()
        .map(|record| record["payload"]["name"].clone())
        .collect();
    assert_eq!(
        names,
        [
            "Acorn Tools Ltd",
            "Larkfield Logistics Ltd",
            "Brightwater Foods Ltd"
        ]
    );

    // It is the document `spool extract` stores: the command finds it held,
    // with everything read.
    let again = shown(&server, &project, &["extract", "samples/notes-1.txt"]);
    assert!(
        again.contains("loaded     0 new, 1 already held, 0 new versions\n"),
        "{again}"
    );
    assert!(again.contains("found      0 records: "), "{again}");

    // A body of JSON still names the extractor or leaves it out, and the
    // text form takes where the source keeps the document.
    for body in [
        json!({}),
        json!({ "extractor": "companies", "again": true }),
    ] {
        let read = server.call("/v1/extractions", body);
        assert_eq!(read["run"]["extractor"]["name"], "companies", "{read}");
    }
    text_plain(
        "/v1/documents?source=notes&key=visit.txt&uri=https://notes.test/visit",
        b"We visited Tidewater Marine LLC.",
    )
    .ok();
    let visit = server.read("/v1/documents?source=notes&key=visit.txt");
    assert_eq!(visit["documents"][0]["uri"], "https://notes.test/visit");

    // The query string says that the body is a document, so a file sent
    // the way `curl` sends one unless told otherwise is still the file.
    let sent = server
        .request("POST", "/v1/documents?source=notes&key=memo.md")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(b"# Memo\n\nHalden Group Inc wrote.\n")
        .send()
        .ok();
    assert_eq!(sent["run"]["load"]["inserted"], 1, "{sent}");
    // And without one the body is JSON, as a browser sends it unless told
    // otherwise.
    let sent = server
        .request("POST", "/v1/documents")
        .header("Content-Type", "text/plain;charset=UTF-8")
        .body(br#"{"source": "notes", "documents": [{"key": "fax.txt", "text": "A fax."}]}"#)
        .send()
        .ok();
    assert_eq!(sent["run"]["load"]["inserted"], 1, "{sent}");
    assert_eq!(
        text_plain("/v1/documents", b"A note.").error(400, "invalid"),
        "the body is not the JSON expected: expected value at line 1 column 1. To send one \
         document as text, name it in the query string, as in \
         `/v1/documents?source=notes&key=call.txt`"
    );

    // What is wrong with a document sent as text is said in its terms.
    let unnamed = text_plain("/v1/documents?source=notes", b"text").error(400, "invalid");
    assert!(
        unnamed.starts_with(
            "a document sent as text is named in the query string, as in \
             `/v1/documents?source=notes&key=call.txt`: "
        ),
        "{unnamed}"
    );
    assert!(unnamed.contains("key"), "{unnamed}");
    assert_eq!(
        text_plain(
            "/v1/documents?source=notes&key=scan.pdf",
            b"%PDF-1.7\n\xff\xfe"
        )
        .error(400, "invalid"),
        "the body is not UTF-8 text: a document is stored as text, so turn a PDF or a web page \
         into text first"
    );
    assert_eq!(
        server.read("/v1/documents?source=notes")["documents"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    // An extractor has to be named once there are two, and there is none
    // to name once there is none.
    write(
        &project.join("extractors/shouting.py"),
        &BY_LEGAL_FORM.replace(
            r"(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\b",
            "[A-Z]{4,}",
        ),
    );
    server.printed("applied");
    assert_eq!(
        server
            .request("POST", "/v1/extractions")
            .send()
            .error(400, "invalid"),
        "the project has the extractors `companies` and `shouting`: say which in `extractor`"
    );
    let read = server.call("/v1/extractions", json!({ "extractor": "shouting" }));
    assert_eq!(read["run"]["extractor"]["name"], "shouting", "{read}");
    for extractor in ["companies", "shouting"] {
        std::fs::remove_file(project.join(format!("extractors/{extractor}.py"))).unwrap();
        server.printed("applied");
    }
    assert_eq!(
        server
            .request("POST", "/v1/extractions")
            .send()
            .error(400, "invalid"),
        "the project has no extractors: one is a file under extractors/, and \
         `spool init <dir> --example companies` makes a project with a sample"
    );
}

/// What a reader is told when the step cannot be taken, each as it is said.
/// The machine has neither `uv` nor `bun`, as a reader's may not.
#[test]
#[expect(clippy::too_many_lines)]
fn extract_says_what_is_in_the_way() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());

    // No server is running: said as every command says it, with what to do.
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = free.local_addr().unwrap();
    drop(free);
    let output = spool()
        .current_dir(&project)
        .args(["extract", "samples/notes-1.txt"])
        .args(["--url", &format!("http://{address}")])
        .output()
        .unwrap();
    let nobody = complaint(&output, &["samples/notes-1.txt"]);
    assert!(
        nobody.starts_with(&format!(
            "the server at http://{address} cannot be reached: "
        )),
        "{nobody}"
    );
    assert!(
        nobody.ends_with(
            "\nhint: start one with `spool dev`, or name another with `--url` or SPOOL_URL"
        ),
        "{nobody}"
    );

    let server = dev_with_nothing_installed(&project, dir.path());
    let refused = |arguments: &[&str]| refused(&server, &project, arguments);

    // The file: what is wrong with it is said before anything is sent.
    let missing = refused(&["samples/notes-9.txt"]);
    assert!(
        missing.starts_with("samples/notes-9.txt: No such file or directory"),
        "{missing}"
    );
    std::fs::write(project.join("report.pdf"), b"%PDF-1.7\n\xff\xfe\xfd").unwrap();
    assert_eq!(
        refused(&["report.pdf", "--source", "notes"]),
        "report.pdf is not text: a document is stored as UTF-8 text. Turn a PDF or a web page \
         into text first."
    );
    write(&project.join("notes-empty.txt"), "\n\n");
    assert_eq!(refused(&["notes-empty.txt"]), "notes-empty.txt is empty");
    write(&project.join("other/notes-1.txt"), "Acorn Tools Ltd.\n");
    assert_eq!(
        refused(&["samples/notes-1.txt", "other/notes-1.txt"]),
        "samples/notes-1.txt and other/notes-1.txt would both be stored as `notes-1.txt`: a \
         file is stored under its name, or under its path from the folder that was named, and \
         a second document under one key is taken for a new version of the first. Name the \
         folder that holds both, or extract them into sources of their own."
    );
    // A folder: what starts with a dot is left out, and every other file
    // has to be text.
    write(&project.join("empty/.DS_Store"), "\0\0");
    write(
        &project.join("empty/.cache/notes-1.txt"),
        "Acorn Tools Ltd.\n",
    );
    assert_eq!(
        refused(&["empty"]),
        "empty is a folder with no files in it, other than ones whose names start with a dot"
    );
    std::fs::create_dir_all(project.join("scans/2024")).unwrap();
    std::fs::write(project.join("scans/2024/report.pdf"), b"%PDF-1.7\n\xff").unwrap();
    assert_eq!(
        refused(&["scans", "--source", "notes"]),
        "scans/2024/report.pdf is not text: a document is stored as UTF-8 text. Turn a PDF or \
         a web page into text first."
    );

    // The source: a file of the reader's own is named after neither.
    write(&project.join("call.txt"), "We met Acorn Tools Ltd.\n");
    assert_eq!(
        refused(&["call.txt"]),
        "the project has the sources `crm` and `notes`, and call.txt is named after neither: \
         say which its documents belong to with `--source`"
    );
    assert_eq!(
        refused(&["call.txt", "--source", "calls"]),
        "the project has no source `calls`: there is no sources/calls.yaml. It has `crm` and \
         `notes`."
    );
    assert_eq!(
        refused(&["call.txt", "--extractor", "people"]),
        "the project has no extractor `people`: it has `companies`"
    );
    // A source no entity type reads cannot hold what an extractor finds,
    // which is said before the document is stored.
    write(&project.join("sources/wire.yaml"), "columns: [name]\n");
    server.printed("applied");
    assert_eq!(
        refused(&["call.txt", "--source", "wire"]),
        "no entity type reads source `wire`, so nothing found in its documents can be a \
         record. Add the source to the `mapping` in an entities/<type>/entity.yaml."
    );
    // None of that stored a document or started a run.
    assert_eq!(server.read("/v1/documents")["documents"], json!([]));
    assert_eq!(runs(&server, "extract"), Vec::<Value>::new());

    // The step itself, on a machine without `uv`: the note is stored, and
    // the reader is told what to install, in one line to paste, before a
    // run is started and with no task left behind.
    let without_uv = "extractor `companies`: it is a Python file, and Spool runs those with \
         `uv`, which is not installed where Spool runs. Install it there and run this again:\n\n  \
         curl -LsSf https://astral.sh/uv/install.sh | sh\n\n\
         Spool finds it where that puts it, with no restart. Other ways to install it: \
         https://docs.astral.sh/uv/getting-started/installation/. If it is installed where the \
         search path does not reach, set SPOOL_UV to it where Spool is started";
    let output = server
        .spool_in(&project, &["extract", "samples/notes-1.txt"])
        .output()
        .unwrap();
    assert_eq!(
        text(&output.stdout),
        "\
source     notes
read       1 document from samples/notes-1.txt
loaded     1 new, 0 already held, 0 new versions
"
    );
    assert_eq!(complaint(&output, &["samples/notes-1.txt"]), without_uv);
    // The rows need nothing installed.
    shown(&server, &project, &["load", "samples/crm-1.csv"]);
    shown(&server, &project, &["match"]);

    // A second extractor: the command asks which. Nothing here runs a
    // TypeScript file either.
    write(
        &project.join("extractors/words.ts"),
        "import { extractor } from \"spool-strategy\";\n\nawait extractor(async () => []);\n",
    );
    server.printed("applied");
    assert_eq!(
        refused(&["samples/notes-1.txt"]),
        "the project has the extractors `companies` and `words`: say which with `--extractor`"
    );
    assert_eq!(refused(&["--extractor", "companies"]), without_uv);
    assert_eq!(
        refused(&["--extractor", "words"]),
        "extractor `words`: it is a TypeScript file, and Spool runs those with `bun`, which is \
         not installed where Spool runs. Install it there and run this again:\n\n  \
         curl -fsSL https://bun.sh/install | bash\n\n\
         Spool finds it where that puts it, with no restart. Other ways to install it: \
         https://bun.sh/docs/installation. If it is installed where the search path does not \
         reach, set SPOOL_BUN to it where Spool is started"
    );
    assert_eq!(runs(&server, "extract"), Vec::<Value>::new());
    assert_eq!(kept_tasks(&project), Vec::<PathBuf>::new());

    // The reader installs `uv`, which goes under the home directory, where
    // the search path of a server that is already running does not reach.
    // The next run starts it all the same: here a stand-in, which says so.
    let installed = dir.path().join("empty/.local/bin/uv");
    write(
        &installed,
        "#!/bin/sh\necho 'a stand-in for uv was started' >&2\nexit 3\n",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let started = refused(&["--extractor", "companies"]);
    assert!(
        started.starts_with(
            "extractor `companies`: it failed with exit status: 3:\n\
             a stand-in for uv was started\n"
        ),
        "{started}"
    );
}

/// The Dockerfile `spool build` writes for a new project puts what the
/// extractor fetches in the image, its model with it, and has a run fetch
/// nothing.
#[test]
fn the_image_of_a_new_project_holds_what_its_extractor_fetches() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    succeed(&["build", path(&project)]);
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(project.join(".spool/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["extractors"][0]["runtime"], "python");
    assert_eq!(manifest["extractors"][0]["file"], "extractors/companies.py");
    let dockerfile = std::fs::read_to_string(project.join("Dockerfile")).unwrap();
    let lines: Vec<&str> = dockerfile.lines().collect();
    let at = |line: &str| {
        lines
            .iter()
            .position(|found| *found == line)
            .unwrap_or_else(|| panic!("no `{line}` in:\n{dockerfile}"))
    };
    // What `uv` fetches goes under /spool/cache, and so does what a model's
    // library fetches, which would otherwise go to the home directory: the
    // volume at /data, which a mount hides.
    let caches =
        at("ENV UV_CACHE_DIR=/spool/cache/uv UV_PYTHON_INSTALL_DIR=/spool/cache/python \\");
    assert_eq!(lines[caches + 1], "    XDG_CACHE_HOME=/spool/cache");
    // The file is started once while the image is built, which is when it
    // loads its model, and only then is everything told to stay offline.
    let install = at("RUN spool build --install");
    let offline = at("ENV UV_OFFLINE=1 HF_HUB_OFFLINE=1");
    assert!(caches < install && install < offline, "{dockerfile}");
    assert!(
        at("COPY --from=ghcr.io/astral-sh/uv:0.13.8 /uv /usr/local/bin/uv") < install,
        "{dockerfile}"
    );
    assert!(!dockerfile.contains("bun"), "{dockerfile}");
}

/// What the image is built to do, without the image: `spool build
/// --install` with the caches the Dockerfile names, on a machine that has
/// fetched nothing, and then a server that may fetch nothing.
#[test]
#[ignore = "fetches the sample extractor's model, and only with SPOOL_TEST_MODEL=1"]
fn build_install_fetches_the_sample_model_and_a_run_then_needs_no_network() {
    if !model_asked_for() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let cache = dir.path().join("cache");
    // A home directory with nothing in it: nothing that was fetched before
    // this test is read, and nothing may be written there.
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let caches = [
        ("HOME", path(&home).to_owned()),
        ("UV_CACHE_DIR", path(&cache.join("uv")).to_owned()),
        (
            "UV_PYTHON_INSTALL_DIR",
            path(&cache.join("python")).to_owned(),
        ),
        ("XDG_CACHE_HOME", path(&cache).to_owned()),
    ];
    // `uv` itself is the one this machine has, which the image copies in.
    let uv = which("uv");

    let installed = spool()
        .args(["build", path(&project), "--install"])
        .envs(caches.iter().map(|(name, value)| (*name, value.as_str())))
        .env("SPOOL_UV", &uv)
        .output()
        .unwrap();
    assert!(
        installed.status.success(),
        "{}{}",
        text(&installed.stdout),
        text(&installed.stderr)
    );
    assert!(
        text(&installed.stdout).ends_with("starts    extractors/companies.py\n"),
        "{}",
        text(&installed.stdout)
    );
    // The weights are where the Dockerfile sends them, in one file of the
    // eight the model is published as, and nothing went to the home
    // directory.
    let snapshots =
        cache.join("huggingface/hub/models--onnx-community--gliner_small-v2.1/snapshots");
    let weights = snapshots
        .join("8142fb00740ccea973e64b1272949ff48653df5e")
        .join("onnx");
    let held: Vec<String> = std::fs::read_dir(&weights)
        .unwrap_or_else(|error| panic!("{}: {error}", weights.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(held, ["model_q4f16.onnx"]);
    let megabytes = std::fs::metadata(weights.join("model_q4f16.onnx"))
        .unwrap()
        .len()
        / 1_000_000;
    assert!((200..300).contains(&megabytes), "{megabytes} MB");
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);

    // The server of the image: it has the same caches and is told to stay
    // offline, and its home directory is the store's volume, where nothing
    // was fetched to.
    let mut environment: Vec<(&str, &str)> = caches
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    environment.extend([
        ("UV_OFFLINE", "1"),
        ("HF_HUB_OFFLINE", "1"),
        ("SPOOL_UV", uv.as_str()),
        // And for good measure nothing it asks for can be reached.
        ("HTTPS_PROXY", "http://127.0.0.1:9"),
        ("HTTP_PROXY", "http://127.0.0.1:9"),
        ("ALL_PROXY", "http://127.0.0.1:9"),
    ]);
    let server = Server::start_in(
        &[
            "serve",
            "--manifest",
            path(&project.join(".spool/manifest.json")),
            "--project",
            path(&project),
            "--store",
            path(&dir.path().join("store.duckdb")),
            "--listen",
            "127.0.0.1:0",
        ],
        &environment,
    )
    .unwrap_or_else(|stopped| panic!("the server did not start:\n{}", stopped.output));
    assert_eq!(
        extracted(&server, &project, &["samples/notes-1.txt"]),
        FIRST
    );
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

/// Another way to find what a note names: the example extractor that asks
/// a language model over HTTP, copied into the new project as its comment
/// says. The model here is a stand-in.
#[test]
#[ignore = "runs bun"]
fn the_example_that_asks_a_model_reads_the_note_of_a_new_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extractors/model.ts");
    std::fs::copy(example, project.join("extractors/model.ts")).unwrap();

    // The model names a company the note gives no legal form, and one the
    // note does not name at all.
    let note = NOTE.replace("with Dana Reyes", "with Dana Reyes of Halden Group");
    write(&project.join("samples/notes-1.txt"), &note);
    let said = |answer: Value| Value::String(answer.to_string());
    let (port, requests) = model(vec![
        said(json!({
            "quotes": ["Acorn Tools Ltd", "Halden Group", "Acorn", "Zennor Glass Ltd"]
        })),
        // What a model says in place of quotes: other JSON, a quote that is
        // no text, words, and nothing.
        said(json!({ "companies": ["Tidewater Marine LLC"] })),
        said(json!({ "quotes": ["Tidewater Marine LLC", 7] })),
        json!("I found one company: Tidewater Marine LLC."),
        Value::Null,
        said(json!({ "quotes": [] })),
    ]);
    let server = Server::start_in(
        &[
            "dev",
            path(&project),
            "--listen",
            "127.0.0.1:0",
            "--seed",
            "1",
        ],
        &[
            ("OPENAI_BASE_URL", &format!("http://127.0.0.1:{port}/v1")),
            ("OPENAI_API_KEY", "a-key"),
            ("SPOOL_EXTRACT_MODEL", "a-model"),
        ],
    )
    .unwrap_or_else(|stopped| panic!("the server did not start:\n{}", stopped.output));

    let output = server
        .spool_in(
            &project,
            &["extract", "samples/notes-1.txt", "--extractor", "model"],
        )
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "\
source     notes
read       1 document from samples/notes-1.txt
loaded     1 new, 0 already held, 0 new versions
extractor  model
found      2 records in 1 document: 2 new, 0 already held

  company  Halden Group     notes:notes-1.txt[24-36]
  company  Acorn Tools Ltd  notes:notes-1.txt[70-85]
"
    );

    // The model was asked once, about the note, and told to quote.
    let asked = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(requests.try_recv().is_err());
    assert_eq!(asked.path, "/v1/chat/completions");
    assert_eq!(asked.authorization, "Bearer a-key");
    assert_eq!(asked.body["model"], "a-model");
    let messages = asked.body["messages"].as_array().unwrap();
    let system = messages[0]["content"].as_str().unwrap();
    assert!(
        system.contains("every company a document names"),
        "{system}"
    );
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], note);

    // An answer that is not quotes fails the run and says what it was, and
    // the document is not taken as read with nothing in it: the next run
    // asks about it again.
    write(
        &project.join("visit.txt"),
        "We visited Tidewater Marine LLC.\n",
    );
    let visit = [
        "extract",
        "visit.txt",
        "--source",
        "notes",
        "--extractor",
        "model",
    ];
    for answer in [
        r#"visit.txt: the model's answer is not {"quotes": ["..."]}: {"companies":["Tidewater Marine LLC"]}"#,
        r#"visit.txt: the model's answer is not {"quotes": ["..."]}: {"quotes":["Tidewater Marine LLC",7]}"#,
        r#"visit.txt: the model's answer is not {"quotes": ["..."]}: I found one company: Tidewater Marine LLC."#,
        "visit.txt: the model gave no answer: ",
    ] {
        let output = server.spool_in(&project, &visit).output().unwrap();
        let said = text(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{said}");
        assert!(said.starts_with("error: extractor `model`: "), "{said}");
        assert!(said.contains(answer), "{said}");
    }
    let nothing = shown(&server, &project, &visit);
    assert!(
        nothing.contains("found      0 records in 1 document"),
        "{nothing}"
    );
    assert_eq!(requests.try_iter().count(), 5);
}
