//! A reviewer's corrections, on the company benchmark: what `spool bench`
//! measures of a wrong merge and of its repair, and that the corrections are
//! the same with the same seed and can be rebuilt from the log.
//!
//! What else corrections do is tested where the code lives:
//!
//! - The in-memory state and the DuckDB store agree on all six operations and
//!   on the undo of each: `spool_core::conformance` and
//!   `crates/spool/src/store_property.rs`.
//! - Merge, undo, split, constraints, superseding rows and dismissed review
//!   items through the library: `crates/spool/tests/it/lifecycle.rs`.
//! - The same through the API: `lifecycle.rs` beside this file, and
//!   `packages/client/test` for the TypeScript client.

use std::path::{Path, PathBuf};

use spool::core::{IdentityStore, MemoryState, Op};
use spool::{Engine, Options};
use spool_bench::{BenchReport, Scenario, StepReport};

use crate::support::*;

fn corrections() -> PathBuf {
    benchmarks().join("identity/companies-small/corrections.toml")
}

/// Runs `spool bench` and returns what it printed and the results it wrote.
fn bench(scenario: &Path, seed: u64) -> (String, BenchReport) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("results.json");
    let printed = succeed(&[
        "bench",
        path(scenario),
        "--db",
        path(&dir.path().join("bench.duckdb")),
        "--out",
        path(&out),
        "--seed",
        &seed.to_string(),
    ]);
    let report = serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap();
    (printed, report)
}

fn step<'a>(report: &'a BenchReport, name: &str) -> &'a StepReport {
    report
        .steps
        .iter()
        .find(|step| step.name == name)
        .unwrap_or_else(|| panic!("no step `{name}`"))
}

// The corrections scenario reports ID stability, churn, split recovery,
// override persistence and order sensitivity.

#[test]
fn the_benchmark_measures_what_a_wrong_merge_costs_and_whether_its_repair_holds() {
    let (printed, report) = bench(&corrections(), 0);
    assert_eq!(report.scenario, "corrections");

    // Two wrong merges: two identities each hold two companies. Whoever kept
    // a merged ID is redirected, so no ID is lost.
    let merged = step(&report, "two wrong merges");
    assert_eq!(merged.metrics.false_merges, 2);
    assert_eq!(merged.id_stability.unchanged, 23);
    assert_eq!(merged.id_stability.following_redirects, Some(1.0));
    assert_eq!(merged.churn.identities_merged, 2);
    assert_eq!(merged.churn.records_reassigned, 3);

    // One is undone and one is split, and a missed match is merged by hand.
    // State is then exactly the truth.
    let repaired = step(&report, "the merges are put right");
    assert_eq!(
        (repaired.metrics.false_merges, repaired.metrics.false_splits),
        (0, 0)
    );
    assert_eq!(repaired.metrics.pairs.recall, Some(1.0));
    // The undo gave two records their identity back, the split gave one
    // a new identity, and the merge moved one. Only the last is a redirect.
    assert_eq!(repaired.id_stability.unchanged, 22);
    assert_eq!(repaired.id_stability.unchanged_following_redirects, 23);
    assert_eq!(repaired.churn.identities_created, 1);
    assert_eq!(repaired.churn.identities_merged, 1);
    assert_eq!(repaired.split_recovery.splits, 1);
    assert_eq!(repaired.override_persistence.constraints, 2);

    // The next batch keeps every ID, keeps the split apart and honours both
    // constraints. The record that fits both sides of the split is left for
    // review, as it is without the corrections.
    let second = step(&report, "batch two");
    assert_eq!(second.id_stability.strict, Some(1.0));
    assert_eq!(second.split_recovery.share, Some(1.0));
    assert_eq!(second.override_persistence.share, Some(1.0));
    assert_eq!(second.metrics.false_merges, 0);
    assert_eq!(second.metrics.unassigned, 1);
    assert_eq!(second.churn.new_records, 13);
    assert_eq!(second.churn.identities_created, 4);
    assert_eq!(second.churn.records_reassigned, 0);

    // A reviewer places it, and every record is where the truth says.
    let last = report.steps.last().unwrap();
    assert_eq!(last.metrics.unassigned, 0);
    assert_eq!(last.metrics.identities, last.metrics.true_entities);
    assert_eq!(
        (last.metrics.pairs.precision, last.metrics.pairs.recall),
        (Some(1.0), Some(1.0))
    );
    assert_eq!(last.split_recovery.share, Some(1.0));
    assert_eq!(last.override_persistence.share, Some(1.0));

    // The default policy never merges identities, so what it decides depends
    // on which batch came first. The benchmark says by how much.
    let order = report.order_sensitivity.as_ref().unwrap();
    assert_eq!((order.same_group, order.records), (32, 39));
    assert_eq!((order.pairs_in_both, order.pairs_in_either), (22, 27));

    for line in [
        "corrected merge `atlas`: changeset at 2",
        "corrected undo: changeset at 4",
        "churn         0 identities created, 2 merged, 3 records reassigned",
        "splits        1 of 1 still apart (1.0000)",
        "constraints   2 of 2 still honoured (1.0000)",
        "same group    32 of 39 records (0.8205)",
    ] {
        assert!(printed.contains(line), "no `{line}` in:\n{printed}");
    }
}

// Two runs with the same seed give the same report, corrections included.

#[test]
fn the_same_seed_gives_the_same_corrections_and_the_same_report() {
    let (_, first) = bench(&corrections(), 7);
    let (_, second) = bench(&corrections(), 7);
    assert_eq!(first, second);

    let engine = Engine::open_in_memory(Options::seeded(7)).unwrap();
    let in_process = Scenario::load(corrections())
        .unwrap()
        .run(&engine, |_| Ok(()))
        .unwrap();
    // Compared as the results file holds them: a ratio read back from JSON
    // can differ from the one that was written in its last digit.
    let written: BenchReport =
        serde_json::from_str(&serde_json::to_string(&in_process).unwrap()).unwrap();
    assert_eq!(written, first);
}

// Rebuilding the projection from the log reproduces the state after every
// kind of operation.

#[test]
fn rebuilding_the_projection_reproduces_the_state_the_corrections_left() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("spool.duckdb"), Options::seeded(0)).unwrap();
    Scenario::load(corrections())
        .unwrap()
        .run(&engine, |_| Ok(()))
        .unwrap();

    let log = engine.log().unwrap();
    let mut kinds: Vec<&str> = log
        .iter()
        .flat_map(|entry| &entry.changeset.ops)
        .map(Op::kind)
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(
        kinds,
        [
            "assign",
            "constrain",
            "create_identity",
            "merge",
            "reinstate"
        ]
    );

    let committed = engine.snapshot().unwrap();
    assert_eq!(committed.constraints.len(), 2);
    let replayed = MemoryState::replay(&log).unwrap();
    assert_eq!(replayed.snapshot().unwrap(), committed);

    engine.rebuild_projection().unwrap();
    assert_eq!(engine.snapshot().unwrap(), committed);
    engine.verify().unwrap();
}
