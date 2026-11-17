//! Splink as the second strategy of the person project.
//!
//! Every test here starts `uv`, which fetches Splink the first time, so they
//! are ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool --test splink -- --include-ignored
//! ```

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use common::{benchmarks, identity, members, select};
use spool::core::ReviewItem;
use spool::{BenchReport, Engine, Options, Scenario, UserCode};

/// Runs a scenario of the person project on a new store.
fn run(file: &str, strategy: Option<&str>, seed: u64) -> (Engine, BenchReport) {
    let mut scenario = Scenario::load(benchmarks().join("resolution/people").join(file)).unwrap();
    if let Some(strategy) = strategy {
        scenario = scenario.with_strategy(strategy);
    }
    let engine = Engine::open_in_memory(Options::seeded(seed)).unwrap();
    engine.set_user_code(UserCode {
        timeout: Duration::from_secs(300),
        ..UserCode::new(scenario.project_dir())
    });
    let report = scenario.run(&engine, |_| Ok(())).unwrap();
    engine.verify().unwrap();
    (engine, report)
}

fn hashes(engine: &Engine) -> Vec<String> {
    engine
        .log()
        .unwrap()
        .iter()
        .map(|entry| entry.content_hash.to_string())
        .collect()
}

/// The keys of the records that share an identity with `key`.
fn group(engine: &Engine, key: &str) -> BTreeSet<String> {
    members(engine, identity(engine, "sample", key).unwrap())
}

fn keys(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|key| format!("sample:{key}")).collect()
}

/// Every judgement Splink made is a `match` or an `unsure` with a
/// probability as its score, on the side of the threshold its verdict says,
/// and names the level of each comparison.
fn assert_judgements_are_splinks(engine: &Engine) {
    let judgements = select(
        engine,
        "SELECT j.verdict, j.score::VARCHAR, j.detail::VARCHAR
         FROM spool.judgement j JOIN spool.run r USING (run_id)
         WHERE r.strategy = 'splink'",
    );
    assert!(!judgements.is_empty());
    for judgement in &judgements {
        let verdict = judgement[0].as_deref().unwrap();
        let score: f64 = judgement[1].as_deref().unwrap().parse().unwrap();
        match verdict {
            "match" => assert!((0.95..=1.0).contains(&score), "{judgement:?}"),
            "unsure" => assert!((0.5..0.95).contains(&score), "{judgement:?}"),
            other => panic!("Splink says nothing is a `{other}`: {judgement:?}"),
        }
        let detail: serde_json::Value =
            serde_json::from_str(judgement[2].as_deref().unwrap()).unwrap();
        assert!(detail["match_weight"].is_number(), "{detail}");
        let levels = detail["levels"].as_object().unwrap();
        assert_eq!(
            levels.keys().map(String::as_str).collect::<Vec<_>>(),
            ["city", "dob", "email", "first_name", "surname"]
        );
    }
}

#[test]
#[ignore = "runs uv"]
fn splink_resolves_people_as_they_arrive_and_the_engine_decides_the_identities() {
    let (engine, report) = run("two-batches.toml", Some("splink"), 3);
    let [first, second] = &report.steps[..] else {
        panic!("two steps");
    };
    assert_eq!(first.strategy, "splink");
    assert_eq!(first.metrics.identities, 6);
    assert_eq!(
        (first.metrics.false_merges, first.metrics.false_splits),
        (0, 0)
    );

    // The second batch is scored against records that have an identity. The
    // record with the mistyped e-mail address and no city, which no rule of
    // the project matches, joins the two it belongs with.
    assert_eq!(group(&engine, "p-11"), keys(&["p-04", "p-05", "p-11"]));
    assert_eq!(group(&engine, "p-12"), keys(&["p-06", "p-07", "p-12"]));
    assert_eq!(group(&engine, "p-15"), keys(&["p-15", "p-16"]));
    assert_eq!(second.id_stability.unchanged, 10);
    assert_eq!(second.id_stability.records, 10);

    // Splink scores Wen Zhao's new record as a match with both Wen Zhao and
    // Lin Zhao, who share a surname, a city and most of an e-mail address.
    // Its clustering would make one person of the three. The engine does
    // not merge identities on a strategy's word: the record waits.
    assert_eq!(second.metrics.false_merges, 0);
    assert_eq!(second.metrics.unassigned, 1);
    assert_eq!(identity(&engine, "sample", "p-13"), None);
    let wen = identity(&engine, "sample", "p-08").unwrap();
    let lin = identity(&engine, "sample", "p-09").unwrap();
    assert_ne!(wen, lin);
    let [ReviewItem::PossibleMerge { identities, .. }] = &second.resolve.resolved[0].review[..]
    else {
        panic!("{:?}", second.resolve.resolved[0].review);
    };
    assert_eq!(
        identities.iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([&wen, &lin])
    );

    assert_judgements_are_splinks(&engine);

    // The model is a file, and nothing is trained during a run: the same
    // records give the same changesets.
    let (again, _) = run("two-batches.toml", Some("splink"), 3);
    assert_eq!(hashes(&again), hashes(&engine));
    assert_eq!(hashes(&engine).len(), 2);
}

#[test]
#[ignore = "runs uv"]
fn changing_to_splink_suggests_merges_and_moves_nothing() {
    let (rules, _) = run("two-batches.toml", None, 3);
    let (engine, report) = run("upgrade.toml", None, 3);
    let [_, second, upgrade] = &report.steps[..] else {
        panic!("three steps");
    };
    assert_eq!(second.strategy, "rules");
    assert_eq!(upgrade.strategy, "splink");

    // The rules left the record with the mistyped e-mail address on its own.
    assert_eq!(group(&engine, "p-11"), keys(&["p-11"]));
    assert_eq!(second.metrics.false_splits, 1);

    // Splink judged every record again, and committed nothing.
    assert_eq!(upgrade.head, second.head);
    assert_eq!(hashes(&engine), hashes(&rules));
    assert_eq!(upgrade.metrics, second.metrics);
    let resolved = &upgrade.resolve.resolved[0];
    assert_eq!(resolved.committed, None);
    assert!(resolved.judgements.match_ > 0);

    // What it would change is listed: the merge the rules missed, and the
    // merge of two different people that it would get wrong.
    let suggestions = resolved.suggestions.as_ref().unwrap();
    assert_eq!(suggestions.splits, []);
    let id = |key: &str| identity(&engine, "sample", key).unwrap();
    let pair = |a: &str, b: &str| {
        let mut pair = [id(a), id(b)];
        pair.sort();
        pair
    };
    let mut suggested: Vec<_> = suggestions
        .merges
        .iter()
        .map(|merge| merge.identities)
        .collect();
    suggested.sort();
    let mut expected = vec![pair("p-04", "p-11"), pair("p-08", "p-09")];
    expected.sort();
    assert_eq!(suggested, expected);

    let stability = upgrade.upgrade_stability.as_ref().unwrap();
    assert_eq!(stability.strategy, "splink");
    assert_eq!((stability.records, stability.retained), (16, 16));
    assert_eq!(stability.share, Some(1.0));
    assert_eq!(
        (stability.suggested_merges, stability.suggested_splits),
        (2, 0)
    );
    assert_eq!(stability.applied_changes, 0);
    assert_eq!(second.upgrade_stability, None);

    assert_judgements_are_splinks(&engine);
}

#[test]
#[ignore = "runs uv and downloads fake_1000"]
fn splink_finds_more_of_fake_1000_than_the_rules_and_merges_nobody_wrongly() {
    let (_, rules) = run("fake-1000.toml", None, 0);
    let (engine, splink) = run("fake-1000.toml", Some("splink"), 0);
    let (rules, splink) = (&rules.steps[0].metrics, &splink.steps[0].metrics);
    assert_eq!(splink.records, 1000);
    assert_eq!(splink.unassigned, 0);
    assert_eq!(splink.false_merges, 0);
    assert!(
        splink.pairs.recall.unwrap() > rules.pairs.recall.unwrap(),
        "splink {:?}, rules {:?}",
        splink.pairs,
        rules.pairs
    );
    assert!(splink.false_splits < rules.false_splits);
    assert_judgements_are_splinks(&engine);

    let (again, _) = run("fake-1000.toml", Some("splink"), 0);
    assert_eq!(hashes(&again), hashes(&engine));
}

#[test]
#[ignore = "runs uv and downloads fake_1000"]
fn changing_to_splink_on_fake_1000_keeps_every_identity() {
    let (_, report) = run("fake-1000-upgrade.toml", None, 0);
    let [rules, upgrade] = &report.steps[..] else {
        panic!("two steps");
    };
    assert_eq!(upgrade.head, rules.head);
    assert_eq!(upgrade.metrics, rules.metrics);
    let stability = upgrade.upgrade_stability.as_ref().unwrap();
    assert_eq!((stability.records, stability.retained), (1000, 1000));
    assert_eq!(stability.applied_changes, 0);
    assert_eq!(stability.suggested_splits, 0);
    // Each suggested merge is of two identities the rules kept apart.
    assert!(stability.suggested_merges > 0);
    assert!(stability.suggested_merges <= rules.metrics.false_splits + rules.metrics.false_merges);
}
