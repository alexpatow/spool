//! A `resolve` that is interrupted loses nothing: the next one does the work.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use common::*;
use spool::core::{Clock, ReviewItem, SeededIds, StepClock};
use spool::{Engine, Options, ResolveOptions};

/// A clock that stops the process, as far as the engine can tell, at a chosen
/// reading. The engine reads the clock at each step of a run, so counting
/// readings walks through the places where a run can be cut short.
struct CrashClock {
    inner: StepClock,
    /// Readings left before the crash. Negative means never.
    remaining: Arc<AtomicI64>,
}

impl Clock for CrashClock {
    fn now_micros(&mut self) -> i64 {
        if self.remaining.fetch_sub(1, Ordering::SeqCst) == 0 {
            // Unwinds without running the panic hook, so the test stays quiet.
            resume_unwind(Box::new("simulated crash"));
        }
        self.inner.now_micros()
    }
}

fn open(path: &Path, seed: u64, remaining: &Arc<AtomicI64>) -> Engine {
    let engine = Engine::open(
        path,
        Options {
            ids: Box::new(SeededIds::new(seed)),
            clock: Box::new(CrashClock {
                inner: StepClock::new(1_767_225_600_000_000, 1_000_000),
                remaining: Arc::clone(remaining),
            }),
        },
    )
    .unwrap();
    engine.apply(&things()).unwrap();
    engine
}

/// Batch one makes four identities. Batch two has a record for each of the
/// first two, a new thing, and one that matches the other two identities and
/// goes to review.
fn first_batch(engine: &Engine) {
    ingest(
        engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", None, Some("K7")),
            ("3", Some("hammer"), None),
            ("4", None, Some("H1")),
        ],
    );
    engine.resolve(&ResolveOptions::default()).unwrap();
}

fn second_batch(engine: &Engine) {
    ingest(
        engine,
        "depot",
        &[
            ("1", Some("anvil"), None),
            ("2", None, Some("K7")),
            ("3", Some("chisel"), None),
            ("9", Some("hammer"), Some("H1")),
        ],
    );
}

/// Which records share an identity, which have none, and which are under
/// review. Identity IDs are left out: an interrupted run may have minted
/// some that were never committed.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    groups: BTreeSet<BTreeSet<String>>,
    unassigned: BTreeSet<String>,
    under_review: BTreeSet<String>,
}

fn outcome(engine: &Engine) -> Outcome {
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut unassigned = BTreeSet::new();
    for row in select(
        engine,
        "SELECT o.source_id || ':' || o.source_key, m.identity_id::VARCHAR
         FROM spool.record o
         LEFT JOIN spool.membership_current m ON m.record_id = o.record_id",
    ) {
        let record = row[0].clone().unwrap();
        match &row[1] {
            Some(identity) => {
                groups.entry(identity.clone()).or_default().insert(record);
            }
            None => {
                unassigned.insert(record);
            }
        }
    }
    let under_review = select(
        engine,
        "SELECT DISTINCT o.source_id || ':' || o.source_key
         FROM (SELECT unnest(record_ids) AS record_id FROM spool.review_open) r
         JOIN spool.record o ON o.record_id = r.record_id",
    )
    .into_iter()
    .map(|row| row[0].clone().unwrap())
    .collect();
    Outcome {
        groups: groups.into_values().collect(),
        unassigned,
        under_review,
    }
}

#[test]
fn a_resolve_cut_short_at_any_step_is_completed_by_the_next_one() {
    let never = Arc::new(AtomicI64::new(-1));
    let expected = {
        let dir = tempfile::tempdir().unwrap();
        let engine = open(&dir.path().join("spool.duckdb"), 1, &never);
        first_batch(&engine);
        second_batch(&engine);
        engine.resolve(&ResolveOptions::default()).unwrap();
        outcome(&engine)
    };
    assert_eq!(expected.groups.len(), 5);
    assert_eq!(expected.unassigned, ["depot:9".to_owned()].into());
    assert_eq!(expected.under_review, expected.unassigned);

    let mut cut_short = 0;
    for reading in 0.. {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spool.duckdb");
        let remaining = Arc::new(AtomicI64::new(-1));
        let engine = open(&path, 1, &remaining);
        first_batch(&engine);
        second_batch(&engine);

        // The resolve of batch two stops at its `reading`-th clock reading.
        remaining.store(reading, Ordering::SeqCst);
        let interrupted = catch_unwind(AssertUnwindSafe(|| {
            engine.resolve(&ResolveOptions::default())
        }));
        remaining.store(-1, Ordering::SeqCst);
        drop(engine);
        match interrupted {
            Err(_) => cut_short += 1,
            // The run finished before the reading came up: every step has
            // been tried.
            Ok(report) => {
                report.unwrap();
                break;
            }
        }

        // The process starts again and resolves, as it would have anyway.
        let engine = open(&path, 2 + reading as u64, &never);
        let report = engine.resolve(&ResolveOptions::default()).unwrap();
        // Its report counts what it examined, not what was already placed.
        for resolved in &report.resolved {
            let reviewed: usize = resolved
                .review
                .iter()
                .map(|item| match item {
                    ReviewItem::PossibleMerge { records, .. }
                    | ReviewItem::Conflict { records, .. } => records.len(),
                })
                .sum();
            assert_eq!(resolved.records_unassigned as usize, reviewed);
            assert_eq!(
                resolved.new_records,
                resolved.records_assigned + resolved.records_unassigned
            );
        }
        assert_eq!(
            outcome(&engine),
            expected,
            "after a resolve cut short at clock reading {reading}"
        );
        engine.verify().unwrap();

        // And once more changes nothing.
        let head = engine.head().unwrap();
        engine.resolve(&ResolveOptions::default()).unwrap();
        assert_eq!(engine.head().unwrap(), head);
        assert_eq!(outcome(&engine), expected);
    }
    // Starting the match run, starting and finishing the reconcile run,
    // committing, and finishing the match run.
    assert_eq!(cut_short, 5);
}
