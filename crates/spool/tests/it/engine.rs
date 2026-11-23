//! The engine's API, exercised with a made-up entity type.

use std::collections::BTreeSet;

use spool::core::{
    Actor, Changeset, EntityType, IdGenerator, Op, Policy, Reason, ReviewItem, SeededIds, Seq,
};
use spool::{
    Engine, EntityTypeSource, Error, Options, ResolveOptions, Rows, Scope, StrategyRef, YamlFile,
};
use spool_testing::*;

mod external_runs;
mod loading;

fn resolve(engine: &Engine) -> spool::ResolveReport {
    engine.resolve(&ResolveOptions::default()).unwrap()
}

fn judgements(rows: &[(String, String, &str)]) -> Rows {
    let rows: Vec<Vec<Option<&str>>> = rows
        .iter()
        .map(|(left, right, verdict)| {
            vec![Some(left.as_str()), Some(right.as_str()), Some(*verdict)]
        })
        .collect();
    Rows::from_text(&["left_id", "right_id", "verdict"], &rows).unwrap()
}

// Resolve

#[test]
fn resolve_gives_matching_records_one_identity() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", Some("bellows"), None),
        ],
    );
    let report = resolve(&engine);

    let [resolved] = &report.resolved[..] else {
        panic!("one entity type was resolved: {report:?}");
    };
    assert_eq!(resolved.new_records, 3);
    assert_eq!(resolved.candidates, 1);
    assert_eq!(resolved.judgements.match_, 1);
    assert_eq!(resolved.identities_created, 2);
    assert_eq!(resolved.records_assigned, 3);
    assert_eq!(resolved.records_unassigned, 0);
    assert_eq!(resolved.committed.as_ref().unwrap().seq, Seq(1));

    let anvil = identity(&engine, "shop", "1").unwrap();
    assert_eq!(identity(&engine, "shop", "2"), Some(anvil));
    assert_ne!(identity(&engine, "shop", "3"), Some(anvil));
    assert_eq!(engine.current_id(anvil).unwrap(), anvil);
    engine.verify().unwrap();

    // The judgement says which rules held.
    assert_eq!(
        select(
            &engine,
            "SELECT verdict, detail->>'$.block[0]', detail->>'$.match[0]' FROM spool.judgement"
        ),
        [[
            Some("match".to_owned()),
            Some("same_name".to_owned()),
            Some("same_name".to_owned())
        ]]
    );
    // The entity view joins identities to their members' features.
    assert_eq!(
        select(
            &engine,
            &format!("SELECT name FROM spool.entity_thing WHERE identity_id = '{anvil}'")
        ),
        [[Some("anvil".to_owned())], [Some("anvil".to_owned())]]
    );
}

#[test]
fn resent_rows_create_no_records_and_no_operations() {
    let engine = engine();
    let batch = [("1", Some("anvil"), None), ("2", Some("anvil"), None)];
    load(&engine, "shop", &batch);
    resolve(&engine);
    let (head, log) = (engine.head().unwrap(), engine.log().unwrap());

    let again = load(&engine, "shop", &batch);
    assert_eq!(again.inserted, 0);
    let report = resolve(&engine);

    assert!(report.resolved.is_empty(), "{report:?}");
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(engine.log().unwrap(), log);
    assert_eq!(
        count(
            &engine,
            "SELECT count(*) FROM spool.run WHERE kind = 'match'"
        ),
        1
    );
}

#[test]
fn identities_keep_their_ids_and_gain_members() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("bellows"), None)],
    );
    resolve(&engine);
    let before = engine.snapshot().unwrap();
    let anvil = identity(&engine, "shop", "1").unwrap();

    load(
        &engine,
        "depot",
        &[("7", Some("anvil"), None), ("8", Some("chisel"), None)],
    );
    resolve(&engine);
    let after = engine.snapshot().unwrap();

    for identity in &before.identities {
        assert!(after.identities.contains(identity), "{identity:?} is gone");
    }
    for membership in &before.memberships {
        assert!(
            after.memberships.contains(membership),
            "{membership:?} changed"
        );
    }
    assert_eq!(
        members(&engine, anvil),
        BTreeSet::from(["shop:1".to_owned(), "depot:7".to_owned()])
    );
    assert_eq!(after.identities.len(), 3);

    let stability = engine.id_stability(before.head, after.head).unwrap();
    assert_eq!((stability.records, stability.unchanged), (2, 2));
    assert_eq!(stability.strict, Some(1.0));
}

#[test]
fn an_record_matching_two_identities_is_left_for_review() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", None, Some("K7"))],
    );
    resolve(&engine);
    let (by_name, by_code) = (
        identity(&engine, "shop", "1").unwrap(),
        identity(&engine, "shop", "2").unwrap(),
    );
    let head = engine.head().unwrap();

    load(&engine, "depot", &[("9", Some("anvil"), Some("K7"))]);
    let report = resolve(&engine);

    let resolved = &report.resolved[0];
    let both = record_id(&engine, "depot", "9");
    assert_eq!(resolved.records_unassigned, 1);
    assert_eq!(resolved.committed, None);
    let [
        ReviewItem::PossibleMerge {
            records,
            identities,
        },
    ] = &resolved.review[..]
    else {
        panic!("one possible merge is flagged: {:?}", resolved.review);
    };
    assert_eq!(records, &[both]);
    assert_eq!(
        identities.iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([&by_name, &by_code])
    );
    assert_eq!(engine.identity_of(both).unwrap(), None);
    assert_eq!(engine.head().unwrap(), head);
}

#[test]
fn a_no_match_keeps_an_record_out_of_an_identity() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), Some("A"))]);
    resolve(&engine);

    load(&engine, "depot", &[("9", Some("anvil"), Some("B"))]);
    let report = resolve(&engine);

    assert_eq!(report.resolved[0].judgements.no_match, 1);
    assert_eq!(report.resolved[0].identities_created, 1);
    assert_ne!(
        identity(&engine, "depot", "9"),
        identity(&engine, "shop", "1")
    );
}

#[test]
fn contradictory_evidence_about_one_identity_is_left_for_review() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), Some("A")), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();

    // Matches shop:2 by name, and cannot be shop:1 because the codes differ.
    load(&engine, "depot", &[("9", Some("anvil"), Some("B"))]);
    let report = resolve(&engine);

    assert_eq!(
        report.resolved[0].review,
        [ReviewItem::Conflict {
            records: vec![record_id(&engine, "depot", "9")],
            identity: Some(anvil),
        }]
    );
    assert_eq!(identity(&engine, "depot", "9"), None);
}

#[test]
fn entity_types_are_resolved_apart() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let widget = EntityTypeSource {
        name: "widget".to_owned(),
        ..thing()
    };
    engine.apply(&manifest_of(vec![thing(), widget])).unwrap();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    engine
        .load("shop", "widget", rows(&[("2", Some("anvil"), None)]))
        .unwrap();

    let only_widgets = ResolveOptions {
        entity_type: Some(EntityType::new("widget").unwrap()),
        ..ResolveOptions::default()
    };
    let report = engine.resolve(&only_widgets).unwrap();
    assert_eq!(report.resolved.len(), 1);
    assert_eq!(identity(&engine, "shop", "1"), None);

    let report = resolve(&engine);
    assert_eq!(report.resolved.len(), 1);
    assert_eq!(report.resolved[0].entity_type.as_str(), "thing");
    assert_eq!(report.resolved[0].candidates, 0);
    assert_ne!(
        identity(&engine, "shop", "1"),
        identity(&engine, "shop", "2")
    );
    assert_eq!(
        select(
            &engine,
            "SELECT entity_type FROM spool.identity_current ORDER BY 1"
        ),
        [[Some("thing".to_owned())], [Some("widget".to_owned())]]
    );
}

// The store

fn two_batches(engine: &Engine) {
    load(
        engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", Some("bellows"), None),
        ],
    );
    resolve(engine);
    load(
        engine,
        "depot",
        &[("7", Some("bellows"), None), ("8", Some("chisel"), None)],
    );
    resolve(engine);
}

#[test]
fn rebuilding_the_projection_from_the_log_reproduces_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    let expected = {
        let engine = Engine::open(&path, Options::seeded(1)).unwrap();
        engine.apply(&things()).unwrap();
        two_batches(&engine);
        engine.snapshot().unwrap()
    };
    assert_eq!(expected.identities.len(), 3);
    assert_eq!(expected.memberships.len(), 5);

    // Damage the projection behind the engine's back. The log is untouched.
    {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "ATTACH '{}' AS db;
             DELETE FROM db.spool.identity;
             UPDATE db.spool.membership SET to_seq = 2 WHERE from_seq = 1;",
            path.display()
        ))
        .unwrap();
    }

    let engine = Engine::open(&path, Options::seeded(2)).unwrap();
    assert_ne!(engine.snapshot().unwrap(), expected);
    assert!(matches!(engine.verify(), Err(Error::Verify(_))));

    engine.rebuild_projection().unwrap();
    assert_eq!(engine.snapshot().unwrap(), expected);
    engine.verify().unwrap();
}

#[test]
fn the_same_seed_gives_the_same_changesets() {
    let run = |seed: u64| {
        let engine = Engine::open_in_memory(Options::seeded(seed)).unwrap();
        engine.apply(&things()).unwrap();
        two_batches(&engine);
        let partition: BTreeSet<BTreeSet<String>> = engine
            .snapshot()
            .unwrap()
            .identities
            .iter()
            .map(|identity| members(&engine, identity.id))
            .collect();
        (engine.log().unwrap(), partition)
    };
    let (first, first_partition) = run(7);
    let (second, _) = run(7);
    let (other, other_partition) = run(8);

    assert_eq!(first.len(), 2);
    assert_eq!(first, second);
    let hashes = |log: &[spool::core::LogEntry]| {
        log.iter()
            .map(|entry| entry.content_hash.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes(&first), hashes(&second));
    // Another seed mints other IDs for the same grouping.
    assert_ne!(hashes(&first), hashes(&other));
    assert_eq!(first_partition, other_partition);
}

#[test]
fn state_survives_reopening_and_a_changed_declaration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    let (snapshot, anvil) = {
        let engine = Engine::open(&path, Options::seeded(1)).unwrap();
        let report = engine.apply(&things()).unwrap();
        assert_eq!(report.declared.len(), 1);
        two_batches(&engine);
        (engine.snapshot().unwrap(), identity(&engine, "shop", "1"))
    };

    let engine = Engine::open(&path, Options::seeded(2)).unwrap();
    assert_eq!(engine.snapshot().unwrap(), snapshot);
    assert_eq!(identity(&engine, "shop", "1"), anvil);
    assert!(matches!(
        engine.resolve(&ResolveOptions::default()),
        Err(Error::NoManifest)
    ));

    let changed = EntityTypeSource {
        entity: entity(
            Some("name"),
            &[
                ("name", "{ from: name, transform: upper }"),
                ("code", "code"),
            ],
            &SOURCES,
        ),
        ..thing()
    };
    // Rules may change without touching what is stored.
    let fewer_rules = EntityTypeSource {
        no_match: Vec::new(),
        ..thing()
    };
    let report = engine.apply(&manifest_of(vec![fewer_rules])).unwrap();
    assert!(report.declared.is_empty());
    assert!(report.rebuilt.is_empty());
    assert_eq!(report.unchanged.len(), 1);

    // A changed declaration computes the feature table again from the stored
    // payloads, under the next version. Identities are not touched.
    let names = |engine: &Engine| {
        select(
            engine,
            "SELECT DISTINCT name, declaration_version::VARCHAR FROM spool.features_thing
             WHERE name IS NOT NULL ORDER BY 1",
        )
    };
    let before = names(&engine);
    let report = engine.apply(&manifest_of(vec![changed.clone()])).unwrap();
    assert_eq!(report.rebuilt.len(), 1);
    assert!(report.declared.is_empty() && report.unchanged.is_empty());
    let after = names(&engine);
    assert_eq!(after.len(), before.len());
    for (old, new) in before.iter().zip(&after) {
        assert_eq!(
            old[0].as_ref().unwrap().to_uppercase(),
            *new[0].as_ref().unwrap()
        );
        assert_eq!(
            (old[1].as_deref(), new[1].as_deref()),
            (Some("1"), Some("2"))
        );
    }
    assert_eq!(
        count(&engine, "SELECT count(*) FROM spool.features_thing"),
        count(&engine, "SELECT count(*) FROM spool.record")
    );
    assert_eq!(engine.snapshot().unwrap(), snapshot);
    assert_eq!(identity(&engine, "shop", "1"), anvil);
    assert_eq!(
        engine.info().unwrap().entity_types[0].declaration_version,
        2
    );
    engine.verify().unwrap();

    // The entity view follows the new columns, and later loads use them.
    load(&engine, "kiosk", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    assert_eq!(identity(&engine, "kiosk", "1"), anvil);
    assert_eq!(
        count(
            &engine,
            "SELECT count(*) FROM spool.entity_thing WHERE name = 'ANVIL'"
        ),
        3
    );

    // Applying it again changes nothing. Going back to the first declaration
    // is a change like any other, and gets a version of its own.
    let report = engine.apply(&manifest_of(vec![changed])).unwrap();
    assert_eq!(report.unchanged.len(), 1);
    let report = engine.apply(&things()).unwrap();
    assert_eq!(report.rebuilt.len(), 1);
    assert_eq!(
        engine.info().unwrap().entity_types[0].declaration_version,
        3
    );

    load(&engine, "depot", &[("9", Some("anvil"), None)]);
    resolve(&engine);
    assert_eq!(identity(&engine, "depot", "9"), anvil);
    engine.verify().unwrap();
}

#[test]
fn query_runs_selects_only() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);

    for statement in [
        "DELETE FROM spool.record",
        "DROP TABLE spool.record",
        "SELECT 1; DELETE FROM spool.record",
        "",
    ] {
        let error = engine.query(statement).unwrap_err();
        assert!(matches!(error, Error::Query(_)), "{statement:?}: {error}");
    }
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.record"), 1);
    // One trailing semicolon is tolerated.
    assert!(
        engine
            .query("SELECT count(*) FROM spool.membership_current;")
            .is_ok()
    );
}

#[test]
fn an_unknown_identity_has_no_current_id() {
    let engine = engine();
    let unknown = SeededIds::new(3).identity_id();
    assert!(matches!(
        engine.current_id(unknown),
        Err(Error::UnknownIdentity(id)) if id == unknown
    ));
}
