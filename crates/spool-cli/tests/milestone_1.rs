//! Milestone 1's exit criteria, one test each, in the order the architecture
//! proposal lists them. Three of them are tested where the code lives:
//!
//! - "A resolve that is cut short at any step is completed by the next one":
//!   `interrupted.rs`.
//! - "The in-memory state and the DuckDB store pass the same conformance
//!   suite": `spool_core::conformance` runs in `spool-core/tests/conformance.rs`
//!   and in `src/store.rs`, and `src/store_property.rs` compares the two
//!   stores on random changesets.
//! - "A changeset with a stale precondition is rejected and names the
//!   conflicting operation": the conformance suite, and
//!   `a_stale_draft_is_rejected_and_names_the_operation` in `engine.rs`.

#[path = "../../spool/tests/common/mod.rs"]
mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use spool::core::{IdentityStore, MemoryState, ReviewItem, Seq, Snapshot};
use spool::{BenchReport, Engine, Manifest, Options, RecordFilter, ResolveOptions, Rows, Scenario};

fn companies() -> PathBuf {
    benchmarks().join("identity/companies-small")
}

fn people() -> PathBuf {
    benchmarks().join("resolution/people")
}

/// Runs the company scenario in process and returns its report with the
/// state each step left.
fn run_companies(engine: &Engine) -> (BenchReport, Vec<Snapshot>) {
    let mut snapshots = Vec::new();
    let report = Scenario::load(companies())
        .unwrap()
        .run(engine, |_| {
            snapshots.push(engine.snapshot()?);
            Ok(())
        })
        .unwrap();
    (report, snapshots)
}

fn company_engine(seed: u64) -> Engine {
    Engine::open_in_memory(Options::seeded(seed)).unwrap()
}

/// Runs `spool bench` and returns the results it wrote.
fn bench(scenario: &Path, seed: u64) -> BenchReport {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("results.json");
    let output = Command::new(env!("CARGO_BIN_EXE_spool"))
        .arg("bench")
        .arg(scenario)
        .arg("--db")
        .arg(dir.path().join("bench.duckdb"))
        .arg("--out")
        .arg(&out)
        .args(["--seed", &seed.to_string()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "spool bench failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("identities ("), "{stdout}");
    assert!(stdout.contains("false merges"), "{stdout}");
    serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap()
}

// The `fake_1000` person data runs through the same binary as the company
// fixture, with only the project directory differing.

#[test]
fn person_records_run_through_the_same_binary_as_the_company_fixture() {
    let companies = bench(&companies(), 0);
    assert_eq!(companies.scenario, "companies-small");
    assert_eq!(companies.steps.len(), 2);
    assert_eq!(
        companies.steps[0].resolve.resolved[0].entity_type.as_str(),
        "company"
    );

    // The same columns as `fake_1000`, in a sample small enough to commit.
    let sample = bench(&people().join("sample.toml"), 0);
    assert_eq!(sample.scenario, "sample");
    let [step] = &sample.steps[..] else {
        panic!("one step");
    };
    assert_eq!(step.resolve.resolved[0].entity_type.as_str(), "person");
    assert_eq!(step.metrics.records, 10);
    assert_eq!(step.metrics.identities, 6);
    assert_eq!(
        (step.metrics.false_merges, step.metrics.false_splits),
        (0, 0)
    );
    assert_ne!(sample.manifest_hash, companies.manifest_hash);
}

/// Downloads the dataset on first use, so it needs the network and `curl`:
/// `cargo test -p spool-cli --test milestone_1 -- --ignored`
#[test]
#[ignore = "downloads fake_1000"]
fn fake_1000_runs_through_the_same_binary_as_the_company_fixture() {
    let report = bench(&people().join("fake-1000.toml"), 0);
    let [step] = &report.steps[..] else {
        panic!("one step");
    };
    assert_eq!(step.ingest[0].inserted, 1000);
    assert_eq!(step.resolve.resolved[0].entity_type.as_str(), "person");
    assert_eq!(step.metrics.records, 1000);
    assert_eq!(step.metrics.true_entities, 251);
    assert_eq!(step.metrics.false_merges, 0);
    assert!(
        step.metrics.pairs.recall.unwrap() > 0.6,
        "{:?}",
        step.metrics
    );
    assert_eq!(step.metrics.unassigned, 0);
}

// Re-sent rows create no records and no operations.

#[test]
fn resent_rows_create_no_records_and_no_operations() {
    let engine = company_engine(0);
    let (report, _) = run_companies(&engine);

    // Batch two re-sends two CRM rows and two registry rows unchanged.
    let second = &report.steps[1];
    let resent: Vec<(u64, u64, u64)> = second
        .ingest
        .iter()
        .map(|ingest| (ingest.received, ingest.inserted, ingest.duplicates))
        .collect();
    assert_eq!(resent, [(11, 9, 2), (6, 4, 2)]);
    let resolved = &second.resolve.resolved[0];
    assert_eq!(resolved.new_records, 13);
    let operations = engine.log().unwrap()[1].changeset.ops.len() as u64;
    assert_eq!(
        operations,
        resolved.identities_created + resolved.records_assigned
    );
    assert_eq!(resolved.records_assigned, 12);

    // Sending again what the sources hold now changes nothing at all. That
    // is every file but the first of the CRM, which has a row as it was
    // before batch two changed it.
    let records = count(&engine, "SELECT count(*) FROM spool.record");
    let head = engine.head().unwrap();
    for (file, source) in [
        ("batch-1-registry.csv", "registry"),
        ("batch-2-crm.csv", "crm"),
        ("batch-2-registry.csv", "registry"),
    ] {
        let rows = Rows::from_csv(companies().join(file)).unwrap();
        let again = engine.load(source, "company", rows).unwrap();
        assert_eq!(again.inserted, 0, "{file}");
        assert_eq!(again.duplicates, again.received, "{file}");
    }
    // The one record that was left for review is looked at again, with
    // the same outcome. Nothing else is examined, and nothing is committed.
    let report = engine.resolve(&ResolveOptions::default()).unwrap();
    let [again] = &report.resolved[..] else {
        panic!("one entity type had something under review: {report:?}");
    };
    assert_eq!((again.new_records, again.reexamined), (1, 1));
    assert_eq!((again.identities_created, again.records_assigned), (0, 0));
    assert_eq!(again.committed, None);
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.review_item"), 1);
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.record"), records);
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(engine.log().unwrap().len(), 2);
    assert_eq!(head, Seq(2));

    // The first file of the CRM says that one row is again what it was: a
    // source that sends a row as it was before is taken at its word. The
    // row has a version more, in the identity it was in, and the other rows
    // of the file are known.
    let kestrel = engine
        .records(&RecordFilter::keyed("crm", "c-104"))
        .unwrap();
    let rows = Rows::from_csv(companies().join("batch-1-crm.csv")).unwrap();
    let back = engine.load("crm", "company", rows).unwrap();
    assert_eq!((back.inserted, back.superseded), (1, 1));
    assert_eq!(back.duplicates, back.received - 1);
    let report = engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(report.resolved[0].identities_created, 0);
    let versions = engine
        .records(&RecordFilter::keyed("crm", "c-104"))
        .unwrap();
    assert_eq!(versions.len(), kestrel.len() + 1);
    assert!(
        versions
            .iter()
            .all(|version| version.identity == kestrel[0].identity)
    );
    engine.verify().unwrap();
}

// Every identity ID from batch one still exists after batch two, with the
// same members plus any additions.

#[test]
fn every_identity_from_batch_one_keeps_its_id_and_members_after_batch_two() {
    let engine = company_engine(0);
    let (report, snapshots) = run_companies(&engine);
    let [first, second] = &snapshots[..] else {
        panic!("two steps");
    };

    assert_eq!(first.identities.len(), 14);
    for identity in &first.identities {
        assert!(second.identities.contains(identity), "{identity:?} is gone");
    }
    for membership in &first.memberships {
        assert_eq!(membership.to, None);
        assert!(
            second.memberships.contains(membership),
            "{membership:?} changed"
        );
    }
    // Some of them gained members, and nothing that was added closes an interval.
    let grew = second
        .memberships
        .iter()
        .filter(|membership| {
            membership.from == Seq(2)
                && first
                    .identities
                    .iter()
                    .any(|identity| identity.id == membership.identity)
        })
        .count();
    assert_eq!(grew, 5);
    assert!(
        second
            .memberships
            .iter()
            .all(|membership| membership.to.is_none())
    );

    let stability = &report.steps[1].id_stability;
    assert_eq!((stability.records, stability.unchanged), (26, 26));
    assert_eq!(stability.strict, Some(1.0));

    // A record of a known company found its identity: the new domain of
    // crm:c-104 arrives as a second record under the same key.
    let kestrel = identity(&engine, "registry", "X0000004").unwrap();
    assert_eq!(
        members(&engine, kestrel),
        BTreeSet::from(["registry:X0000004", "crm:c-104", "crm:c-115"].map(str::to_owned))
    );
    assert_eq!(records(&engine, "crm", "c-104").len(), 2);
}

// A subsidiary with a different registry identifier is not attached to its
// parent.

#[test]
fn a_subsidiary_with_a_different_registry_identifier_is_not_attached_to_its_parent() {
    let engine = company_engine(0);
    run_companies(&engine);

    // Three companies on one domain: the group and two subsidiaries.
    let group = identity(&engine, "crm", "c-106").unwrap();
    let services = identity(&engine, "crm", "c-107").unwrap();
    let energy = identity(&engine, "crm", "c-118").unwrap();
    assert_eq!(BTreeSet::from([group, services, energy]).len(), 3);

    // Each sits with its own registry record.
    assert_eq!(identity(&engine, "registry", "X0000005"), Some(group));
    assert_eq!(identity(&engine, "registry", "X0000006"), Some(services));
    assert_eq!(identity(&engine, "registry", "X0000015"), Some(energy));

    // They were compared, and the registry identifiers kept them apart.
    let kept_apart = count(
        &engine,
        "SELECT count(*) FROM spool.judgement j
         JOIN spool.features_company l ON l.record_id = j.left_id
         JOIN spool.features_company r ON r.record_id = j.right_id
         WHERE l.domain = 'halden.example' AND r.domain = 'halden.example'
           AND j.verdict = 'no_match'
           AND j.detail->>'$.no_match[0]' = 'different_registry_id'",
    );
    assert_eq!(kept_apart, 3);
}

// Two records that a rule says are different are not attached to the same
// identity, even when each of them matches it.

#[test]
fn two_records_a_rule_keeps_apart_are_not_attached_to_the_same_identity() {
    let engine = company_engine(0);
    engine.apply(&Manifest::load(companies()).unwrap()).unwrap();
    let resolve = || engine.resolve(&ResolveOptions::default()).unwrap();

    // A CRM record with a name and a country, and no registry number.
    let crm = Rows::from_text(
        &["id", "name", "country"],
        &[vec![Some("c-1"), Some("Pioneer Supplies"), Some("GB")]],
    )
    .unwrap();
    engine.load("crm", "company", crm).unwrap();
    resolve();
    let known = identity(&engine, "crm", "c-1").unwrap();

    // Two legal entities with that name arrive together. Each matches the CRM
    // record by name and country, and their registry numbers differ.
    let registry = Rows::from_text(
        &["number", "country", "legal_name", "city"],
        &[
            vec![
                Some("X0000009"),
                Some("GB"),
                Some("PIONEER SUPPLIES LTD"),
                Some("Leeds"),
            ],
            vec![
                Some("X0000010"),
                Some("GB"),
                Some("PIONEER SUPPLIES LTD"),
                Some("Bristol"),
            ],
        ],
    )
    .unwrap();
    engine.load("registry", "company", registry).unwrap();
    let report = resolve();

    // No identity holds two registry numbers.
    assert_eq!(
        count(
            &engine,
            "SELECT count(*) FROM (
               SELECT m.identity_id
               FROM spool.membership_current m
               JOIN spool.features_company f ON f.record_id = m.record_id
               GROUP BY m.identity_id
               HAVING count(DISTINCT f.registry_id) > 1)"
        ),
        0
    );
    assert_eq!(members(&engine, known), ["crm:c-1".to_owned()].into());

    // Both wait for review, in one item that names the identity.
    let (leeds, bristol) = (
        record_id(&engine, "registry", "X0000009"),
        record_id(&engine, "registry", "X0000010"),
    );
    let mut waiting = vec![leeds, bristol];
    waiting.sort();
    assert_eq!(
        report.resolved[0].review,
        [ReviewItem::Conflict {
            records: waiting,
            identity: Some(known),
        }]
    );
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.review_open"), 1);
    engine.verify().unwrap();
}

// A record matching two identities is left unassigned with a review
// item.

#[test]
fn an_record_matching_two_identities_is_left_unassigned_with_a_review_item() {
    let engine = company_engine(0);
    let (report, _) = run_companies(&engine);

    // Two unrelated companies share a name. crm:c-117 has that name and
    // nothing to say which of them it is.
    let leeds = identity(&engine, "registry", "X0000009").unwrap();
    let bristol = identity(&engine, "registry", "X0000010").unwrap();
    assert_ne!(leeds, bristol);
    let ambiguous = record_id(&engine, "crm", "c-117");

    assert_eq!(engine.identity_of(ambiguous).unwrap(), None);
    // The item is in the store, open, for whoever reviews it.
    assert_eq!(
        select(
            &engine,
            "SELECT kind, array_to_string(record_ids::VARCHAR[], ','), len(identity_ids)::VARCHAR
             FROM spool.review_open"
        ),
        [[
            Some("possible_merge".to_owned()),
            Some(ambiguous.to_string()),
            Some("2".to_owned())
        ]]
    );
    let resolved = &report.steps[1].resolve.resolved[0];
    assert_eq!(resolved.records_unassigned, 1);
    let [
        ReviewItem::PossibleMerge {
            records,
            identities,
        },
    ] = &resolved.review[..]
    else {
        panic!("one possible merge is flagged: {:?}", resolved.review);
    };
    assert_eq!(records, &[ambiguous]);
    assert_eq!(
        identities.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([leeds, bristol])
    );

    // Nothing was merged on its account, and the scores say so.
    let metrics = &report.steps[1].metrics;
    assert_eq!(metrics.false_merges, 0);
    assert_eq!(metrics.unassigned, 1);
    assert_eq!(metrics.identities, 18);
    // The one split is crm:c-111, which is only the name "Orchard".
    assert_eq!(metrics.false_splits, 1);
    assert_eq!(metrics.pairs.precision, Some(1.0));
}

// A review item is still in the store after the store is reopened, and its
// record is examined again by the next run.

#[test]
fn a_review_item_survives_a_reopen_and_its_record_is_examined_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    let read = |engine: &Engine| {
        select(
            engine,
            "SELECT review_id::VARCHAR, run_id::VARCHAR, kind, status,
                    array_to_string(record_ids::VARCHAR[], ','),
                    array_to_string(identity_ids::VARCHAR[], ',')
             FROM spool.review_item",
        )
    };
    let (before, ambiguous) = {
        let engine = Engine::open(&path, Options::seeded(0)).unwrap();
        run_companies(&engine);
        (read(&engine), record_id(&engine, "crm", "c-117"))
    };
    assert_eq!(before.len(), 1);
    assert_eq!(before[0][3].as_deref(), Some("open"));
    assert_eq!(before[0][4], Some(ambiguous.to_string()));

    let engine = Engine::open(&path, Options::seeded(1)).unwrap();
    assert_eq!(read(&engine), before);

    engine.apply(&Manifest::load(companies()).unwrap()).unwrap();
    let report = engine.resolve(&ResolveOptions::default()).unwrap();
    let again = &report.resolved[0];
    assert_eq!((again.new_records, again.reexamined), (1, 1));
    assert!(matches!(
        &again.review[..],
        [ReviewItem::PossibleMerge { records, .. }] if records == &[ambiguous]
    ));
    // The same conclusion, so the same item: nothing was added or closed.
    assert_eq!(read(&engine), before);
    engine.verify().unwrap();
}

// Rebuilding the projection from the log reproduces the state exactly.

#[test]
fn rebuilding_the_projection_from_the_log_reproduces_the_state_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path().join("spool.duckdb"), Options::seeded(0)).unwrap();
    run_companies(&engine);
    let committed = engine.snapshot().unwrap();
    assert_eq!(committed.identities.len(), 18);
    assert_eq!(committed.memberships.len(), 38);

    // The log alone, replayed by the reference model, gives the same state.
    let replayed = MemoryState::replay(&engine.log().unwrap()).unwrap();
    assert_eq!(replayed.snapshot().unwrap(), committed);

    engine.rebuild_projection().unwrap();
    assert_eq!(engine.snapshot().unwrap(), committed);
    engine.verify().unwrap();
}

// Two runs with the same seed produce the same changeset content hashes.

#[test]
fn two_runs_with_the_same_seed_produce_the_same_changeset_content_hashes() {
    let hashes = |report: &BenchReport| -> Vec<String> {
        report
            .steps
            .iter()
            .flat_map(|step| &step.resolve.resolved)
            .map(|resolved| {
                let committed = resolved.committed.as_ref().expect("each step commits");
                committed.content_hash.to_string()
            })
            .collect()
    };

    // Through the binary, on a database file.
    let first = bench(&companies(), 11);
    let second = bench(&companies(), 11);
    assert_eq!(hashes(&first).len(), 2);
    assert_eq!(hashes(&first), hashes(&second));
    assert_eq!(first, second);

    // Through the library, in memory: the same hashes again.
    let (in_process, _) = run_companies(&company_engine(11));
    assert_eq!(hashes(&in_process), hashes(&first));

    // Another seed mints other identity IDs, so the operations differ. What
    // was decided does not.
    let other = bench(&companies(), 12);
    assert_ne!(hashes(&other), hashes(&first));
    assert_eq!(other.steps[1].metrics, first.steps[1].metrics);
}
