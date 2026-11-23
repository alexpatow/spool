//! Strategies written in TypeScript.

use super::*;

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

    let kept = spool_testing::select(
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
