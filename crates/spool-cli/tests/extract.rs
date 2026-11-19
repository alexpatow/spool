//! Documents and a project's extractors through the commands and the server.
//!
//! The tests that start `uv` are ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool-cli --test extract -- --include-ignored
//! ```

mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::*;

/// Takes a capitalised word to be a thing, with the word in lower case as
/// its code.
const WORDS: &str = r#"
import re

from spool import extractor


@extractor
def find(document):
    for found in re.finditer(r"[A-Z][a-z]+", document.text):
        yield document.mention("thing", found.start(), found.end(), code=found.group().lower())
"#;

const BROKEN: &str = r#"
from spool import extractor


@extractor
def find(documents):
    raise RuntimeError("the model is missing")
"#;

fn write_extractor(project: &Path, file: &str, source: &str) {
    let path = project.join("extractors").join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, source).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// `spool serve` on a new store in `dir`, with the project's files where
/// the server can run them.
fn serve(project: &Path, dir: &Path) -> Server {
    let manifest = build(project);
    Server::start(&[
        "serve",
        "--manifest",
        path(&manifest),
        "--project",
        path(project),
        "--tasks",
        path(&dir.join("tasks")),
        "--store",
        path(&dir.join("store.duckdb")),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ])
}

fn add(server: &Server, documents: Value) -> Value {
    let run = server.call(
        "/v1/documents",
        json!({ "source": "notes", "documents": documents }),
    )["run"]
        .clone();
    assert_eq!(run["status"], "succeeded", "{run}");
    run
}

#[test]
fn info_lists_extractors_and_build_writes_a_dockerfile_that_can_run_them() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_things(project);
    write_extractor(project, "words.py", WORDS);
    std::fs::write(
        project.join("entities/thing/extract.md"),
        "A thing is a capitalised word.\n",
    )
    .unwrap();

    let listing = succeed(&["info", path(project)]);
    assert!(
        listing.contains("extractor `words` (python, extractors/words.py)"),
        "{listing}"
    );
    assert!(listing.contains("  extract   extract.md"), "{listing}");
    let listing: Value =
        serde_json::from_str(&succeed(&["info", path(project), "--json"])).unwrap();
    assert_eq!(listing["extractors"].as_array().unwrap().len(), 1);
    assert_eq!(listing["extractors"][0]["name"], "words");
    assert_eq!(listing["extractors"][0]["runtime"], "python");
    assert_eq!(
        listing["entity_types"][0]["extract"],
        "A thing is a capitalised word."
    );

    // An extractor is user code like a strategy: the image holds the
    // project's files and what runs them.
    succeed(&["build", path(project)]);
    let manifest: Value =
        serde_json::from_str(&read(&project.join(".spool/manifest.json"))).unwrap();
    assert_eq!(
        manifest["extractors"][0]["hash"],
        listing["extractors"][0]["hash"]
    );
    let dockerfile = read(&project.join("Dockerfile"));
    for line in [
        "COPY --from=ghcr.io/astral-sh/uv:",
        "COPY --from=build --chown=spool:spool /project /spool/project",
        "RUN spool build --install",
    ] {
        assert!(dockerfile.contains(line), "no `{line}` in:\n{dockerfile}");
    }
    assert!(!dockerfile.contains("bun"), "{dockerfile}");

    // Two extractors cannot share a name, whatever they are written in.
    write_extractor(project, "words.ts", "export {};\n");
    let refused = run(&["info", path(project)]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        text(&refused.stderr).contains("words"),
        "{}",
        text(&refused.stderr)
    );
}

#[test]
fn documents_are_stored_listed_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = serve(&project, dir.path());

    let run = add(
        &server,
        json!([
            { "key": "a", "text": "🔨 Anvil and Hammer.", "uri": "https://notes.test/a" },
            { "key": "b", "text": "Hammer again." },
            { "key": "a", "text": "🔨 Anvil and Hammer.", "uri": "https://notes.test/a" },
        ]),
    );
    assert_eq!(run["kind"], "load");
    assert_eq!(run["entity_type"], Value::Null);
    let report = &run["load"];
    assert_eq!(
        (
            &report["received"],
            &report["inserted"],
            &report["duplicates"]
        ),
        (&json!(3), &json!(2), &json!(1))
    );
    assert_eq!(server.read("/v1/info")["store"]["documents"], 2);

    // A list leaves the text out and says how long it is, in code points.
    let list = server.read("/v1/documents");
    let documents = list["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 2);
    assert_eq!(list["next"], Value::Null);
    let a = documents
        .iter()
        .find(|document| document["key"] == "a")
        .unwrap();
    assert_eq!(a["source"], "notes");
    assert_eq!(a["uri"], "https://notes.test/a");
    assert_eq!(a["length"], 19);
    assert_eq!(a["text"], Value::Null);
    assert_eq!(a["load_run"], run["run"]);
    assert_eq!(a["extractions"], json!([]));

    // One document is read with its text.
    let id = a["id"].as_str().unwrap();
    let read = server.read(&format!("/v1/documents/{id}"));
    assert_eq!(read["text"], "🔨 Anvil and Hammer.");
    assert_eq!(read["id"], a["id"]);

    // A changed text under the same key is the document's next version.
    let run = add(&server, json!([{ "key": "a", "text": "Only an Anvil." }]));
    assert_eq!(run["load"]["superseded"], 1);
    let versions = server.read("/v1/documents?key=a&source=notes");
    assert_eq!(versions["documents"].as_array().unwrap().len(), 2);
    let latest = server.read("/v1/documents?key=a&latest=true");
    let latest = latest["documents"].as_array().unwrap();
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0]["supersedes"], a["id"]);
    assert_eq!(
        server.read(&format!("/v1/documents/{id}"))["superseded_by"],
        latest[0]["id"]
    );

    // Lists come in pages.
    let first = server.read("/v1/documents?limit=2");
    assert_eq!(first["documents"].as_array().unwrap().len(), 2);
    let next = first["next"].as_str().unwrap();
    let second = server.read(&format!("/v1/documents?limit=2&after={next}"));
    assert_eq!(second["documents"].as_array().unwrap().len(), 1);
    assert_eq!(second["next"], Value::Null);

    // What the server will not take is described.
    let missing = server
        .get("/v1/documents/00000000-0000-0000-0000-000000000000")
        .error(404, "not_found");
    assert!(missing.contains("document"), "{missing}");
    server.get("/v1/documents/nine").error(400, "invalid");
    server
        .get("/v1/documents?newest=true")
        .error(400, "invalid");
    let refused = server
        .post(
            "/v1/documents",
            json!({ "source": "notes", "documents": [{ "key": "", "text": "x" }] }),
        )
        .error(400, "invalid");
    assert!(refused.contains("document 1: it has no key"), "{refused}");
    server
        .post("/v1/documents", json!({ "source": "notes" }))
        .error(400, "invalid");

    // A project without extractors has none to run, and says so.
    assert_eq!(server.read("/v1/info")["store"]["extractors"], json!([]));
    let unknown = server
        .post("/v1/extractions", json!({ "extractor": "words" }))
        .error(400, "invalid");
    assert!(
        unknown.contains("the project has no extractor `words`"),
        "{unknown}"
    );
    server.stop();
}

#[test]
#[ignore = "runs uv"]
fn the_server_runs_the_projects_extractors_and_resolves_what_they_find() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    write_extractor(&project, "words.py", WORDS);
    write_extractor(&project, "broken.py", BROKEN);
    let server = serve(&project, dir.path());

    let info = server.read("/v1/info")["store"].clone();
    let extractors = info["extractors"].as_array().unwrap();
    let words = extractors
        .iter()
        .find(|extractor| extractor["name"] == "words")
        .unwrap();
    assert_eq!(words["runtime"], "python");
    assert_eq!(words["file"], "extractors/words.py");

    add(
        &server,
        json!([
            { "key": "a", "text": "🔨 Anvil and Hammer." },
            { "key": "b", "text": "Hammer again." },
        ]),
    );

    // The call is an extract run, and what it found is records.
    let run = server.call("/v1/extractions", json!({ "extractor": "words" }))["run"].clone();
    assert_eq!(run["kind"], "extract", "{run}");
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(run["extractor"]["name"], "words");
    assert_eq!(run["extractor"]["version"], words["hash"]);
    let report = &run["extraction"];
    assert_eq!(
        (
            &report["documents"],
            &report["mentions"],
            &report["inserted"]
        ),
        (&json!(2), &json!(3), &json!(3))
    );
    assert_eq!(
        server.read("/v1/info")["store"]["entity_types"][0]["unassigned"],
        3
    );

    // The document says who read it, and its mentions are found by it.
    let documents = server.read("/v1/documents")["documents"].clone();
    let a = documents
        .as_array()
        .unwrap()
        .iter()
        .find(|document| document["key"] == "a")
        .unwrap()
        .clone();
    assert_eq!(a["extractions"][0]["extractor"], "words");
    assert_eq!(a["extractions"][0]["run"], run["run"]);
    assert_eq!(a["extractions"][0]["mentions"], 2);
    let id = a["id"].as_str().unwrap();
    let found = server.read(&format!("/v1/records?document={id}"))["records"].clone();
    let found = found.as_array().unwrap();
    assert_eq!(found.len(), 2);
    let anvil = found
        .iter()
        .find(|record| record["payload"]["code"] == "anvil")
        .unwrap();
    assert_eq!(anvil["source"], "notes");
    assert_eq!(anvil["key"], format!("{id}#thing:2-7"));
    assert_eq!(anvil["evidence"]["text"], "Anvil");
    assert_eq!(anvil["evidence"]["key"], "a");
    assert_eq!(anvil["evidence"]["extractor"], "words");
    // The records were stored by a load run of the extractor's.
    let stored = server.read(&format!("/v1/runs/{}", report["load_run"]))["run"].clone();
    assert_ne!(stored["run"], run["run"]);
    assert_eq!(stored["kind"], "load");
    assert_eq!(stored["extractor"]["name"], "words");
    // Whoever asked for the extraction started both, and the load of the
    // documents before them.
    let starters: Vec<Value> = server.read("/v1/runs")["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["started_by"].clone())
        .collect();
    assert_eq!(starters, ["agent:api"; 3]);

    // Matching then treats them as any record: the two hammers are one.
    let resolved = server.call("/v1/match", json!({ "entity_type": "thing" }))["run"].clone();
    assert_eq!(
        resolved["resolution"]["identities_created"], 2,
        "{resolved}"
    );
    assert_eq!(resolved["resolution"]["records_assigned"], 3);

    // Documents the extractor has read are not read again unless asked.
    let again = server.call("/v1/extractions", json!({ "extractor": "words" }))["run"].clone();
    assert_eq!(again["extraction"]["documents"], 0, "{again}");
    let again = server.call(
        "/v1/extractions",
        json!({ "extractor": "words", "again": true, "limit": 1 }),
    )["run"]
        .clone();
    assert_eq!(again["extraction"]["documents"], 1, "{again}");
    assert_eq!(again["extraction"]["inserted"], 0, "{again}");

    // An extractor that fails, and not for one document, is a failed run,
    // and nothing was stored.
    let failed = server
        .post("/v1/extractions", json!({ "extractor": "broken" }))
        .error(500, "extractor_failed");
    assert!(failed.contains("extractor `broken`"), "{failed}");
    assert!(failed.contains("the model is missing"), "{failed}");
    let runs = server.read("/v1/runs?kind=extract&status=failed")["runs"].clone();
    assert_eq!(runs.as_array().unwrap().len(), 1, "{runs}");
    assert_eq!(runs[0]["extractor"]["name"], "broken");
    assert_eq!(
        server.read("/v1/records")["records"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    server.stop();
}

#[test]
#[ignore = "runs uv"]
fn dev_runs_an_extractor_as_it_was_last_saved() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let server = Server::start_with(&[
        "dev",
        path(dir.path()),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
        "--tasks",
        path(&dir.path().join("tasks")),
    ])
    .unwrap();
    assert_eq!(server.read("/v1/info")["store"]["extractors"], json!([]));
    add(
        &server,
        json!([{ "key": "a", "text": "Anvil and Hammer." }]),
    );

    // An extractor saved while the server runs is one it can run.
    write_extractor(dir.path(), "words.py", WORDS);
    server.printed("applied");
    let extractors = server.read("/v1/info")["store"]["extractors"].clone();
    assert_eq!(extractors[0]["name"], "words");
    let run = server.call("/v1/extractions", json!({ "extractor": "words" }))["run"].clone();
    assert_eq!(run["extraction"]["mentions"], 2, "{run}");

    // And the file as it is saved next is the extractor from then on.
    write_extractor(
        dir.path(),
        "words.py",
        &WORDS.replace("[A-Z][a-z]+", "Anvil"),
    );
    server.printed("applied");
    let run = server.call(
        "/v1/extractions",
        json!({ "extractor": "words", "again": true }),
    )["run"]
        .clone();
    assert_ne!(run["extractor"]["version"], extractors[0]["hash"]);
    assert_eq!(run["extraction"]["mentions"], 1, "{run}");
    server.stop();
}

#[test]
#[ignore = "runs uv"]
fn bench_scores_extraction_apart_from_resolution_and_the_two_together() {
    let dir = tempfile::tempdir().unwrap();
    let results = dir.path().join("results.json");
    let printed = succeed(&[
        "bench",
        path(&benchmarks().join("extraction/company-news")),
        "--db",
        path(&dir.path().join("bench.duckdb")),
        "--tasks",
        path(&dir.path().join("tasks")),
        "--out",
        path(&results),
    ]);
    for line in [
        "stored    5 documents from `news`: 5 new, 0 already known",
        "extracted `suffix` read 5 documents and found 10 mentions: 10 new, 0 already known",
        "extraction (25 labelled mentions):",
        "  mentions      21 extracted, 18 of them labelled",
        "  spans         precision 0.8571  recall 0.7200  (F1 0.7826)",
        "against the truth (18 records):",
        "end to end (25 labelled mentions, 21 extracted):",
        "  entities      10 of 10 true entities have a mention that was found",
        "  pairs         precision 0.8333  recall 0.3571  (F1 0.5000)",
        // A mention is named by its document and its place in it.
        "news:02-brightwave-funding.txt[12-26], news:08-kestrel-bericht.txt[48-62]",
    ] {
        assert!(printed.contains(line), "no `{line}` in:\n{printed}");
    }

    let report: Value = serde_json::from_str(&read(&results)).unwrap();
    let last = &report["steps"][1];
    assert_eq!(last["documents"][0]["source"], "news");
    assert_eq!(last["extract"][0]["extractor"], "suffix");
    assert_eq!(last["extraction"]["labelled"], 25);
    assert_eq!(last["extraction"]["correct"], 18);
    assert_eq!(last["metrics"]["false_merges"], 1);
    assert_eq!(last["end_to_end"]["pairs"]["actual"], 28);
}

/// The benchmark on Linked-DocRED is run by hand: the dataset is under the
/// GPL and none of it is here. Its script is run here on five invented
/// documents in the dataset's format, so that the script, the project and
/// its extractors stay in step with `spool bench`.
#[test]
#[ignore = "runs uv"]
fn the_linked_docred_script_converts_and_scores_documents_in_the_datasets_format() {
    let project = benchmarks().join("extraction/linked-docred");
    let dir = tempfile::tempdir().unwrap();
    let uv = std::env::var_os("SPOOL_UV").unwrap_or_else(|| "uv".into());
    let output = std::process::Command::new(uv)
        .args(["run", "--no-project", path(&project.join("run.py"))])
        .args(["--data", path(&project.join("sample"))])
        .args(["--work", path(dir.path())])
        .args(["--spool", env!("CARGO_BIN_EXE_spool")])
        .args(["--types", "organisation,person"])
        // The other extractors are models, which are fetched before they run.
        .args(["--extractors", "gold,suffix"])
        .output()
        .unwrap();
    let printed = text(&output.stdout);
    assert!(
        output.status.success(),
        "run.py failed:\n{printed}{}",
        text(&output.stderr)
    );
    assert!(
        printed.contains("5 documents in 3 batches, 17 labelled mentions of 9 entities"),
        "{printed}"
    );

    // Tokens are joined the way a writer spaces them, and a mention is
    // where its tokens went.
    assert_eq!(
        read(&dir.path().join("documents/train/train-0000.txt")),
        "Northwind Traders Ltd is a kitchenware company based in Leeds, England. \
         It was founded in 1998 by Dana Reyes (born 1961), who had left Halden Freight Corp. \
         Reyes called it \"a mail-order shop\"; Northwind's first warehouse opened in 2001."
    );
    let labels = read(&dir.path().join("labels/organisation.csv"));
    for label in [
        "wikipedia,train-0000.txt,193,202,organisation,Q900000001,Northwind",
        // An entity with a page and no Wikidata entry is known by the page.
        "wikipedia,train-0001.txt,54,81,organisation,wikipedia:Dunedin_Chamber_of_Commerce,\
         Dunedin Chamber of Commerce",
    ] {
        assert!(labels.contains(label), "no `{label}` in:\n{labels}");
    }

    let summary: Value =
        serde_json::from_str(&read(&dir.path().join("results/summary.json"))).unwrap();
    let scenario = |name: &str| {
        summary
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["scenario"] == name)
            .unwrap_or_else(|| panic!("no `{name}` in {summary}"))
    };

    // With the labels for an extractor, end to end is resolution. Two
    // companies called "Pioneer Group" are one identity, and "Northwind" is
    // not "Northwind Traders".
    let gold = scenario("organisation-gold");
    assert_eq!(gold["extraction"]["correct"], 13);
    assert_eq!(gold["resolution"]["false_merges"], 1);
    assert_eq!(gold["resolution"]["false_splits"], 1);
    assert_eq!(gold["end_to_end"]["pairs"], gold["resolution"]["pairs"]);

    // "Reyes" is "Dana Reyes" within a document, and "Dana Reyes" is one
    // person across documents.
    let person = scenario("person-gold");
    assert_eq!(person["resolution"]["records"], 4);
    assert_eq!(person["resolution"]["identities"], 1);

    // The extractor that goes by endings finds 6 of the 13 mentions. It
    // takes "Yesterday" for part of a name, and twice the full stop that
    // ends a sentence for the one "Corp." and "Ltd." are written with.
    let suffix = scenario("organisation-suffix");
    assert_eq!(suffix["extraction"]["extracted"], 9);
    assert_eq!(suffix["extraction"]["correct"], 6);
    assert_eq!(suffix["end_to_end"]["entities_found"], 6);
    assert_eq!(suffix["end_to_end"]["true_entities"], 8);
    assert_eq!(suffix["end_to_end"]["pairs"]["actual"], 7);
    assert_eq!(suffix["end_to_end"]["pairs"]["true_positives"], 0);
}
