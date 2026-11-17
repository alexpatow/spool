//! Runs as task records, the read lane, and what a restart does to a run.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use common::*;
use spool::arrow::datatypes::SchemaRef;
use spool::arrow::error::ArrowError;
use spool::arrow::record_batch::{RecordBatch, RecordBatchReader};
use spool::core::{EntityType, Policy, RunId, Seq};
use spool::{
    Engine, EntityTypeSource, Error, IngestOptions, Options, ResolveOptions, RunFilter, RunKind,
    RunStatus, Scope, SqlFile, StrategyRef, watch_runs,
};

fn thing_scope() -> Scope {
    Scope::new(EntityType::new("thing").unwrap())
}

fn by_hand() -> StrategyRef {
    StrategyRef::external("by-hand", Some("1".to_owned()))
}

/// Rows that do something of the test's choosing before they are handed
/// over, which is while the ingest run is in progress.
struct Held<F: FnMut()> {
    rows: spool::Rows,
    before: F,
}

impl<F: FnMut()> Iterator for Held<F> {
    type Item = Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        (self.before)();
        self.rows.next()
    }
}

impl<F: FnMut()> RecordBatchReader for Held<F> {
    fn schema(&self) -> SchemaRef {
        self.rows.schema()
    }
}

#[test]
fn a_run_records_what_it_reported() {
    let engine = engine();
    let ingested = ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    let record = engine.run(ingested.run).unwrap();
    assert_eq!(record.kind, RunKind::Ingest);
    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(record.entity_type, Some(thing_scope().entity_type));
    assert_eq!(record.ingest, Some(ingested.clone()));
    assert_eq!(record.resolution, None);
    assert_eq!(record.error, None);
    // The seeded clock starts at the beginning of 2026 and steps a second.
    assert_eq!(record.started_at, "2026-01-01T00:00:01.000000Z");
    assert_eq!(
        record.finished_at.as_deref(),
        Some("2026-01-01T00:00:03.000000Z")
    );

    let report = engine.resolve(&ResolveOptions::default()).unwrap();
    let resolution = &report.resolved[0];
    let record = engine.run(resolution.match_run).unwrap();
    assert_eq!(record.kind, RunKind::Match);
    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(record.strategy, Some(StrategyRef::rules()));
    assert_eq!(record.resolution.as_ref(), Some(resolution));
    let record = engine.run(resolution.reconcile_run).unwrap();
    assert_eq!(record.kind, RunKind::Reconcile);
    assert_eq!((record.ingest, record.resolution), (None, None));

    // A run that fails says why.
    let no_key =
        spool::Rows::from_text(&["id", "where"], &[vec![None, Some(r#"{"page": 1}"#)]]).unwrap();
    let options = IngestOptions {
        key_column: Some("id".to_owned()),
        evidence_column: Some("where".to_owned()),
    };
    let error = engine
        .ingest("shop", "thing", no_key, &options)
        .unwrap_err();
    let failed = &engine
        .runs(&RunFilter {
            status: Some(RunStatus::Failed),
            ..RunFilter::default()
        })
        .unwrap()[0];
    assert_eq!(failed.kind, RunKind::Ingest);
    assert_eq!(failed.error.as_deref(), Some(error.to_string().as_str()));
    assert!(failed.finished_at.is_some());

    // Newest first, and filtered by kind and type.
    let all = engine.runs(&RunFilter::default()).unwrap();
    let ids: Vec<u64> = all.iter().map(|run| run.run.0).collect();
    assert_eq!(ids, [4, 3, 2, 1]);
    let matches = engine
        .runs(&RunFilter {
            kind: Some(RunKind::Match),
            entity_type: Some(thing_scope().entity_type),
            ..RunFilter::default()
        })
        .unwrap();
    assert_eq!(matches.len(), 1);
    let latest = engine
        .runs(&RunFilter {
            limit: 1,
            ..RunFilter::default()
        })
        .unwrap();
    assert_eq!(latest.len(), 1);

    let error = engine.run(RunId(99)).unwrap_err();
    assert!(matches!(error, Error::UnknownRun(RunId(99))), "{error}");
}

#[test]
fn a_run_is_announced_before_its_work_is_done_and_reads_do_not_wait_for_it() {
    let engine = Arc::new(engine());
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    engine.resolve(&ResolveOptions::default()).unwrap();

    let announced = Arc::new(Mutex::new(Vec::new()));
    let (release, held) = mpsc::channel::<()>();
    let (reached, waiting) = mpsc::channel::<()>();

    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            let announced = Arc::clone(&announced);
            let rows = Held {
                rows: rows(&[("2", Some("anvil"), None)]),
                before: move || {
                    // The ingest holds the engine's write lock from here on.
                    let _ = reached.send(());
                    let _ = held.recv();
                },
            };
            watch_runs(
                move |run| announced.lock().unwrap().push(run),
                || {
                    engine
                        .ingest("depot", "thing", rows, &IngestOptions::keyed_by("id"))
                        .unwrap()
                },
            )
        });

        // The ingest is in the middle of its work.
        waiting.recv().unwrap();
        let [run] = announced.lock().unwrap()[..] else {
            panic!("one run announced");
        };
        let record = engine.run(run).unwrap();
        assert_eq!(record.status, RunStatus::Running);
        assert_eq!(record.finished_at, None);
        // Everything that only reads answers from committed state.
        assert_eq!(engine.head().unwrap(), Seq(1));
        assert_eq!(engine.info().unwrap().entity_types[0].records, 1);
        assert_eq!(engine.snapshot().unwrap().identities.len(), 1);
        assert!(identity(&engine, "shop", "1").is_some());
        assert_eq!(engine.log().unwrap().len(), 1);

        release.send(()).unwrap();
        drop(release);
        let report = writer.join().unwrap();
        assert_eq!(report.run, run);
        assert_eq!(engine.run(run).unwrap().status, RunStatus::Succeeded);
        assert_eq!(engine.info().unwrap().entity_types[0].records, 2);
    });

    // Nothing is announced outside `watch_runs`.
    ingest(&engine, "kiosk", &[("3", Some("anvil"), None)]);
    assert_eq!(announced.lock().unwrap().len(), 1);
}

#[test]
fn an_open_run_survives_a_restart_and_a_running_one_is_failed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    let (open, interrupted) = {
        let engine = Engine::open(&path, Options::seeded(1)).unwrap();
        engine.apply(&things()).unwrap();
        ingest(
            &engine,
            "shop",
            &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
        );
        let open = engine.begin_run(&by_hand(), &thing_scope()).unwrap();

        // An ingest that never finishes, as far as the store can tell.
        let rows = Held {
            rows: rows(&[("3", Some("hammer"), None)]),
            before: || resume_unwind(Box::new("simulated crash")),
        };
        let crashed = catch_unwind(AssertUnwindSafe(|| {
            engine.ingest("depot", "thing", rows, &IngestOptions::keyed_by("id"))
        }));
        assert!(crashed.is_err());
        let interrupted = RunId(open.0 + 1);
        assert_eq!(engine.run(interrupted).unwrap().status, RunStatus::Running);
        (open, interrupted)
    };

    let engine = Engine::open(&path, Options::seeded(2)).unwrap();
    engine.adopt(&things()).unwrap();
    let record = engine.run(interrupted).unwrap();
    assert_eq!(record.status, RunStatus::Failed);
    assert!(record.error.unwrap().starts_with("interrupted"));
    assert!(record.finished_at.is_some());
    assert_eq!(
        count(
            &engine,
            "SELECT count(*) FROM spool.run WHERE status = 'running'"
        ),
        0
    );

    // The open run is where it was left, and can be finished.
    assert_eq!(engine.run(open).unwrap().status, RunStatus::Open);
    let pairs = engine.candidates(open).unwrap();
    let (left, right) = (
        cell(&pairs[0], 0, 0).unwrap(),
        cell(&pairs[0], 1, 0).unwrap(),
    );
    let judgements = spool::Rows::from_text(
        &["left_id", "right_id", "verdict"],
        &[vec![
            Some(left.as_str()),
            Some(right.as_str()),
            Some("match"),
        ]],
    )
    .unwrap();
    assert_eq!(engine.submit_judgements(open, judgements).unwrap(), 1);
    let draft = engine.reconcile(open, &Policy::default()).unwrap();
    engine.commit(draft.changeset).unwrap();
    assert_eq!(
        identity(&engine, "shop", "1"),
        identity(&engine, "shop", "2")
    );
    engine.verify().unwrap();
}

#[test]
fn a_cancelled_run_is_failed_and_its_records_are_still_new() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    let run = engine.begin_run(&by_hand(), &thing_scope()).unwrap();
    engine.cancel_run(run).unwrap();

    let record = engine.run(run).unwrap();
    assert_eq!(record.status, RunStatus::Failed);
    assert_eq!(record.error.as_deref(), Some("cancelled"));
    for error in [
        engine.cancel_run(run).unwrap_err(),
        engine
            .reconcile(run, &Policy::default())
            .map(|_| ())
            .unwrap_err(),
    ] {
        assert!(matches!(error, Error::Run { .. }), "{error}");
    }

    let report = engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(report.resolved[0].new_records, 1);
    // Only an open run can be cancelled.
    let error = engine.cancel_run(report.resolved[0].match_run).unwrap_err();
    assert!(matches!(error, Error::Run { .. }), "{error}");
}

#[test]
fn changing_a_declaration_fails_the_runs_that_wait_on_the_old_one() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("Anvil"), None)],
    );
    let run = engine.begin_run(&by_hand(), &thing_scope()).unwrap();
    assert!(
        engine
            .candidates(run)
            .unwrap()
            .iter()
            .all(|batch| batch.num_rows() == 0)
    );

    let folded = EntityTypeSource {
        features: SqlFile::new(
            "features",
            "SELECT record_id, lower(payload->>'name') AS name, payload->>'code' AS code
             FROM record",
        ),
        ..thing()
    };
    let report = engine.apply(&manifest_of(vec![folded])).unwrap();
    assert_eq!(report.rebuilt, [thing_scope().entity_type]);

    let record = engine.run(run).unwrap();
    assert_eq!(record.status, RunStatus::Failed);
    assert!(record.error.unwrap().contains("declaration"));

    // The next run sees the new features.
    let report = engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(report.resolved[0].candidates, 1);
    assert_eq!(
        identity(&engine, "shop", "1"),
        identity(&engine, "shop", "2")
    );
    engine.verify().unwrap();
}

#[test]
fn a_changed_declaration_that_breaks_its_contract_changes_nothing() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    let doubled = EntityTypeSource {
        features: SqlFile::new(
            "features",
            "SELECT o.record_id, o.payload->>'name' AS name, o.payload->>'code' AS code
             FROM record o, range(2)",
        ),
        ..thing()
    };
    let error = engine.apply(&manifest_of(vec![doubled])).unwrap_err();
    assert!(
        matches!(&error, Error::Apply(message) if message.contains("one row per record")),
        "{error}"
    );
    // The store is as it was, and so is the manifest the engine runs on.
    assert_eq!(
        engine.info().unwrap().entity_types[0].declaration_version,
        1
    );
    assert_eq!(engine.applied_manifest_hash().unwrap(), Some(things().hash));
    assert_eq!(engine.manifest().unwrap().hash, things().hash);
    engine.resolve(&ResolveOptions::default()).unwrap();
    engine.verify().unwrap();
}

#[test]
fn an_engine_adopts_only_the_manifest_its_store_was_applied_with() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    {
        let engine = Engine::open(&path, Options::seeded(1)).unwrap();
        let error = engine.adopt(&things()).unwrap_err();
        assert!(
            matches!(&error, Error::ManifestMismatch { applied: None, .. }),
            "{error}"
        );
        engine.apply(&things()).unwrap();
    }
    let engine = Engine::open(&path, Options::seeded(2)).unwrap();
    assert!(engine.manifest().is_none());
    let other = manifest_of(vec![EntityTypeSource {
        no_match: Vec::new(),
        ..thing()
    }]);
    let error = engine.adopt(&other).unwrap_err();
    assert!(
        matches!(&error, Error::ManifestMismatch { applied: Some(hash), .. } if *hash == things().hash),
        "{error}"
    );
    assert!(engine.manifest().is_none());

    engine.adopt(&things()).unwrap();
    assert_eq!(engine.manifest().unwrap().hash, things().hash);
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    engine.resolve(&ResolveOptions::default()).unwrap();
}
