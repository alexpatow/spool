//! Scenario files, CSV rows and checksummed datasets.

use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};
use spool::arrow::record_batch::RecordBatchReader;
use spool::{Engine, Options, Rows};
use spool_bench::{BenchReport, Error, PossibleSplits, Scenario, StepReport, take_column};
use spool_testing::cell;

mod corrections;
mod datasets;
mod documents;
mod retractions;

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

fn run(root: &Path) -> spool_bench::Result<BenchReport> {
    let engine = Engine::open_in_memory(Options::seeded(0))?;
    Scenario::load(root.join("bench"))?.run(&engine, |_| Ok(()))
}

fn scenario_error(result: spool_bench::Result<impl std::fmt::Debug>) -> String {
    match result {
        Err(Error::Scenario(message)) => message,
        other => panic!("expected a scenario error, got {other:?}"),
    }
}

fn sha256(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
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

    let (rest, truth) = take_column(rows, "work", "id").unwrap();
    assert_eq!(rest.schema().fields().len(), 2);
    assert_eq!(
        truth,
        [("1", "w1"), ("2", "w1"), ("4", "w3")].map(|(id, work)| (id.to_owned(), work.to_owned()))
    );
    let batches: Vec<_> = rest.map(Result::unwrap).collect();
    assert_eq!(cell(&batches[0], 0, 0).as_deref(), Some("1"));
    assert_eq!(cell(&batches[0], 1, 2).as_deref(), Some("Threads"));

    let missing = Rows::from_csv(dir.path().join("nothing.csv"));
    assert!(matches!(missing, Err(spool::Error::Io { .. })));
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

    let rows =
        Rows::from_csv_where(dir.path().join("bench/papers.csv"), "title ILIKE '%spools'").unwrap();
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

/// Papers in two batches, with a reviewer between them.
#[test]
fn a_scenario_is_checked_against_its_project_before_anything_is_downloaded() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path());
    let checked = |ingest: &str| {
        write(dir.path(), "bench/scenario.toml", &scenario(ingest));
        scenario_error(run(dir.path()))
    };

    // The dataset would be fetched from a file that is not there, and the
    // run never gets that far.
    write(
        dir.path(),
        "datasets.toml",
        &format!(
            "[[dataset]]\nname = \"papers\"\nurl = \"file://{}\"\nsha256 = \"{}\"\nlicence = \"test\"\n",
            dir.path().join("nowhere.csv").display(),
            sha256(PAPERS)
        ),
    );
    let message = checked("dataset = \"pamphlets\"");
    assert!(
        message.contains("the ingest from `library`")
            && message.contains("no dataset named `pamphlets`"),
        "{message}"
    );
    let message = checked("dataset = \"papers\"\nmember = \"papers.csv\"");
    assert!(message.contains("is one file"), "{message}");
    let message = checked("file = \"papers.csv\"");
    assert!(message.contains("there is no file"), "{message}");

    write(dir.path(), "bench/papers.csv", PAPERS);
    let unknown = |source: &str, entity_type: &str| {
        write(
            dir.path(),
            "bench/scenario.toml",
            &format!(
                "[truth]\ncolumn = \"work\"\n\n[[step]]\nname = \"all\"\n\n[[step.ingest]]\n\
                 dataset = \"papers\"\nsource = \"{source}\"\nentity_type = \"{entity_type}\"\n"
            ),
        );
        scenario_error(run(dir.path()))
    };
    let message = unknown("library", "book");
    assert!(
        message.contains("the project has no entity type `book`"),
        "{message}"
    );
    let message = unknown("archive", "paper");
    assert!(
        message.contains("the mapping of `paper` has no block for the source"),
        "{message}"
    );
    assert!(!dir.path().join(".cache").exists());
}

/// Every scenario file under `benchmarks`.
fn committed_scenarios() -> Vec<std::path::PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if !name.starts_with('.') {
                    walk(&path, found);
                }
            } else if name.ends_with(".toml") && name != "datasets.toml" {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(&spool_testing::benchmarks(), &mut found);
    found.sort();
    found
}

#[test]
fn every_committed_scenario_fits_its_project_and_the_datasets_that_are_listed() {
    // The big scenarios are run by hand, so nothing else would notice one
    // that names a source its project has lost.
    let scenarios = committed_scenarios();
    assert!(scenarios.len() >= 12, "{scenarios:?}");
    for path in scenarios {
        Scenario::load(&path)
            .and_then(|scenario| scenario.check())
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
}

#[test]
fn each_source_of_the_music_project_is_read_in_its_own_way() {
    let engine = Engine::open_in_memory(Options::seeded(0)).unwrap();
    let report = Scenario::load(spool_testing::benchmarks().join("resolution/music/sample.toml"))
        .unwrap()
        .run(&engine, |_| Ok(()))
        .unwrap();
    let features = |key: &str| {
        spool_testing::select(
            &engine,
            &format!(
                "SELECT f.title, f.artist, f.album, f.number::VARCHAR, f.seconds::VARCHAR, f.year, f.language
                 FROM spool.features_song f JOIN spool.record o USING (record_id)
                 WHERE o.source_key = '{key}'"
            ),
        )
        .remove(0)
        .into_iter()
        .map(|value| value.unwrap_or_else(|| "-".to_owned()))
        .collect::<Vec<_>>()
        .join(" | ")
    };

    // One song as each of the five sources has it.
    assert_eq!(
        features("116832"),
        "enslavers | hellbastard | natural order | 6 | 261 | 92 | -"
    );
    assert_eq!(
        features("5800"),
        "enslavers | hellbastard | natural order | 6 | 261 | 92 | eng"
    );
    assert_eq!(
        features("66518"),
        "enslavers | hellbastard | natural order | 6 | 261 | 92 | eng"
    );
    assert_eq!(
        features("91071"),
        "enslavers | hellbastard | natural order | 6 | 261 | 92 | eng"
    );
    assert_eq!(
        features("123090"),
        "enslavers | hellbastard | natural order | 6 | 261 | 92 | eng"
    );

    // Values a source has no way of writing, or wrote badly, are missing
    // and not wrong.
    assert_eq!(
        features("8"),
        "ayebo | djelimady | sigui | - | 278 | 01 | -"
    );
    assert_eq!(
        features("10"),
        "try acoustic | neil young | 2008 02 15 le grand rex paris france | 6 | - | - | eng"
    );
    assert_eq!(
        features("9"),
        "i mready | john lee hooker | the collection | 3 | 161 | 06 | eng"
    );
    assert_eq!(
        features("24"),
        "gimme dat remix | rye rye | ryeot powrr | 16 | 263 | 11 | -"
    );
    assert_eq!(
        features("2"),
        "双剣乱舞 みょん me | silver forest | silver forest 2006 2012 best1 | 11 | 229 | 12 | jap"
    );
    assert_eq!(
        features("39"),
        "go the distance hercules | alan menken | disney karaoke volume 1 | 5 | - | - | -"
    );

    // Two of the four songs that are in every source are found whole. Each
    // of the other two has a record with the wrong track number, which the
    // rules take for another song, because an album that came out twice has
    // the same titles at other numbers. Nothing is put with a song that is
    // not it.
    let metrics = &report.steps[0].metrics;
    assert_eq!((metrics.records, metrics.true_entities), (26, 10));
    assert_eq!(metrics.false_merges, 0);
    assert_eq!(metrics.false_splits, 2);
    assert_eq!(metrics.pairs.true_positives, 10 + 6 + 10 + 6);
    assert_ne!(
        spool_testing::identity(&engine, "source_3", "101459"),
        spool_testing::identity(&engine, "source_5", "135493")
    );
}

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
