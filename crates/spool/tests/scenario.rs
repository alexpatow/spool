//! Scenario files, CSV rows and checksummed datasets.

mod common;

use std::fs;
use std::path::Path;

use common::cell;
use sha2::{Digest, Sha256};
use spool::arrow::record_batch::RecordBatchReader;
use spool::{Engine, Error, Options, Rows, Scenario};

const PAPERS: &str = "\
id,title,work
1,On Spools,w1
2,on spools,w1
3,Threads,
4,Bobbins,w3
";

fn write(root: &Path, file: &str, contents: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A project that resolves papers by title, under `<root>/bench`.
fn project(root: &Path) {
    write(
        root,
        "bench/entities/paper/features.sql",
        "SELECT observation_id, lower(payload->>'title') AS title FROM observation",
    );
    write(
        root,
        "bench/entities/paper/rules/block/same_title.sql",
        "l.title = r.title",
    );
    write(
        root,
        "bench/entities/paper/rules/match/same_title.sql",
        "l.title = r.title",
    );
}

fn scenario(ingest: &str) -> String {
    format!(
        "[truth]\ncolumn = \"work\"\n\n[[step]]\nname = \"all\"\n\n[[step.ingest]]\n{ingest}\n\
         source = \"library\"\nentity_type = \"paper\"\nkey = \"id\"\n"
    )
}

fn run(root: &Path) -> spool::Result<spool::BenchReport> {
    let engine = Engine::open_in_memory(Options::seeded(0))?;
    Scenario::load(root.join("bench"))?.run(&engine, |_| Ok(()))
}

fn scenario_error(result: spool::Result<impl std::fmt::Debug>) -> String {
    match result {
        Err(Error::Scenario(message)) => message,
        other => panic!("expected a scenario error, got {other:?}"),
    }
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn csv_rows_are_text_and_an_empty_field_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "papers.csv", PAPERS);
    let rows = Rows::from_csv(dir.path().join("papers.csv")).unwrap();

    let names: Vec<String> = rows
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect();
    assert_eq!(names, ["id", "title", "work"]);

    let (rest, truth) = rows.take_column("work", "id").unwrap();
    assert_eq!(rest.schema().fields().len(), 2);
    assert_eq!(
        truth,
        [("1", "w1"), ("2", "w1"), ("4", "w3")].map(|(id, work)| (id.to_owned(), work.to_owned()))
    );
    let batches: Vec<_> = rest.map(Result::unwrap).collect();
    assert_eq!(cell(&batches[0], 0, 0).as_deref(), Some("1"));
    assert_eq!(cell(&batches[0], 1, 2).as_deref(), Some("Threads"));

    let missing = Rows::from_csv(dir.path().join("nothing.csv"));
    assert!(matches!(missing, Err(Error::Io { .. })));
}

#[test]
fn truth_can_be_a_column_of_the_ingested_file() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    write(dir.path(), "bench/papers.csv", PAPERS);
    write(
        dir.path(),
        "bench/scenario.toml",
        &scenario("file = \"papers.csv\""),
    );

    let report = run(dir.path()).unwrap();
    assert_eq!(report.scenario, "bench");
    let step = &report.steps[0];
    assert_eq!(step.ingest[0].inserted, 4);
    // The truth column is not an attribute, and a row without truth is not scored.
    assert_eq!(step.metrics.observations, 3);
    assert_eq!(step.metrics.true_entities, 2);
    assert_eq!(step.metrics.pairs.recall, Some(1.0));
    assert_eq!(step.resolve.resolved[0].identities_created, 3);
}

#[test]
fn a_scenario_file_is_checked_when_it_is_loaded() {
    let cases = [
        (
            "[truth]\nfile = \"t.csv\"\ncolumn = \"work\"\n[[step]]\nname = \"all\"\ningest = []\n",
            "exactly one of `file` and `column`",
        ),
        (
            "[truth]\n[[step]]\nname = \"all\"\ningest = []\n",
            "exactly one of `file` and `column`",
        ),
        (
            "[truth]\ncolumn = \"work\"\n[[step]]\nname = \"all\"\n[[step.ingest]]\n\
             source = \"library\"\nentity_type = \"paper\"\nkey = \"id\"\n",
            "exactly one of `file` and `dataset`",
        ),
        (
            "[truth]\ncolumn = \"work\"\n[[step]]\nname = \"all\"\n[[step.ingest]]\n\
             file = \"papers.csv\"\nsource = \"library\"\nentity_type = \"paper\"\n",
            "needs a `key` column",
        ),
        (
            "[truth]\ncolumn = \"work\"\nshuffle = true\n[[step]]\nname = \"all\"\ningest = []\n",
            "unknown field `shuffle`",
        ),
    ];
    for (text, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "bench/scenario.toml", text);
        let message = scenario_error(Scenario::load(dir.path().join("bench")));
        assert!(
            message.contains(expected),
            "expected `{expected}` in: {message}"
        );
        assert!(message.contains("scenario.toml"), "{message}");
    }

    let nowhere = tempfile::tempdir().unwrap();
    assert!(matches!(
        Scenario::load(nowhere.path()),
        Err(Error::Io { .. })
    ));
}

#[test]
fn a_truth_file_needs_its_three_columns() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    write(dir.path(), "bench/papers.csv", "id,title\n1,On Spools\n");
    write(
        dir.path(),
        "bench/truth.csv",
        "source,key,entity\nlibrary,1,w1\n",
    );
    write(
        dir.path(),
        "bench/scenario.toml",
        &scenario("file = \"papers.csv\"").replace("column = \"work\"", "file = \"truth.csv\""),
    );
    let message = scenario_error(run(dir.path()));
    assert!(
        message.contains("`source`, `source_key` and `true_entity`"),
        "{message}"
    );
}

#[test]
fn a_dataset_is_downloaded_once_and_checked_against_its_checksum() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    write(
        root,
        "bench/scenario.toml",
        &scenario("dataset = \"papers\""),
    );
    write(root, "upstream/papers.csv", PAPERS);
    let datasets = |sha256: &str| {
        format!(
            "[[dataset]]\nname = \"papers\"\nurl = \"file://{}\"\nsha256 = \"{sha256}\"\nlicence = \"invented for this test\"\n",
            root.join("upstream/papers.csv").display()
        )
    };

    // No datasets.toml above the scenario.
    let message = scenario_error(run(root));
    assert!(message.contains("no datasets.toml"), "{message}");

    // A download that is not the file the checksum names is refused and not kept.
    write(root, "datasets.toml", &datasets(&sha256("something else")));
    let message = scenario_error(run(root));
    assert!(message.contains("SHA-256"), "{message}");
    assert!(!root.join(".cache/papers.csv").exists());
    assert!(!root.join(".cache/papers.csv.part").exists());

    write(root, "datasets.toml", &datasets(&sha256(PAPERS)));
    assert_eq!(run(root).unwrap().steps[0].ingest[0].inserted, 4);
    assert_eq!(
        fs::read_to_string(root.join(".cache/papers.csv")).unwrap(),
        PAPERS
    );

    // The kept copy is used from then on.
    fs::remove_file(root.join("upstream/papers.csv")).unwrap();
    assert_eq!(run(root).unwrap().steps[0].ingest[0].inserted, 4);

    // A kept copy that no longer matches is fetched again, which now fails.
    write(root, ".cache/papers.csv", "id,title,work\n");
    let message = scenario_error(run(root));
    assert!(message.contains("downloading"), "{message}");

    write(
        root,
        "bench/scenario.toml",
        &scenario("dataset = \"posters\""),
    );
    let message = scenario_error(run(root));
    assert!(message.contains("no dataset named `posters`"), "{message}");
}
