//! What the engine checks of a strategy: what it writes, which file runs, and when.

use super::*;

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
            spool_testing::count(&things.engine, "SELECT count(*) FROM spool.judgement"),
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
                spool_testing::rows(&[("1", Some("anvil"), None), ("2", Some("anvil"), None)]),
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
    let kept = spool_testing::select(
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
