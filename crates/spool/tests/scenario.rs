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
        "bench/sources/library.yaml",
        "key: id\ncolumns: [id, title]\n",
    );
    write(
        root,
        "bench/entities/paper/entity.yaml",
        "label: title\nfeatures: [title]\nmapping:\n  library:\n    title: { from: title, transform: lower }\n",
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
         source = \"library\"\nentity_type = \"paper\"\n"
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
    assert_eq!(step.metrics.records, 3);
    assert_eq!(step.metrics.true_entities, 2);
    assert_eq!(step.metrics.pairs.recall, Some(1.0));
    assert_eq!(step.resolve.resolved[0].identities_created, 3);
    // One batch cannot arrive in another order.
    assert_eq!(report.order_sensitivity, None);
}

#[test]
fn a_scenario_file_is_checked_when_it_is_loaded() {
    let cases = [
        (
            "[truth]\nfile = \"t.csv\"\ncolumn = \"work\"\n[[step]]\nname = \"all\"\ningest = []\n",
            "[truth] has `file` or `column`, not both",
        ),
        (
            "[truth]\n[[step]]\nname = \"all\"\ningest = []\n",
            "[truth] needs `file`, `column` or `mentions`",
        ),
        (
            "[truth]\nmentions = \"mentions.csv\"\n[[step]]\nname = \"all\"\n[[step.ingest]]\n\
             file = \"papers.csv\"\nsource = \"library\"\nentity_type = \"paper\"\n",
            "step `all` ingests rows, and [truth] has no `file` or `column`",
        ),
        (
            "[truth]\nmentions = \"mentions.csv\"\n[[step]]\nname = \"all\"\n[[step.documents]]\n\
             dir = \"documents\"\n",
            "missing field `source`",
        ),
        (
            "[truth]\nmentions = \"mentions.csv\"\n[[step]]\nname = \"all\"\n[[step.extract]]\n\
             source = \"shelf\"\n",
            "missing field `extractor`",
        ),
        (
            "[truth]\ncolumn = \"work\"\n[[step]]\nname = \"all\"\n[[step.ingest]]\n\
             source = \"library\"\nentity_type = \"paper\"\n",
            "exactly one of `file` and `dataset`",
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

#[test]
fn an_ingest_can_take_only_the_rows_a_condition_holds_for() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    write(dir.path(), "bench/papers.csv", PAPERS);
    // Every column is text, so a number is cast. A comment in the condition
    // does not reach past it.
    write(
        dir.path(),
        "bench/scenario.toml",
        &scenario("file = \"papers.csv\"\nwhere = \"id::BIGINT % 2 = 0 -- even\""),
    );
    let report = run(dir.path()).unwrap();
    let ingest = &report.steps[0].ingest[0];
    assert_eq!((ingest.received, ingest.inserted), (2, 2));
    // Truth is read from the rows that were taken, and from no others.
    assert_eq!(report.steps[0].metrics.records, 2);
    assert_eq!(report.steps[0].metrics.true_entities, 2);

    let rows = Rows::from_csv_where(dir.path().join("bench/papers.csv"), "title ILIKE '%spools'")
        .unwrap();
    let batches: Vec<_> = rows.map(Result::unwrap).collect();
    let ids: Vec<_> = batches
        .iter()
        .flat_map(|batch| (0..batch.num_rows()).map(|row| cell(batch, 0, row)))
        .collect();
    assert_eq!(ids, [Some("1".to_owned()), Some("2".to_owned())]);

    // A condition that is not SQL over the file's columns names the source
    // and the condition.
    write(
        dir.path(),
        "bench/scenario.toml",
        &scenario("file = \"papers.csv\"\nwhere = \"shelf = 3\""),
    );
    let message = scenario_error(run(dir.path()));
    assert!(
        message.contains("the rows of `library` where `shelf = 3`"),
        "{message}"
    );
    assert!(message.contains("shelf"), "{message}");
}

#[test]
fn the_truth_column_may_be_the_key_of_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    // Two shelves list the same works under the works' own numbers.
    for shelf in ["north", "south"] {
        write(
            root,
            &format!("bench/sources/{shelf}.yaml"),
            "key: work\ncolumns: [work, title]\n",
        );
        write(
            root,
            &format!("bench/{shelf}.csv"),
            "work,title\nw1,On Spools\nw2,Threads\n",
        );
    }
    write(
        root,
        "bench/entities/paper/entity.yaml",
        "label: title\nfeatures: [title]\nmapping:\n  north:\n    title: title\n  south:\n    title: title\n",
    );
    write(
        root,
        "bench/scenario.toml",
        "[truth]\ncolumn = \"work\"\n\n[[step]]\nname = \"all\"\n\n\
         [[step.ingest]]\nfile = \"north.csv\"\nsource = \"north\"\nentity_type = \"paper\"\n\n\
         [[step.ingest]]\nfile = \"south.csv\"\nsource = \"south\"\nentity_type = \"paper\"\n",
    );
    let report = run(root).unwrap();
    let step = &report.steps[0];
    // The key stays in the rows: each record has it, and is scored by it.
    assert_eq!(step.ingest[0].inserted, 2);
    assert_eq!(step.metrics.records, 4);
    assert_eq!(step.metrics.true_entities, 2);
    assert_eq!(step.metrics.pairs.precision, Some(1.0));
    assert_eq!(step.metrics.pairs.recall, Some(1.0));
}

#[test]
fn a_dataset_that_is_an_archive_is_unpacked_once_and_an_ingest_names_a_file_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    write(root, "upstream/shelves/papers.csv", PAPERS);
    write(root, "upstream/shelves/more.csv", MORE);
    let archive = root.join("upstream/shelves.tar.gz");
    let packed = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(root.join("upstream"))
        .arg("shelves")
        .status()
        .unwrap();
    assert!(packed.success());
    let checksum: String = Sha256::digest(fs::read(&archive).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let datasets = |archive_line: &str| {
        format!(
            "[[dataset]]\nname = \"shelves\"\nurl = \"file://{}\"\nsha256 = \"{checksum}\"\n\
             licence = \"invented for this test\"\n{archive_line}",
            archive.display()
        )
    };
    let ingest = |member: &str| scenario(&format!("dataset = \"shelves\"\n{member}"));

    write(root, "datasets.toml", &datasets("archive = \"tar\"\n"));
    write(
        root,
        "bench/scenario.toml",
        &ingest("member = \"shelves/papers.csv\""),
    );
    assert_eq!(run(root).unwrap().steps[0].ingest[0].inserted, 4);
    assert!(root.join(".cache/shelves/shelves/more.csv").is_file());
    // What is kept is the unpacked copy, not the archive beside it.
    assert!(!root.join(".cache/shelves.tar").exists());

    // The unpacked copy is used from then on, and another file of it is
    // there without a second download.
    fs::remove_file(&archive).unwrap();
    write(
        root,
        "bench/scenario.toml",
        &ingest("member = \"shelves/more.csv\""),
    );
    assert_eq!(run(root).unwrap().steps[0].ingest[0].inserted, 3);

    let cases = [
        (
            ingest("member = \"shelves/posters.csv\""),
            "the archive has no file `shelves/posters.csv`",
        ),
        (
            ingest("member = \"../papers.csv\""),
            "`../papers.csv` does not name a file within the archive",
        ),
        (
            ingest(""),
            "dataset `shelves` is an archive: the ingest names the file to read with `member`",
        ),
    ];
    for (scenario, expected) in cases {
        write(root, "bench/scenario.toml", &scenario);
        let message = scenario_error(run(root));
        assert!(
            message.contains(expected),
            "expected `{expected}` in: {message}"
        );
    }

    // A dataset that is one file has no members, and only `tar` is an archive.
    write(root, "datasets.toml", &datasets(""));
    write(
        root,
        "bench/scenario.toml",
        &ingest("member = \"shelves/papers.csv\""),
    );
    let message = scenario_error(run(root));
    assert!(message.contains("is one file"), "{message}");
    write(root, "datasets.toml", &datasets("archive = \"zip\"\n"));
    let message = scenario_error(run(root));
    assert!(message.contains("unknown variant `zip`"), "{message}");

    // `member` belongs to a dataset.
    write(
        root,
        "bench/scenario.toml",
        &scenario("file = \"papers.csv\"\nmember = \"shelves/papers.csv\""),
    );
    let message = scenario_error(Scenario::load(root.join("bench")));
    assert!(
        message.contains("`member` names a file within a `dataset`"),
        "{message}"
    );
}

/// Papers in two batches, with a reviewer between them.
const REVIEWED: &str = r#"
[truth]
column = "work"

[[step]]
name = "first"
[[step.ingest]]
file = "papers.csv"
source = "library"
entity_type = "paper"

[[step]]
name = "a mistake"
[[step.correct]]
name = "wrong"
merge = ["library:3", "library:4"]

[[step]]
name = "put right"
[[step.correct]]
undo = "wrong"
[[step.correct]]
split = ["library:2"]
reason = "another edition"

[[step]]
name = "second"
[[step.ingest]]
file = "more.csv"
source = "library"
entity_type = "paper"

[[step]]
name = "review"
[[step.correct]]
assign = ["library:5", "library:1"]
"#;

/// A fifth paper that matches both sides of the split, a second record of
/// "Threads", and a new title for the paper with the key 4.
const MORE: &str = "\
id,title,work
5,On Spools,w1
6,threads,w2
4,Bobbins and Reels,w3
";

fn reviewed(root: &Path) {
    project(root);
    write(
        root,
        "bench/papers.csv",
        &PAPERS.replace("3,Threads,", "3,Threads,w2"),
    );
    write(root, "bench/more.csv", MORE);
    write(root, "bench/scenario.toml", REVIEWED);
}

#[test]
fn corrections_are_applied_as_a_reviewer_would_and_measured_after_each_step() {
    let dir = tempfile::tempdir().unwrap();
    reviewed(dir.path());
    let report = run(dir.path()).unwrap();
    let [first, mistake, right, second, review] = &report.steps[..] else {
        panic!("five steps");
    };

    assert_eq!(first.corrections, []);
    assert_eq!(first.churn.new_records, 4);
    assert_eq!(first.churn.identities_created, 3);
    assert_eq!(first.churn.created_per_thousand, Some(750.0));
    assert_eq!(first.split_recovery.splits, 0);
    assert_eq!(first.override_persistence.share, None);

    // The merge puts two works in one identity. Whoever held the merged ID
    // is redirected, so only the strict measure counts it as a change.
    let kinds = |step: &spool::StepReport| -> Vec<String> {
        step.corrections
            .iter()
            .map(|correction| correction.kind.clone())
            .collect()
    };
    assert_eq!(kinds(mistake), ["merge"]);
    assert_eq!(mistake.corrections[0].name.as_deref(), Some("wrong"));
    assert_eq!(mistake.corrections[0].seq, mistake.head);
    assert_eq!(mistake.metrics.false_merges, 1);
    let stability = &mistake.id_stability;
    assert_eq!(
        (
            stability.records,
            stability.unchanged,
            stability.unchanged_following_redirects
        ),
        (4, 3, 4)
    );
    let churn = &mistake.churn;
    assert_eq!(
        (
            churn.identities_created,
            churn.identities_merged,
            churn.records_reassigned
        ),
        (0, 1, 1)
    );
    // Nothing arrived, so there is no rate.
    assert_eq!(churn.merged_per_thousand, None);

    // The undo brings the merged identity back, and the split makes one.
    assert_eq!(kinds(right), ["undo", "split"]);
    assert_eq!(right.metrics.false_merges, 0);
    assert_eq!(right.metrics.identities, 4);
    let churn = &right.churn;
    assert_eq!(
        (
            churn.identities_created,
            churn.identities_merged,
            churn.records_reassigned
        ),
        (1, 0, 2)
    );
    assert_eq!(right.split_recovery.splits, 1);
    assert_eq!(right.split_recovery.share, Some(1.0));
    assert_eq!(right.override_persistence.constraints, 1);
    assert_eq!(right.override_persistence.share, Some(1.0));

    // The next batch has a paper that matches both sides of the split. It is
    // left for review, and the sides stay apart.
    assert_eq!(second.ingest[0].inserted, 3);
    assert_eq!(second.ingest[0].superseded, 1);
    assert_eq!(second.resolve.resolved[0].review.len(), 1);
    assert_eq!(second.metrics.unassigned, 1);
    assert_eq!(second.split_recovery.kept_apart, 1);
    assert_eq!(second.override_persistence.honoured, 1);
    assert_eq!(second.id_stability.strict, Some(1.0));
    // The row that changed went where its earlier version is, though the
    // titles no longer match.
    assert_eq!(second.metrics.records, 7);
    assert_eq!(second.metrics.false_splits, 1);

    // The reviewer places it, and a run has nothing left to do.
    assert_eq!(kinds(review), ["assign"]);
    assert_eq!(review.metrics.unassigned, 0);
    assert_eq!(review.resolve.resolved, []);
    assert_eq!(review.split_recovery.share, Some(1.0));

    // Without the corrections, these batches give the same groups in either
    // order: seven records in three identities.
    let order = report.order_sensitivity.as_ref().unwrap();
    assert_eq!((order.records, order.same_group), (7, 7));
    assert_eq!((order.pairs_in_both, order.pairs_in_either), (5, 5));
    assert_eq!(order.pair_agreement, Some(1.0));

    // The same scenario gives the same report.
    assert_eq!(run(dir.path()).unwrap(), report);
}

#[test]
fn a_correction_that_cannot_be_made_names_its_step() {
    let cases = [
        (
            "merge = [\"library:3\", \"library:9\"]",
            "no record is `library:9`",
        ),
        // The two are one identity already.
        (
            "merge = [\"library:1\", \"library:2\"]",
            "changeset rejected",
        ),
        ("split = [\"library:3\"]", "identity"),
    ];
    for (correction, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        reviewed(dir.path());
        write(
            dir.path(),
            "bench/scenario.toml",
            &REVIEWED.replace("merge = [\"library:3\", \"library:4\"]", correction),
        );
        let message = scenario_error(run(dir.path()));
        assert!(
            message.contains("step `a mistake`, correction 1"),
            "{message}"
        );
        assert!(
            message.contains(expected),
            "expected `{expected}` in: {message}"
        );
    }
}

#[test]
fn corrections_are_checked_when_the_scenario_is_loaded() {
    let cases = [
        (
            "merge = [\"a:1\", \"a:2\"]\nsplit = [\"a:1\"]",
            "exactly one of",
        ),
        ("reason = \"none\"", "exactly one of"),
        ("split = []", "a split needs records"),
        ("merge = [\"a:1\", \"2\"]", "`2` does not name a record"),
        ("merge = [\"a:1\"]", "scenario.toml"),
        ("undo = \"later\"", "no earlier correction is named `later`"),
        (
            "name = \"x\"\nmerge = [\"a:1\", \"a:2\"]\n[[step.correct]]\nname = \"x\"\nundo = \"x\"",
            "two corrections are named `x`",
        ),
        ("dismiss = [\"a:1\"]", "unknown field `dismiss`"),
    ];
    for (correction, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "bench/scenario.toml",
            &format!(
                "[truth]\ncolumn = \"work\"\n[[step]]\nname = \"review\"\n[[step.correct]]\n{correction}\n"
            ),
        );
        let message = scenario_error(Scenario::load(dir.path().join("bench")));
        assert!(
            message.contains(expected),
            "expected `{expected}` in: {message}"
        );
    }
}

/// A scenario under `<root>/bench` that stores two documents and labels the
/// papers they mention.
fn shelf(root: &Path, labels: &str) {
    project(root);
    write(root, "bench/documents/a.txt", "📚 On Spools was reprinted.");
    write(root, "bench/documents/b.txt", "See On Spools, and Threads.");
    write(root, "bench/documents/.hidden", "not a document");
    write(root, "bench/mentions.csv", labels);
    write(
        root,
        "bench/scenario.toml",
        "[truth]\nmentions = \"mentions.csv\"\n\n[[step]]\nname = \"all\"\n\n\
         [[step.documents]]\ndir = \"documents\"\nsource = \"shelf\"\n",
    );
}

const LABELS: &str = "\
source,document,start,end,entity_type,true_entity,text
shelf,a.txt,2,11,paper,w1,On Spools
shelf,b.txt,4,13,paper,w1,On Spools
shelf,b.txt,19,26,paper,w2,Threads
";

#[test]
fn documents_are_stored_and_their_labelled_mentions_are_what_an_extractor_should_find() {
    let dir = tempfile::tempdir().unwrap();
    shelf(dir.path(), LABELS);
    let report = run(dir.path()).unwrap();
    let step = &report.steps[0];

    // Every file but a hidden one is a document, and its name is its key.
    assert_eq!(step.documents.len(), 1);
    let stored = &step.documents[0];
    assert_eq!(
        (stored.source.as_str(), stored.received, stored.inserted),
        ("shelf", 2, 2)
    );
    assert!(step.ingest.is_empty() && step.extract.is_empty());

    // Nothing read the documents, so every labelled mention is missed, and
    // precision has nothing to be a share of.
    let extraction = step.extraction.as_ref().unwrap();
    assert_eq!(
        (
            extraction.labelled,
            extraction.extracted,
            extraction.correct
        ),
        (3, 0, 0)
    );
    assert_eq!(extraction.precision, None);
    assert_eq!(extraction.recall, Some(0.0));
    assert_eq!(extraction.f1, Some(0.0));
    // And there is no record for resolution to be scored on.
    assert_eq!(step.metrics.records, 0);

    // A scenario without labelled mentions does not report extraction.
    let rows = tempfile::tempdir().unwrap();
    project(rows.path());
    write(rows.path(), "bench/papers.csv", PAPERS);
    write(
        rows.path(),
        "bench/scenario.toml",
        &scenario("file = \"papers.csv\""),
    );
    assert_eq!(run(rows.path()).unwrap().steps[0].extraction, None);
}

#[test]
fn labelled_mentions_are_checked_against_the_documents() {
    let cases = [
        (
            "source,document,start,end,true_entity\nshelf,a.txt,2,11,w1\n",
            "needs the columns `source`, `document`, `start`, `end`, `entity_type` and `true_entity`",
        ),
        (
            "source,document,start,end,entity_type,true_entity\nshelf,a.txt,two,11,paper,w1\n",
            "line 2: `start` is not a position",
        ),
        (
            "source,document,start,end,entity_type,true_entity\nshelf,a.txt,2,11,paper,\n",
            "line 2 has no `true_entity`",
        ),
        // Positions count code points: the emoji is one, and the label that
        // counts it as two is off by one.
        (
            "source,document,start,end,entity_type,true_entity,text\n\
             shelf,a.txt,3,12,paper,w1,On Spools\n",
            "the labelled mention of `w1` at 3-12 in document `a.txt` of `shelf`: \
             the label says the text there is `On Spools`, and it is `n Spools `",
        ),
        (
            "source,document,start,end,entity_type,true_entity\nshelf,a.txt,20,40,paper,w1\n",
            "the document is 26 long",
        ),
        (
            "source,document,start,end,entity_type,true_entity\nshelf,a.txt,11,11,paper,w1\n",
            "the document is 26 long",
        ),
    ];
    for (labels, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        shelf(dir.path(), labels);
        let message = scenario_error(run(dir.path()));
        assert!(
            message.contains(expected),
            "expected `{expected}` in: {message}"
        );
    }
}
