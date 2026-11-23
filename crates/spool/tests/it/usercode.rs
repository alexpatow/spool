//! User-code strategies: files in the project that the engine runs as child
//! processes.
//!
//! Almost every test here starts `uv`, which fetches a Python and `pyarrow`
//! the first time, or `bun`, so they are ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool --test it -- --include-ignored usercode::
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use spool::core::{Actor, EntityType, Op};
use spool::{Engine, Error, Manifest, Options, ResolveOptions, RunFilter, RunStatus, UserCode};
use spool_bench::Scenario;
use spool_testing::{benchmarks, identity, load, members, record_id};

mod checks;
mod typescript;

/// The content hash of every changeset in the log.
fn hashes(engine: &Engine) -> Vec<String> {
    engine
        .log()
        .unwrap()
        .iter()
        .map(|entry| entry.content_hash.to_string())
        .collect()
}

fn user_code(project: &Path, work: &Path) -> UserCode {
    UserCode {
        work_dir: work.to_owned(),
        timeout: Duration::from_secs(300),
        ..UserCode::new(project)
    }
}

#[test]
#[ignore = "runs uv"]
fn a_python_strategy_with_the_rules_commits_what_the_rules_commit() {
    let scenario = Scenario::load(benchmarks().join("identity/companies-small")).unwrap();
    let work = tempfile::tempdir().unwrap();

    let with_rules = Engine::open_in_memory(Options::seeded(7)).unwrap();
    let rules = scenario.run(&with_rules, |_| Ok(())).unwrap();

    let with_python = Engine::open_in_memory(Options::seeded(7)).unwrap();
    with_python.set_user_code(user_code(&scenario.project_dir(), work.path()));
    let python = scenario
        .with_strategy("in_python")
        .run(&with_python, |_| Ok(()))
        .unwrap();

    assert_eq!(hashes(&with_python), hashes(&with_rules));
    assert!(!hashes(&with_rules).is_empty());
    for (python, rules) in python.steps.iter().zip(&rules.steps) {
        assert_eq!(python.metrics, rules.metrics);
        assert_eq!(
            python.resolve.resolved[0].judgements,
            rules.resolve.resolved[0].judgements
        );
    }
}

/// A project on disk with the `thing` entity type of the other tests, and
/// the strategies a test writes into it.
struct Things {
    dir: tempfile::TempDir,
    engine: Arc<Engine>,
}

impl Things {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let things = Self {
            engine: Arc::new(Engine::open_in_memory(Options::seeded(1)).unwrap()),
            dir,
        };
        for source in ["shop", "depot"] {
            things.write(
                &format!("../../sources/{source}.yaml"),
                "key: id\ncolumns: [id, name, code]\n",
            );
        }
        things.write(
            "entity.yaml",
            "label: name\nfeatures: [name, code]\nmapping:\n  \
             shop:\n    name: name\n    code: code\n  \
             depot:\n    name: name\n    code: code\n",
        );
        things.write("rules/block/same_name.sql", "l.name = r.name");
        things.write("rules/block/same_code.sql", "l.code = r.code");
        things.write("rules/match/same_name.sql", "l.name = r.name");
        things.engine.set_user_code(UserCode {
            work_dir: things.work(),
            timeout: Duration::from_secs(300),
            ..UserCode::new(things.project())
        });
        things
    }

    fn project(&self) -> PathBuf {
        self.dir.path().join("project")
    }

    /// Where tasks are made, so that a test sees the ones that were kept.
    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn write(&self, file: &str, contents: &str) {
        let path = self.project().join("entities/thing").join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// Writes a strategy file, compiles the project and applies it.
    fn strategy(&self, file: &str, source: &str) {
        self.write(&format!("strategies/{file}"), source);
        self.engine
            .apply(&Manifest::load(self.project()).unwrap())
            .unwrap();
    }

    fn resolve(&self, strategy: &str) -> spool::Result<spool::ResolveReport> {
        self.engine
            .resolve(&ResolveOptions::with_strategy(strategy))
    }

    /// The task directories that were kept.
    fn tasks(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.work()) else {
            return Vec::new();
        };
        let mut tasks: Vec<PathBuf> = entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("run-"))
            })
            .collect();
        tasks.sort();
        tasks
    }
}

/// Matches candidates whose names are the same, as the SQL rule does.
const SAME_NAME: &str = r#"
from spool import strategy

@strategy
def judge(candidates):
    return [
        {"left_id": pair["left_id"], "right_id": pair["right_id"], "verdict": "match"}
        for pair in candidates.to_pylist()
        if pair["left_name"] is not None and pair["left_name"] == pair["right_name"]
    ]
"#;

fn two_anvils_and_a_hammer(engine: &Engine) {
    load(
        engine,
        "shop",
        &[
            ("1", Some("anvil"), Some("K1")),
            ("2", Some("anvil"), Some("K2")),
            ("3", Some("hammer"), None),
        ],
    );
}

fn strategy_error(result: spool::Result<spool::ResolveReport>) -> String {
    match result {
        Err(Error::Strategy { strategy, message }) => {
            assert_eq!(strategy, "model");
            message
        }
        other => panic!("expected the strategy to fail, got {other:?}"),
    }
}

#[test]
#[ignore = "runs uv"]
fn a_strategy_judges_and_the_engine_commits() {
    let things = Things::new();
    things.strategy("model.py", SAME_NAME);
    let engine = &things.engine;
    two_anvils_and_a_hammer(engine);

    let report = things.resolve("model").unwrap();
    let [resolved] = &report.resolved[..] else {
        panic!("one entity type was resolved");
    };
    assert_eq!(resolved.judgements.match_, 1);
    assert_eq!(resolved.identities_created, 2);
    let anvils = identity(engine, "shop", "1").unwrap();
    assert_eq!(
        members(engine, anvils),
        ["shop:1".to_owned(), "shop:2".to_owned()].into()
    );

    // The run names the strategy and the hash of the file that ran.
    let run = engine.run(resolved.match_run).unwrap();
    assert_eq!(run.status, RunStatus::Succeeded);
    let strategy = run.strategy.unwrap();
    let built = engine.manifest().unwrap();
    let built = built
        .entity_type("thing")
        .unwrap()
        .strategy("model")
        .unwrap();
    assert_eq!(strategy.name, "model");
    assert_eq!(strategy.version.as_deref(), Some(built.hash.as_str()));

    // A task that succeeded leaves nothing behind.
    assert!(things.tasks().is_empty(), "{:?}", things.tasks());
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn a_strategy_that_fails_fails_its_run_and_changes_nothing() {
    let things = Things::new();
    things.strategy(
        "model.py",
        r#"
import sys
from spool import strategy

@strategy
def judge(candidates):
    print("about to fail", file=sys.stderr)
    raise ValueError(f"cannot judge {candidates.num_rows} pairs")
"#,
    );
    let engine = &things.engine;
    two_anvils_and_a_hammer(engine);

    let message = strategy_error(things.resolve("model"));
    assert!(
        message.contains("it failed with exit status: 1"),
        "{message}"
    );
    assert!(message.contains("about to fail"), "{message}");
    assert!(
        message.contains("ValueError: cannot judge 1 pairs"),
        "{message}"
    );

    // Nothing was committed, and the run says why.
    assert_eq!(engine.head().unwrap().0, 0);
    assert_eq!(identity(engine, "shop", "1"), None);
    let runs = engine
        .runs(&RunFilter {
            kind: Some(spool::RunKind::Match),
            ..RunFilter::default()
        })
        .unwrap();
    let [run] = &runs[..] else {
        panic!("one match run");
    };
    assert_eq!(run.status, RunStatus::Failed);
    assert!(run.error.as_deref().unwrap().contains("ValueError"));

    // The task is kept, with what the strategy was given and what it said.
    let [task] = &things.tasks()[..] else {
        panic!("one task was kept: {:?}", things.tasks());
    };
    assert!(message.contains(&task.display().to_string()), "{message}");
    for file in [
        "task.json",
        "candidates.parquet",
        "features.parquet",
        "stderr.log",
    ] {
        assert!(task.join(file).is_file(), "{file}");
    }
    assert!(!task.join("judgements.parquet").exists());
    let described: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(task.join("task.json")).unwrap()).unwrap();
    assert_eq!(described["protocol"], 1);
    assert_eq!(described["entity_type"], "thing");
    assert_eq!(described["strategy"]["name"], "model");
    assert_eq!(described["run"], run.run.0);

    // Once the strategy is put right, the next run examines the same
    // records.
    things.strategy("model.py", SAME_NAME);
    let report = things.resolve("model").unwrap();
    assert_eq!(report.resolved[0].new_records, 3);
    assert_eq!(report.resolved[0].identities_created, 2);
    engine.verify().unwrap();
}

/// Runs a `bun` command in the package the TypeScript helper is built from.
fn bun(arguments: &[&str]) {
    let package = benchmarks().join("../packages/strategy");
    let output = std::process::Command::new("bun")
        .args(arguments)
        .current_dir(&package)
        .output()
        .expect("bun is installed");
    assert!(
        output.status.success(),
        "bun {arguments:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_engine_that_was_not_told_where_the_project_is_runs_no_user_code() {
    let dir = tempfile::tempdir().unwrap();
    let write = |file: &str, contents: &str| {
        let path = dir.path().join("entities/thing").join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    };
    write("../../sources/shop.yaml", "key: id\ncolumns: [id, name]\n");
    write(
        "entity.yaml",
        "features: [name]\nmapping:\n  shop:\n    name: name\n",
    );
    write("strategies/model.py", SAME_NAME);
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&Manifest::load(dir.path()).unwrap()).unwrap();

    let error = engine
        .resolve(&ResolveOptions::with_strategy("model"))
        .unwrap_err();
    assert!(
        matches!(error, Error::NoUserCode(ref name) if name == "model"),
        "{error}"
    );
    assert!(engine.runs(&RunFilter::default()).unwrap().is_empty());

    // The store says what strategies the project has.
    let info = engine.info().unwrap();
    assert_eq!(info.entity_types[0].strategies.len(), 1);
    assert_eq!(info.entity_types[0].strategies[0].name, "model");
}
