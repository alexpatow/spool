//! User-code strategies: files in the project that the engine runs as child
//! processes.
//!
//! Almost every test here starts `uv`, which fetches a Python and `pyarrow`
//! the first time, or `bun`, so they are ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool --test usercode -- --include-ignored
//! ```

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{benchmarks, identity, load, members, record_id};
use spool::core::{Actor, EntityType, Op};
use spool::{
    Engine, Error, Manifest, Options, ResolveOptions, RunFilter, RunStatus, Scenario, UserCode,
};

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

#[test]
#[ignore = "runs uv"]
fn a_strategy_that_does_not_finish_is_stopped() {
    let things = Things::new();
    things.strategy(
        "model.py",
        r#"
import time
from spool import strategy

@strategy
def judge(candidates):
    time.sleep(600)
"#,
    );
    let engine = &things.engine;
    engine.set_user_code(UserCode {
        timeout: Duration::from_secs(5),
        ..engine.user_code().unwrap().as_ref().clone()
    });
    two_anvils_and_a_hammer(engine);

    let started = Instant::now();
    let message = strategy_error(things.resolve("model"));
    assert!(
        message.contains(
            "it was stopped after 5 seconds without finishing. That is the longest a strategy \
             or an extractor may run here, and `--task-timeout <seconds>` sets it, on \
             `spool dev`, `spool serve` or `spool bench`. Its task is kept in "
        ),
        "{message}"
    );
    assert!(started.elapsed() < Duration::from_secs(60));
    assert_eq!(engine.head().unwrap().0, 0);
    assert_eq!(things.tasks().len(), 1);

    // The engine works as before.
    things.strategy("model.py", SAME_NAME);
    assert_eq!(
        things.resolve("model").unwrap().resolved[0].identities_created,
        2
    );
}

#[test]
#[ignore = "runs uv"]
fn judgements_from_user_code_are_checked_like_a_callers() {
    let cases = [
        (
            r#"[{"left_id": pair["left_id"], "right_id": "00000000-0000-0000-0000-000000000000", "verdict": "match"} for pair in candidates.to_pylist()]"#,
            "it names a record that is not a known thing",
        ),
        (
            r#"[{"left_id": pair["left_id"], "right_id": pair["right_id"], "verdict": "probably"} for pair in candidates.to_pylist()]"#,
            "the verdict must be match, no_match or unsure",
        ),
        (
            r#"[{"left_id": pair["left_id"], "right_id": pair["right_id"], "verdict": "match", "detail": "not JSON"} for pair in candidates.to_pylist()]"#,
            "the detail is not valid JSON",
        ),
    ];
    for (judgements, problem) in cases {
        let things = Things::new();
        things.strategy(
            "model.py",
            &format!(
                "from spool import strategy\n\n@strategy\ndef judge(candidates):\n    return {judgements}\n"
            ),
        );
        two_anvils_and_a_hammer(&things.engine);

        let message = strategy_error(things.resolve("model"));
        assert!(message.contains("its judgements were refused"), "{message}");
        assert!(message.contains(problem), "{message}");
        // Refused whole: nothing is stored and nothing is committed.
        assert_eq!(
            common::count(&things.engine, "SELECT count(*) FROM spool.judgement"),
            0
        );
        assert_eq!(things.engine.head().unwrap().0, 0);
        let [task] = &things.tasks()[..] else {
            panic!("one task was kept");
        };
        assert!(task.join("judgements.parquet").is_file());
    }
}

#[test]
#[ignore = "runs uv"]
fn a_strategy_cannot_judge_two_records_that_are_not_new() {
    let things = Things::new();
    things.strategy("model.py", SAME_NAME);
    two_anvils_and_a_hammer(&things.engine);
    things.resolve("model").unwrap();

    // The second strategy pairs up everything it is shown, new or not.
    things.strategy(
        "everything.py",
        r#"
from spool import strategy

@strategy
def judge(features):
    ids = features.column("record_id").to_pylist()
    return [
        {"left_id": a, "right_id": b, "verdict": "match"}
        for a in ids for b in ids if a < b
    ]
"#,
    );
    load(&things.engine, "shop", &[("4", Some("tongs"), None)]);
    match things.resolve("everything") {
        Err(Error::Strategy { message, .. }) => {
            assert!(
                message.contains("neither record is new to this run"),
                "{message}"
            );
        }
        other => panic!("expected the judgements to be refused, got {other:?}"),
    }
    assert_eq!(identity(&things.engine, "shop", "4"), None);
}

#[test]
#[ignore = "runs uv"]
fn a_strategy_that_writes_nothing_fails() {
    let things = Things::new();
    // It never calls the helper, so it leaves no judgements.
    things.strategy("model.py", "print('nothing to see')\n");
    two_anvils_and_a_hammer(&things.engine);

    let message = strategy_error(things.resolve("model"));
    assert!(
        message.contains("it finished without writing judgements.parquet"),
        "{message}"
    );
    let [task] = &things.tasks()[..] else {
        panic!("one task was kept");
    };
    assert_eq!(
        fs::read_to_string(task.join("stdout.log")).unwrap(),
        "nothing to see\n"
    );

    // A strategy that has nothing to say returns nothing, which is not the
    // same thing: every record gets an identity of its own.
    things.strategy(
        "model.py",
        "from spool import strategy\n\n@strategy\ndef judge(candidates):\n    return None\n",
    );
    let report = things.resolve("model").unwrap();
    assert_eq!(report.resolved[0].judgements.match_, 0);
    assert_eq!(report.resolved[0].identities_created, 3);
}

#[test]
#[ignore = "runs uv"]
fn only_the_file_the_manifest_was_compiled_from_runs() {
    let things = Things::new();
    things.strategy("model.py", SAME_NAME);
    things.write("strategies/model/threshold.txt", "0.9");
    things
        .engine
        .apply(&Manifest::load(things.project()).unwrap())
        .unwrap();
    two_anvils_and_a_hammer(&things.engine);
    let runs = || things.engine.runs(&RunFilter::default()).unwrap().len();
    let before = runs();

    // The file changes on disk and the project is not built again.
    things.write(
        "strategies/model.py",
        "import os\nos.system('touch /tmp/spool-should-not-run')\n",
    );
    let message = strategy_error(things.resolve("model"));
    assert!(
        message.contains("has changed since the manifest was compiled"),
        "{message}"
    );
    // So does a file that belongs to it.
    things.write("strategies/model.py", SAME_NAME);
    things.write("strategies/model/threshold.txt", "0.1");
    let message = strategy_error(things.resolve("model"));
    assert!(message.contains("has changed"), "{message}");
    // And a file that is gone.
    fs::remove_file(things.project().join("entities/thing/strategies/model.py")).unwrap();
    let message = strategy_error(things.resolve("model"));
    assert!(
        message.contains("is not in the project directory"),
        "{message}"
    );

    // Nothing was started.
    assert_eq!(runs(), before);
    assert!(things.tasks().is_empty());
}

#[test]
#[ignore = "runs uv"]
fn a_strategy_that_changes_while_another_runs_is_not_started() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let work = dir.path().join("work");
    let write = |file: &str, contents: &str| {
        let path = project.join("entities").join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    };
    // Two entity types, resolved in this order. The first one's strategy
    // rewrites the second one's while it runs, which is what an edit made
    // during a long run looks like to the engine.
    for entity_type in ["first", "second"] {
        write(
            &format!("../sources/{entity_type}.yaml"),
            "key: id\ncolumns: [id, name, code]\n",
        );
        write(
            &format!("{entity_type}/entity.yaml"),
            &format!("features: [name]\nmapping:\n  {entity_type}:\n    name: name\n"),
        );
        write(
            &format!("{entity_type}/rules/block/same_name.sql"),
            "l.name = r.name",
        );
    }
    write(
        "first/strategies/model.py",
        r#"
from spool import strategy

@strategy
def judge(candidates):
    with open("entities/second/strategies/model.py", "w") as file:
        file.write("open('second-ran', 'w').close()\n")
    return []
"#,
    );
    write("second/strategies/model.py", SAME_NAME);

    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.set_user_code(user_code(&project, &work));
    engine.apply(&Manifest::load(&project).unwrap()).unwrap();
    for entity_type in ["first", "second"] {
        engine
            .load(
                entity_type,
                entity_type,
                common::rows(&[("1", Some("anvil"), None), ("2", Some("anvil"), None)]),
            )
            .unwrap();
    }

    let error = engine
        .resolve(&ResolveOptions::with_strategy("model"))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("has changed since the manifest was compiled"),
        "{error}"
    );
    // The file that changed was not run.
    assert!(!project.join("second-ran").exists());

    // The first type was resolved, and the second one's run failed with
    // nothing committed for it.
    let runs = engine.runs(&RunFilter::default()).unwrap();
    let of = |entity_type: &str| {
        runs.iter()
            .find(|run| {
                run.strategy.is_some()
                    && run.entity_type.as_ref().map(EntityType::as_str) == Some(entity_type)
            })
            .unwrap_or_else(|| panic!("a match run for `{entity_type}`"))
    };
    assert_eq!(of("first").status, RunStatus::Succeeded);
    assert_eq!(of("second").status, RunStatus::Failed);
    assert!(
        of("second")
            .error
            .as_deref()
            .is_some_and(|error| error.contains("has changed")),
        "{:?}",
        of("second").error
    );
    assert_eq!(engine.log().unwrap().len(), 1);
    // Nothing ran for the second task, so there is nothing to keep of it.
    let kept: Vec<PathBuf> = fs::read_dir(&work)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("run-"))
        })
        .collect();
    assert!(kept.is_empty(), "{kept:?}");
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn a_strategy_that_finds_its_own_pairs_is_given_every_record() {
    let things = Things::new();
    // No block rules: the engine finds no candidates for this type.
    fs::remove_dir_all(things.project().join("entities/thing/rules")).unwrap();
    let seen = things.dir.path().join("seen.json");
    things.strategy(
        "model.py",
        &format!(
            r#"
import json
from spool import strategy

@strategy
def judge(features, candidates, task):
    rows = features.to_pylist()
    with open({seen:?}, "a") as out:
        out.write(json.dumps({{"rows": rows, "candidates": candidates.num_rows,
                              "rematch": task.rematch, "type": task.entity_type}}) + "\n")
    new = [row for row in rows if row["is_new"]]
    return [
        {{"left_id": a["record_id"], "right_id": b["record_id"],
          "verdict": "match", "score": 1.0, "detail": {{"name": a["name"]}}}}
        for a in new for b in rows
        if a["record_id"] != b["record_id"] and a["name"] == b["name"]
        and (not b["is_new"] or a["record_id"] < b["record_id"])
    ]
"#
        ),
    );
    let engine = &things.engine;
    two_anvils_and_a_hammer(engine);
    things.resolve("model").unwrap();
    let anvils = identity(engine, "shop", "1").unwrap();
    assert_eq!(identity(engine, "shop", "2"), Some(anvils));

    load(
        engine,
        "depot",
        &[("9", Some("anvil"), None), ("8", Some("tongs"), None)],
    );
    let report = things.resolve("model").unwrap();
    assert_eq!(report.resolved[0].candidates, 0);
    assert_eq!(report.resolved[0].judgements.match_, 2);
    assert_eq!(identity(engine, "depot", "9"), Some(anvils));

    let seen: Vec<serde_json::Value> = fs::read_to_string(&seen)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let [first, second] = &seen[..] else {
        panic!("the strategy ran twice");
    };
    assert_eq!(first["type"], "thing");
    assert_eq!(first["rematch"], false);
    assert_eq!(first["candidates"], 0);
    // First everything is new and nothing has an identity.
    let rows = first["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .all(|row| row["is_new"] == true && row["identity_id"].is_null())
    );
    // Then the first batch is known, with the identities it was given.
    let rows = second["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 5);
    let row = |source: &str, key: &str| {
        let id = record_id(engine, source, key).to_string();
        rows.iter()
            .find(|row| row["record_id"] == id.as_str())
            .unwrap()
            .clone()
    };
    assert_eq!(row("shop", "1")["is_new"], false);
    assert_eq!(row("shop", "1")["identity_id"], anvils.to_string());
    assert_eq!(row("shop", "1")["name"], "anvil");
    assert_eq!(row("shop", "1")["code"], "K1");
    assert_eq!(row("depot", "9")["is_new"], true);
    assert!(row("depot", "9")["identity_id"].is_null());
    assert!(row("depot", "9")["code"].is_null());

    // The score and the detail are kept with the judgement.
    let kept = common::select(
        engine,
        "SELECT score::VARCHAR, detail::VARCHAR FROM spool.judgement ORDER BY run_id DESC LIMIT 1",
    );
    assert_eq!(kept[0][0].as_deref(), Some("1.0"));
    assert_eq!(kept[0][1].as_deref(), Some(r#"{"name": "anvil"}"#));
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn the_engine_is_free_while_a_strategy_runs() {
    let things = Things::new();
    let go = things.dir.path().join("go");
    let started = things.dir.path().join("started");
    // The strategy waits until the test lets it finish.
    things.strategy(
        "model.py",
        &format!(
            r#"
import os, time
from spool import strategy

@strategy
def judge(candidates):
    open({started:?}, "w").close()
    while not os.path.exists({go:?}):
        time.sleep(0.02)
    return [
        {{"left_id": pair["left_id"], "right_id": pair["right_id"], "verdict": "match"}}
        for pair in candidates.to_pylist()
        if pair["left_name"] == pair["right_name"]
    ]
"#
        ),
    );
    let engine = Arc::clone(&things.engine);
    load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", Some("anvil"), None),
            ("4", Some("hammer"), None),
        ],
    );

    let resolving = {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || engine.resolve(&ResolveOptions::with_strategy("model")))
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the strategy never started");
        assert!(!resolving.is_finished(), "{:?}", resolving.join());
        std::thread::sleep(Duration::from_millis(20));
    }

    // Reads are answered: the run is there to see, at work.
    let runs = engine
        .runs(&RunFilter {
            status: Some(RunStatus::Running),
            ..RunFilter::default()
        })
        .unwrap();
    let [run] = &runs[..] else {
        panic!("one run is at work");
    };
    assert_eq!(run.strategy.as_ref().unwrap().name, "model");

    // Writes go through. A reviewer gives one of the anvils an identity of
    // its own, and another batch arrives.
    let thing = EntityType::new("thing").unwrap();
    let own = engine.mint_ids(1, 0).identities[0];
    let third = record_id(&engine, "shop", "3");
    engine
        .author(
            Actor::User("reviewer".to_owned()),
            None,
            vec![
                Op::CreateIdentity {
                    identity: own,
                    entity_type: thing,
                },
                Op::Assign {
                    record: third,
                    from: None,
                    to: Some(own),
                },
            ],
        )
        .unwrap();
    load(&engine, "depot", &[("9", Some("hammer"), None)]);
    assert!(!resolving.is_finished());

    fs::write(&go, "").unwrap();
    let report = resolving.join().unwrap().unwrap();

    // The run reconciled against the state it found when the strategy was
    // done. The anvil the reviewer placed stays where it was put, and the
    // other two join it, since the strategy matched them to it.
    assert_eq!(engine.identity_of(third).unwrap(), Some(own));
    assert_eq!(
        members(&engine, own),
        [
            "shop:1".to_owned(),
            "shop:2".to_owned(),
            "shop:3".to_owned()
        ]
        .into()
    );
    assert_eq!(report.resolved[0].new_records, 3);
    // The batch that arrived meanwhile was not the run's, and is the next
    // run's.
    assert_eq!(identity(&engine, "depot", "9"), None);
    let report = engine
        .resolve(&ResolveOptions::with_strategy("model"))
        .unwrap();
    assert_eq!(report.resolved[0].new_records, 1);
    assert_eq!(
        identity(&engine, "depot", "9"),
        identity(&engine, "shop", "4")
    );
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs bun"]
fn a_typescript_strategy_with_the_rules_commits_what_the_rules_commit() {
    let scenario = Scenario::load(benchmarks().join("identity/companies-small")).unwrap();
    let work = tempfile::tempdir().unwrap();

    let with_rules = Engine::open_in_memory(Options::seeded(7)).unwrap();
    let rules = scenario.run(&with_rules, |_| Ok(())).unwrap();

    let with_typescript = Engine::open_in_memory(Options::seeded(7)).unwrap();
    with_typescript.set_user_code(user_code(&scenario.project_dir(), work.path()));
    let typescript = scenario
        .with_strategy("in_typescript")
        .run(&with_typescript, |_| Ok(()))
        .unwrap();

    assert_eq!(hashes(&with_typescript), hashes(&with_rules));
    assert!(!hashes(&with_rules).is_empty());
    for (typescript, rules) in typescript.steps.iter().zip(&rules.steps) {
        assert_eq!(typescript.metrics, rules.metrics);
        assert_eq!(
            typescript.resolve.resolved[0].judgements,
            rules.resolve.resolved[0].judgements
        );
    }
}

#[test]
#[ignore = "runs bun"]
fn a_typescript_strategy_is_given_what_a_python_one_is() {
    let things = Things::new();
    let seen = things.dir.path().join("seen.json");
    things.strategy(
        "model.ts",
        &format!(
            r#"
import {{ appendFileSync }} from "node:fs";
import {{ strategy }} from "spool-strategy";

await strategy(async (task) => {{
  const rows = await task.features();
  const pairs = await task.candidates();
  appendFileSync({seen:?}, JSON.stringify({{
    rows, pairs: pairs.length, rematch: task.rematch, type: task.entityType, strategy: task.strategy,
  }}) + "\n");
  return pairs
    .filter((pair) => pair.left_name !== null && pair.left_name === pair.right_name)
    .map((pair) => ({{
      left_id: pair.left_id, right_id: pair.right_id, verdict: "match" as const,
      score: 0.5, detail: {{ name: pair.left_name }},
    }}));
}});
"#
        ),
    );
    let engine = &things.engine;
    two_anvils_and_a_hammer(engine);
    let report = things.resolve("model").unwrap();
    assert_eq!(report.resolved[0].judgements.match_, 1);
    let anvils = identity(engine, "shop", "1").unwrap();
    assert_eq!(identity(engine, "shop", "2"), Some(anvils));
    assert!(things.tasks().is_empty(), "{:?}", things.tasks());

    load(engine, "depot", &[("9", Some("anvil"), None)]);
    things.resolve("model").unwrap();
    assert_eq!(identity(engine, "depot", "9"), Some(anvils));

    let seen: Vec<serde_json::Value> = fs::read_to_string(&seen)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let [first, second] = &seen[..] else {
        panic!("the strategy ran twice");
    };
    assert_eq!(first["type"], "thing");
    assert_eq!(first["strategy"], "model");
    assert_eq!(first["rematch"], false);
    assert_eq!(first["pairs"], 1);
    assert_eq!(second["pairs"], 2);
    let rows = second["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    let row = |source: &str, key: &str| {
        let id = record_id(engine, source, key).to_string();
        rows.iter()
            .find(|row| row["record_id"] == id.as_str())
            .unwrap()
            .clone()
    };
    assert_eq!(row("shop", "1")["is_new"], false);
    assert_eq!(row("shop", "1")["identity_id"], anvils.to_string());
    assert_eq!(row("shop", "1")["code"], "K1");
    assert_eq!(row("depot", "9")["is_new"], true);
    assert!(row("depot", "9")["identity_id"].is_null());
    assert!(row("depot", "9")["code"].is_null());

    let kept = common::select(
        engine,
        "SELECT score::VARCHAR, detail::VARCHAR FROM spool.judgement ORDER BY run_id LIMIT 1",
    );
    assert_eq!(kept[0][0].as_deref(), Some("0.5"));
    assert_eq!(kept[0][1].as_deref(), Some(r#"{"name":"anvil"}"#));
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs bun"]
fn a_typescript_strategy_that_throws_fails_its_run() {
    let things = Things::new();
    things.strategy(
        "model.ts",
        r#"
import { strategy } from "spool-strategy";

// Not awaited: what the function throws still ends the process.
strategy(async (task) => {
  throw new Error(`cannot judge ${(await task.candidates()).length} pairs`);
});
"#,
    );
    let engine = &things.engine;
    two_anvils_and_a_hammer(engine);

    let message = strategy_error(things.resolve("model"));
    assert!(message.contains("it failed with exit status"), "{message}");
    assert!(message.contains("cannot judge 1 pairs"), "{message}");
    assert_eq!(engine.head().unwrap().0, 0);
    assert_eq!(things.tasks().len(), 1);

    // Judgements that are not judgements are refused by the helper, in the
    // strategy's own words.
    things.strategy(
        "model.ts",
        r#"
import { strategy } from "spool-strategy";

await strategy(async (task) =>
  (await task.candidates()).map((pair) => ({ left: pair.left_id, right_id: pair.right_id, verdict: "match" })) as never,
);
"#,
    );
    let message = strategy_error(things.resolve("model"));
    assert!(
        message.contains("row 0 has no `left_id` and also `left`"),
        "{message}"
    );
    assert_eq!(engine.head().unwrap().0, 0);
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
#[ignore = "needs bun, and the network the first time"]
fn the_typescript_helper_the_engine_carries_is_built_from_its_source() {
    bun(&["install", "--frozen-lockfile"]);
    // The file in this crate is what the package builds, the package and the
    // strategies of the benchmarks compile, and the helper passes its tests.
    bun(&["run", "check-build"]);
    bun(&["run", "typecheck"]);
    bun(&["test"]);
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
