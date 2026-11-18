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
//! cargo test -p spool-cli --test first_extraction -- --include-ignored
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
//! SPOOL_TEST_MODEL=1 cargo test -p spool-cli --test first_extraction -- --include-ignored sample_model
//! ```
//!
//! One of the two fetches the model each time it runs, into caches of its
//! own.

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::*;

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

next       `spool match` places them in identities
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

/// What `spool <arguments>` printed in `dir`. It has to succeed and say
/// nothing beside.
fn shown(server: &Server, dir: &Path, arguments: &[&str]) -> String {
    let output = server.spool_in(dir, arguments).output().unwrap();
    assert!(
        output.status.success(),
        "spool {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert_eq!(text(&output.stderr), "", "spool {arguments:?}");
    text(&output.stdout)
}

/// What `spool extract <arguments>` printed, which may have taken long
/// enough for it to say on standard error what is running. It has to
/// succeed.
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
    // All it says there is which file is running, and what that file said.
    assert!(
        said.is_empty() || said.starts_with("running    extractors/"),
        "{said}"
    );
    assert!(
        said.lines()
            .skip(1)
            .all(|line| line.starts_with("           ")),
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
        .strip_prefix("spool: ")
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

    // A project of another entity type looks for companies all the same,
    // since that is what the sample note names, and makes them records of
    // its type.
    let people = dir.path().join("people");
    succeed(&["init", path(&people), "--entity", "person"]);
    let extractor = std::fs::read_to_string(people.join("extractors/person.py")).unwrap();
    assert!(
        extractor.contains("\nLOOK_FOR = {\"company\": \"person\"}\n"),
        "{extractor}"
    );

    // The source of the sample note has its name, so the rows' source
    // takes another.
    let refused = run(&["init", path(&dir.path().join("other")), "--source", "notes"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        text(&refused.stderr).starts_with(
            "spool: `notes` is the source of the sample document: give the source of rows \
             another name\n"
        ),
        "{}",
        text(&refused.stderr)
    );
    assert!(!dir.path().join("other").exists());
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
    assert_eq!(found(&server), records);

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

next       `spool match` places it in identities
"
    );

    // `--json` prints the two runs as the server answered them.
    write(&project.join("notes-2.txt"), "Acorn Tools Ltd rang back.\n");
    let answered: Value =
        serde_json::from_str(&extracted(&server, &project, &["notes-2.txt", "--json"])).unwrap();
    assert_eq!(answered["documents"]["kind"], "load", "{answered}");
    assert_eq!(answered["documents"]["load"]["inserted"], 1);
    assert_eq!(answered["extraction"]["kind"], "extract");
    assert_eq!(answered["extraction"]["status"], "succeeded");
    assert_eq!(answered["extraction"]["extractor"]["name"], "companies");
    assert_eq!(answered["extraction"]["extraction"]["inserted"], 1);
    // A run that is over says nothing more.
    assert_eq!(answered["extraction"]["said"], Value::Null);

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
    let said = text(&output.stderr);
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines[0], "running    extractors/companies.py", "{said}");
    assert_eq!(
        lines.last(),
        Some(&"           Fetching the model (245 MB), once."),
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

/// A run over many documents can take longer than Spool lets user code run.
/// While it runs it says how far it is, and when it is stopped the message
/// says how far it got, what the limit is and what raises it.
#[test]
#[ignore = "runs uv"]
fn a_run_that_is_stopped_says_how_far_it_got_and_what_raises_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    // A second and a half for each document, as a model on a slow machine.
    write(
        &project.join("extractors/companies.py"),
        &BY_LEGAL_FORM
            .replacen("import re\n", "import re\nimport time\n", 1)
            .replacen(
                "    for found in",
                "    time.sleep(1.5)\n    for found in",
                1,
            ),
    );
    let notes: Vec<String> = (1..=20).map(|note| format!("notes-{note}.txt")).collect();
    for (note, name) in notes.iter().enumerate() {
        write(
            &project.join("samples").join(name),
            &format!("Note {note}: Acorn Tools Ltd called."),
        );
    }
    let files: Vec<String> = notes.iter().map(|name| format!("samples/{name}")).collect();
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let limited = |seconds: &str| {
        Server::start(&[
            "dev",
            path(&project),
            "--listen",
            "127.0.0.1:0",
            "--seed",
            "1",
            "--task-timeout",
            seconds,
        ])
    };

    // Twenty documents take thirty seconds, and the run has twenty-five.
    let server = limited("25");
    let output = server
        .spool_in(&project, &[&["extract"], &files[..]].concat())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    let said = text(&output.stderr);
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines[0], "running    extractors/companies.py", "{said}");
    let progress: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("           read "))
        .collect();
    assert!(!progress.is_empty(), "{said}");
    for line in &progress {
        let read: u32 = line
            .strip_suffix(" of 20 documents")
            .unwrap_or_else(|| panic!("{said}"))
            .parse()
            .unwrap();
        assert!((1..20).contains(&read), "{said}");
    }
    let last = progress.last().unwrap();
    let kept = lines
        .last()
        .unwrap()
        .strip_prefix(&format!(
            "spool: run 2 failed: extractor `companies`: it was stopped after 25 seconds without \
             finishing. \
             That is the longest a strategy or an extractor may run here, and \
             `--task-timeout <seconds>` sets it, on `spool dev`, `spool serve` or `spool bench`. \
             The last it said was `read {last}`. Its task is kept in "
        ))
        .unwrap_or_else(|| panic!("{said}"));
    assert!(Path::new(kept).join("stderr.log").is_file(), "{said}");

    // The documents are stored and none of them is read: a run is all or
    // nothing. One that has the time reads every one. Here that is the same
    // extractor without the wait, to keep the test short.
    assert_eq!(found(&server), Vec::<Value>::new());
    drop(server);
    write(&project.join("extractors/companies.py"), BY_LEGAL_FORM);
    let server = limited("25");
    let read = extracted(&server, &project, &[]);
    assert!(
        read.contains("found      20 records in 20 documents: 20 new, 0 already held"),
        "{read}"
    );
}

/// The step as a program takes it: two requests, with no JSON to write.
#[test]
#[ignore = "runs uv"]
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
        "the project has no extractors: one is a file under extractors/, and `spool init` \
         writes a sample"
    );
}

/// What a reader is told when the step cannot be taken, each as it is said.
/// The machine has neither `uv` nor `bun`, as a reader's may not.
#[test]
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
        nobody
            .ends_with("\nStart one with `spool dev`, or name another with `--url` or SPOOL_URL."),
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
        "two of the files are called notes-1.txt, and a document is stored under its file's \
         name: extract them one at a time, from sources of their own"
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

next       `spool match` places them in identities
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
        assert!(said.starts_with("spool: extractor `model`: "), "{said}");
        assert!(said.contains(answer), "{said}");
    }
    let nothing = shown(&server, &project, &visit);
    assert!(
        nothing.contains("found      0 records in 1 document"),
        "{nothing}"
    );
    assert_eq!(requests.try_iter().count(), 5);
}
