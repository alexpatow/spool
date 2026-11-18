//! The first extraction of a new project: `spool init` writes a note and a
//! pattern that finds company names in it, and `spool extract` has the
//! server read the note. The same step over HTTP, and what a reader is told
//! when a step cannot be taken.
//!
//! The server of these tests has neither `uv` nor `bun` to start: a pattern
//! is run by the server itself. The one test that runs a TypeScript
//! extractor starts `bun`, and is ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool-cli --test first_extraction -- --include-ignored
//! ```

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use support::*;

const NOTE: &str = "\
Call with Dana Reyes on 12 November. She has moved to Acorn Tools Ltd and runs
purchasing there. They now ship through Larkfield Logistics Ltd, and she asked
whether we work with Brightwater Foods Ltd, their new supplier.
";

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

/// The steps a reader takes after `spool init`, with what each prints: the
/// text the README and the site show.
#[test]
fn the_note_of_a_new_project_is_read_and_what_it_names_joins_the_rows() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    assert_eq!(
        std::fs::read_to_string(project.join("samples/notes-1.txt")).unwrap(),
        NOTE
    );
    // The extractor is one line of pattern under a comment that says what
    // it is, and the notes are a source whose records say a name.
    let extractor = std::fs::read_to_string(project.join("extractors/companies.yaml")).unwrap();
    let lines: Vec<&str> = extractor
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect();
    assert_eq!(
        lines,
        [
            "entity_type: company",
            "attribute: name",
            r"pattern: '(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\b'",
        ]
    );
    let source = std::fs::read_to_string(project.join("sources/notes.yaml")).unwrap();
    assert!(source.ends_with("\ncolumns: [name]\n"), "{source}");
    let listing = succeed(&["info", path(&project)]);
    assert!(
        listing.contains("extractor `companies` (pattern, extractors/companies.yaml)"),
        "{listing}"
    );
    assert!(listing.contains("sources   crm, notes"), "{listing}");

    let server = dev_with_nothing_installed(&project, dir.path());
    let shown = |arguments: &[&str]| shown(&server, &project, arguments);

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
    assert_eq!(
        shown(&["extract", "samples/notes-1.txt"]),
        "\
source     notes
read       1 document from samples/notes-1.txt
loaded     1 new, 0 already held, 0 new versions
extractor  companies
found      3 records in 1 document: 3 new, 0 already held

  company  Acorn Tools Ltd          notes:notes-1.txt[54-69]
  company  Larkfield Logistics Ltd  notes:notes-1.txt[119-142]
  company  Brightwater Foods Ltd    notes:notes-1.txt[179-200]

next       `spool match` places them in identities
"
    );
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
    // done regardless.
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
        shown(&["extract", "samples/notes-1.txt", "--again"]),
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

    // A note that was edited is the next version of its document: what it
    // names now is new, and what it named at other places is superseded by
    // nothing, since a place is of one version.
    write(
        &project.join("samples/notes-1.txt"),
        &NOTE.replace("Brightwater Foods Ltd", "Brightwater Foods GmbH"),
    );
    let edited = shown(&["extract", "samples/notes-1.txt"]);
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
    assert_eq!(sent["run"]["ingest"]["inserted"], 1, "{sent}");
    assert_eq!(
        shown(&["extract"]),
        "\
extractor  companies
found      1 record in 1 document: 1 new, 0 already held

next       `spool match` places it in identities
"
    );

    // `--json` prints the two runs as the server answered them.
    write(&project.join("notes-2.txt"), "Acorn Tools Ltd rang back.\n");
    let answered: Value =
        serde_json::from_str(&shown(&["extract", "notes-2.txt", "--json"])).unwrap();
    assert_eq!(answered["documents"]["kind"], "ingest", "{answered}");
    assert_eq!(answered["documents"]["ingest"]["inserted"], 1);
    assert_eq!(answered["extraction"]["kind"], "extract");
    assert_eq!(answered["extraction"]["extractor"]["name"], "companies");
    assert_eq!(answered["extraction"]["extraction"]["inserted"], 1);
    assert!(
        runs(&server, "extract")
            .iter()
            .all(|run| run["status"] == "succeeded")
    );
}

/// The step as a program takes it: two requests, with no JSON to write.
#[test]
fn the_same_step_over_http_sends_the_file_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = dev_with_nothing_installed(&project, dir.path());
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
    assert_eq!(stored["run"]["ingest"]["inserted"], 1, "{stored}");
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
    assert_eq!(sent["run"]["ingest"]["inserted"], 1, "{sent}");
    // And without one the body is JSON, as a browser sends it unless told
    // otherwise.
    let sent = server
        .request("POST", "/v1/documents")
        .header("Content-Type", "text/plain;charset=UTF-8")
        .body(br#"{"source": "notes", "documents": [{"key": "fax.txt", "text": "A fax."}]}"#)
        .send()
        .ok();
    assert_eq!(sent["run"]["ingest"]["inserted"], 1, "{sent}");
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
        &project.join("extractors/shouting.yaml"),
        "entity_type: company\nattribute: name\npattern: '[A-Z]{4,}'\n",
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
        std::fs::remove_file(project.join(format!("extractors/{extractor}.yaml"))).unwrap();
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
    // None of that stored a document or started a run.
    assert_eq!(server.read("/v1/documents")["documents"], json!([]));
    assert_eq!(runs(&server, "extract"), Vec::<Value>::new());
    // Nothing held and nothing given is said, and is no failure.
    assert_eq!(
        shown(&server, &project, &["extract"]),
        "\
extractor  companies
found      0 records: the server holds no document. `spool extract <file>` gives it one.
"
    );
    // Told the source, the command reads the file.
    let read = shown(
        &server,
        &project,
        &["extract", "call.txt", "--source", "notes"],
    );
    assert!(
        read.contains("\n  company  Acorn Tools Ltd  notes:call.txt[7-22]\n"),
        "{read}"
    );

    // A source no entity type reads cannot hold what an extractor finds,
    // and a source the pattern's type does not read cannot hold what the
    // pattern finds. Both are said before the document is stored.
    write(&project.join("sources/wire.yaml"), "columns: [name]\n");
    server.printed("applied");
    assert_eq!(
        refused(&["call.txt", "--source", "wire"]),
        "no entity type reads source `wire`, so nothing found in its documents can be a \
         record. Add the source to the `mapping` in an entities/<type>/entity.yaml."
    );
    write(
        &project.join("entities/person/entity.yaml"),
        "features: [name]\nmapping:\n  wire:\n    name: name\n",
    );
    server.printed("applied");
    assert_eq!(
        refused(&["call.txt", "--source", "wire"]),
        "`companies` finds records of type `company`, and the mapping of `company` has nothing \
         for source `wire`: add it to entities/company/entity.yaml"
    );
    assert_eq!(
        server.read("/v1/documents?source=wire")["documents"],
        json!([])
    );

    // A second extractor: the command asks which. These two are user code,
    // and the machine has nothing to run them with, which is said with what
    // to install, before a run is started and with no task left behind.
    write(
        &project.join("extractors/names.py"),
        "from spool import extractor\n\n\n@extractor\ndef find(document):\n    return []\n",
    );
    server.printed("applied");
    write(
        &project.join("extractors/words.ts"),
        "import { extractor } from \"spool-strategy\";\n\nawait extractor(async () => []);\n",
    );
    server.printed("applied");
    assert_eq!(
        refused(&["samples/notes-1.txt"]),
        "the project has the extractors `companies`, `names` and `words`: say which with \
         `--extractor`"
    );
    let before = runs(&server, "extract").len();
    assert_eq!(
        refused(&["--extractor", "names"]),
        "extractor `names`: it is a Python file, and Spool runs those with `uv`, which is not \
         installed where Spool runs. Install uv there \
         (https://docs.astral.sh/uv/getting-started/installation/) and try again. If it is \
         installed where the search path does not reach, set SPOOL_UV to it where Spool is \
         started"
    );
    assert_eq!(
        refused(&["--extractor", "words"]),
        "extractor `words`: it is a TypeScript file, and Spool runs those with `bun`, which is \
         not installed where Spool runs. Install bun there (https://bun.sh/docs/installation) \
         and try again. If it is installed where the search path does not reach, set SPOOL_BUN \
         to it where Spool is started"
    );
    assert_eq!(runs(&server, "extract").len(), before);
    let kept: Vec<PathBuf> = std::fs::read_dir(project.join(".spool"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .flat_map(|dir| std::fs::read_dir(dir).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("run-"))
        })
        .collect();
    assert_eq!(kept, Vec::<PathBuf>::new());
    // The pattern still runs.
    let read = shown(
        &server,
        &project,
        &["extract", "samples/notes-1.txt", "--extractor", "companies"],
    );
    assert!(
        read.contains("found      3 records in 1 document"),
        "{read}"
    );

    // A pattern that cannot find records is a mistake in the project: the
    // server says so and keeps the last version that compiled, and `info`
    // and `build` fail with the file.
    write(
        &project.join("extractors/companies.yaml"),
        "entity_type: company\nattribute: name\npattern: '(?:[A-Z]'\n",
    );
    let said = server.complained("extractors/companies.yaml");
    assert!(
        said.contains("the pattern is not a regular expression Spool can run"),
        "{said}"
    );
    for command in ["info", "build"] {
        let output = run(&[command, path(&project)]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        let stderr = text(&output.stderr);
        assert!(stderr.contains("extractors/companies.yaml"), "{stderr}");
        assert!(
            stderr.contains("the pattern is not a regular expression Spool can run"),
            "{stderr}"
        );
    }
}

/// A project whose only extractor is a pattern needs nothing installed to
/// be deployed either.
#[test]
fn a_new_project_builds_into_an_image_that_installs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    succeed(&["build", path(&project)]);
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(project.join(".spool/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["extractors"][0]["runtime"], "pattern");
    assert_eq!(
        manifest["extractors"][0]["pattern"]["entity_type"],
        "company"
    );
    let dockerfile = std::fs::read_to_string(project.join("Dockerfile")).unwrap();
    for runtime in ["uv", "bun", "--install"] {
        assert!(!dockerfile.contains(runtime), "{runtime} in:\n{dockerfile}");
    }
    // `--install` fetches what user code depends on, and a pattern depends
    // on nothing: it is not handed to `uv` or `bun`.
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let installed = spool()
        .args(["build", path(&project), "--install"])
        .env("PATH", &empty)
        .env("HOME", &empty)
        .output()
        .unwrap();
    assert!(installed.status.success(), "{}", text(&installed.stderr));

    // A server that has the manifest and not the project runs the pattern.
    let server = Server::start_in(
        &[
            "serve",
            "--manifest",
            path(&project.join(".spool/manifest.json")),
            "--store",
            path(&dir.path().join("store.duckdb")),
            "--listen",
            "127.0.0.1:0",
        ],
        &[("PATH", path(&empty)), ("HOME", path(&empty))],
    )
    .unwrap_or_else(|stopped| panic!("the server did not start:\n{}", stopped.output));
    let read = shown(&server, &project, &["extract", "samples/notes-1.txt"]);
    assert!(
        read.contains("found      3 records in 1 document"),
        "{read}"
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

/// The step after the pattern: the example extractor that asks a language
/// model, copied into the new project as its comment says.
#[test]
#[ignore = "runs bun"]
fn the_example_that_asks_a_model_reads_the_note_of_a_new_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extractors/model.ts");
    std::fs::copy(example, project.join("extractors/model.ts")).unwrap();

    // The model names a company the pattern cannot find, since the note
    // gives it no legal form, and one the note does not name at all.
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

    // The pattern finds the place the model found, and it is one record.
    let both = shown(&server, &project, &["extract", "--extractor", "companies"]);
    assert!(
        both.contains("found      3 records in 1 document: 2 new, 1 already held"),
        "{both}"
    );

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
