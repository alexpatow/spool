//! The engine's API, exercised with a made-up entity type.

mod common;

use std::collections::BTreeSet;

use common::*;
use spool::core::{
    Actor, Changeset, EntityType, IdGenerator, Op, Policy, Reason, ReviewItem, SeededIds, Seq,
};
use spool::{
    Engine, EntityTypeSource, Error, IngestOptions, Options, ResolveOptions, Rows, Scope, SqlFile,
    StrategyRef,
};

fn resolve(engine: &Engine) -> spool::ResolveReport {
    engine.resolve(&ResolveOptions::default()).unwrap()
}

fn thing_scope() -> Scope {
    Scope::new(EntityType::new("thing").unwrap())
}

fn by_hand() -> StrategyRef {
    StrategyRef::external("by-hand", None)
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

// Ingest

#[test]
fn ingest_stores_each_row_once() {
    let engine = engine();
    let batch = [
        ("1", Some("anvil"), Some("A")),
        ("2", Some("bellows"), None),
        ("1", Some("anvil"), Some("A")),
    ];
    let first = ingest(&engine, "shop", &batch);
    assert_eq!(
        (first.received, first.inserted, first.duplicates),
        (3, 2, 1)
    );

    let again = ingest(&engine, "shop", &batch);
    assert_eq!(
        (again.received, again.inserted, again.duplicates),
        (3, 0, 3)
    );

    assert_eq!(count(&engine, "SELECT count(*) FROM spool.observation"), 2);
    assert_eq!(
        count(&engine, "SELECT count(*) FROM spool.features_thing"),
        2
    );
    engine.verify().unwrap();
}

#[test]
fn an_observation_id_ignores_column_order_and_empty_columns() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), Some("A"))]);

    let reordered = Rows::from_text(
        &["code", "colour", "name", "id"],
        &[vec![Some("A"), None, Some("anvil"), Some("1")]],
    )
    .unwrap();
    let report = engine
        .ingest("shop", "thing", reordered, &IngestOptions::keyed_by("id"))
        .unwrap();
    assert_eq!((report.inserted, report.duplicates), (0, 1));
}

#[test]
fn a_changed_row_or_another_source_is_another_observation() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), Some("A"))]);

    let changed = ingest(&engine, "shop", &[("1", Some("anvil, large"), Some("A"))]);
    assert_eq!(changed.inserted, 1);
    assert_eq!(observations(&engine, "shop", "1").len(), 2);

    let elsewhere = ingest(&engine, "depot", &[("1", Some("anvil"), Some("A"))]);
    assert_eq!(elsewhere.inserted, 1);
}

#[test]
fn ingest_needs_an_applied_manifest_and_a_declared_type() {
    let bare = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let error = bare
        .ingest("shop", "thing", rows(&[]), &IngestOptions::default())
        .unwrap_err();
    assert!(matches!(error, Error::NoManifest), "{error}");

    let engine = engine();
    let error = engine
        .ingest("shop", "gadget", rows(&[]), &IngestOptions::default())
        .unwrap_err();
    assert!(matches!(error, Error::UndeclaredType(name) if name == "gadget"));
}

#[test]
fn evidence_is_stored_beside_the_payload_and_needs_a_key() {
    let engine = engine();
    let options = IngestOptions {
        key_column: Some("locator".to_owned()),
        evidence_column: Some("evidence".to_owned()),
    };
    let mentions = |locator: Option<&'static str>| {
        Rows::from_text(
            &["locator", "name", "evidence"],
            &[vec![locator, Some("anvil"), Some(r#"{"page": 3}"#)]],
        )
        .unwrap()
    };

    let error = engine
        .ingest("report.pdf", "thing", mentions(None), &options)
        .unwrap_err();
    assert!(
        matches!(&error, Error::Ingest(message) if message.contains("evidence")),
        "{error}"
    );
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.observation"), 0);
    assert_eq!(
        select(
            &engine,
            "SELECT status FROM spool.run WHERE kind = 'ingest'"
        ),
        [[Some("failed".to_owned())]]
    );

    engine
        .ingest(
            "report.pdf",
            "thing",
            mentions(Some("p3:120-125")),
            &options,
        )
        .unwrap();
    assert_eq!(
        select(
            &engine,
            "SELECT source_key, payload::VARCHAR, evidence->>'page' FROM spool.observation"
        ),
        [[
            Some("p3:120-125".to_owned()),
            Some(r#"{"locator":"p3:120-125","name":"anvil"}"#.to_owned()),
            Some("3".to_owned())
        ]]
    );

    // The same words at another place in the document are another observation.
    let report = engine
        .ingest("report.pdf", "thing", mentions(Some("p9:14-19")), &options)
        .unwrap();
    assert_eq!(report.inserted, 1);
}

#[test]
fn a_known_row_cannot_arrive_as_another_entity_type() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let widget = EntityTypeSource {
        name: "widget".to_owned(),
        ..thing()
    };
    engine.apply(&manifest_of(vec![thing(), widget])).unwrap();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);

    let error = engine
        .ingest(
            "shop",
            "widget",
            rows(&[("1", Some("anvil"), None)]),
            &IngestOptions::keyed_by("id"),
        )
        .unwrap_err();
    assert!(
        matches!(&error, Error::Ingest(message) if message.contains("`thing`")),
        "{error}"
    );
}

#[test]
fn a_features_query_that_repeats_an_observation_fails_the_ingest() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let doubled = EntityTypeSource {
        features: SqlFile::new(
            "features",
            "SELECT observation_id, 1 AS n FROM observation
             UNION ALL SELECT observation_id, 2 FROM observation",
        ),
        block: Vec::new(),
        no_match: Vec::new(),
        match_: Vec::new(),
        ..thing()
    };
    engine.apply(&manifest_of(vec![doubled])).unwrap();

    let error = engine
        .ingest(
            "shop",
            "thing",
            rows(&[("1", Some("anvil"), None)]),
            &IngestOptions::keyed_by("id"),
        )
        .unwrap_err();
    assert!(
        matches!(&error, Error::Ingest(message) if message.contains("one row per observation")),
        "{error}"
    );
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.observation"), 0);
    assert_eq!(
        count(&engine, "SELECT count(*) FROM spool.features_thing"),
        0
    );
}

// Resolve

#[test]
fn resolve_gives_matching_observations_one_identity() {
    let engine = engine();
    ingest(
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
    assert_eq!(resolved.new_observations, 3);
    assert_eq!(resolved.candidates, 1);
    assert_eq!(resolved.judgements.match_, 1);
    assert_eq!(resolved.identities_created, 2);
    assert_eq!(resolved.observations_assigned, 3);
    assert_eq!(resolved.observations_unassigned, 0);
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
fn resent_rows_create_no_observations_and_no_operations() {
    let engine = engine();
    let batch = [("1", Some("anvil"), None), ("2", Some("anvil"), None)];
    ingest(&engine, "shop", &batch);
    resolve(&engine);
    let (head, log) = (engine.head().unwrap(), engine.log().unwrap());

    let again = ingest(&engine, "shop", &batch);
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
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("bellows"), None)],
    );
    resolve(&engine);
    let before = engine.snapshot().unwrap();
    let anvil = identity(&engine, "shop", "1").unwrap();

    ingest(
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
    assert_eq!((stability.observations, stability.unchanged), (2, 2));
    assert_eq!(stability.strict, Some(1.0));
}

#[test]
fn an_observation_matching_two_identities_is_left_for_review() {
    let engine = engine();
    ingest(
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

    ingest(&engine, "depot", &[("9", Some("anvil"), Some("K7"))]);
    let report = resolve(&engine);

    let resolved = &report.resolved[0];
    let both = observation(&engine, "depot", "9");
    assert_eq!(resolved.observations_unassigned, 1);
    assert_eq!(resolved.committed, None);
    let [
        ReviewItem::PossibleMerge {
            observations,
            identities,
        },
    ] = &resolved.review[..]
    else {
        panic!("one possible merge is flagged: {:?}", resolved.review);
    };
    assert_eq!(observations, &[both]);
    assert_eq!(
        identities.iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([&by_name, &by_code])
    );
    assert_eq!(engine.identity_of(both).unwrap(), None);
    assert_eq!(engine.head().unwrap(), head);
}

#[test]
fn a_no_match_keeps_an_observation_out_of_an_identity() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), Some("A"))]);
    resolve(&engine);

    ingest(&engine, "depot", &[("9", Some("anvil"), Some("B"))]);
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
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), Some("A")), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();

    // Matches shop:2 by name, and cannot be shop:1 because the codes differ.
    ingest(&engine, "depot", &[("9", Some("anvil"), Some("B"))]);
    let report = resolve(&engine);

    assert_eq!(
        report.resolved[0].review,
        [ReviewItem::Conflict {
            observations: vec![observation(&engine, "depot", "9")],
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
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    engine
        .ingest(
            "shop",
            "widget",
            rows(&[("2", Some("anvil"), None)]),
            &IngestOptions::keyed_by("id"),
        )
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

// A run filled from outside the engine

#[test]
fn an_external_strategy_fills_a_run_and_nothing_changes_until_commit() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), Some("A")),
            ("2", Some("anvil"), Some("B")),
        ],
    );
    let run = engine.begin_run(&by_hand(), &thing_scope()).unwrap();

    let candidates = engine.candidates(run).unwrap();
    let [pairs] = &candidates[..] else {
        panic!("one batch of candidates");
    };
    let columns: Vec<String> = pairs
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect();
    assert_eq!(
        columns,
        [
            "left_id",
            "right_id",
            "left_name",
            "left_code",
            "right_name",
            "right_code"
        ]
    );
    assert_eq!(pairs.num_rows(), 1);
    assert_eq!(cell(pairs, 2, 0).as_deref(), Some("anvil"));

    // Sides swapped on purpose: the engine stores a pair one way round. The
    // project's rules would have said no_match; this strategy disagrees.
    let (left, right) = (cell(pairs, 0, 0).unwrap(), cell(pairs, 1, 0).unwrap());
    let stored = engine
        .submit_judgements(run, judgements(&[(right, left, "match")]))
        .unwrap();
    assert_eq!(stored, 1);
    assert_eq!(
        count(
            &engine,
            "SELECT count(*) FROM spool.judgement WHERE left_id < right_id"
        ),
        1
    );

    let draft = engine.reconcile(run, &Policy::default()).unwrap();
    assert!(draft.review.is_empty());
    assert_eq!(draft.changeset.actor, Actor::Engine);
    assert_eq!(draft.changeset.ops.len(), 3);
    assert_eq!(engine.head().unwrap(), Seq::ZERO);
    assert_eq!(identity(&engine, "shop", "1"), None);

    let committed = engine.commit(draft.changeset.clone()).unwrap();
    assert_eq!(committed.seq, Seq(1));
    assert!(identity(&engine, "shop", "1").is_some());
    assert_eq!(
        identity(&engine, "shop", "1"),
        identity(&engine, "shop", "2")
    );

    // The match run is closed, and the changeset cites the reconcile run.
    assert_eq!(
        select(
            &engine,
            "SELECT r.kind, r.status, r.strategy FROM spool.run r ORDER BY r.run_id"
        ),
        [
            [
                Some("ingest".to_owned()),
                Some("succeeded".to_owned()),
                None
            ],
            [
                Some("match".to_owned()),
                Some("succeeded".to_owned()),
                Some("by-hand".to_owned())
            ],
            [
                Some("reconcile".to_owned()),
                Some("succeeded".to_owned()),
                None
            ],
        ]
    );
    assert_eq!(
        select(
            &engine,
            "SELECT r.kind FROM spool.changeset c JOIN spool.run r ON r.run_id = c.run_id"
        ),
        [[Some("reconcile".to_owned())]]
    );
    let error = engine.submit_judgements(run, judgements(&[])).unwrap_err();
    assert!(matches!(error, Error::Run { .. }), "{error}");
    engine.verify().unwrap();
}

#[test]
fn a_run_records_the_project_version_it_read() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    engine.begin_run(&by_hand(), &thing_scope()).unwrap();

    assert_eq!(
        select(
            &engine,
            "SELECT config->>'manifest_hash', config->>'declaration_version'
             FROM spool.run WHERE kind = 'match'"
        ),
        [[Some(things().hash), Some("1".to_owned())]]
    );
}

#[test]
fn judgements_must_name_known_observations_and_a_new_one() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    ingest(&engine, "depot", &[("9", Some("anvil"), None)]);
    let run = engine.begin_run(&by_hand(), &thing_scope()).unwrap();

    let id = |source: &str, key: &str| observation(&engine, source, key).to_string();
    let (old_1, old_2, new) = (id("shop", "1"), id("shop", "2"), id("depot", "9"));
    let unknown = "00000000-0000-4000-8000-000000000000".to_owned();

    let rejected = |rows: Rows, expected: &str| {
        let error = engine.submit_judgements(run, rows).unwrap_err();
        assert!(
            matches!(&error, Error::Judgements(message) if message.contains(expected)),
            "expected `{expected}` in: {error}"
        );
    };
    rejected(
        judgements(&[(new.clone(), unknown, "match")]),
        "not a known thing",
    );
    rejected(
        judgements(&[(old_1.clone(), old_2, "match")]),
        "neither observation is new",
    );
    rejected(
        judgements(&[(new.clone(), old_1.clone(), "maybe")]),
        "verdict",
    );
    rejected(
        judgements(&[(new.clone(), new.clone(), "match")]),
        "same observation",
    );
    rejected(
        judgements(&[(new.clone(), "not-an-id".to_owned(), "match")]),
        "not a UUID",
    );
    rejected(
        Rows::from_text(&["left_id", "right_id"], &[]).unwrap(),
        "`verdict`",
    );
    // A bad row refuses the whole submission.
    rejected(
        judgements(&[
            (new.clone(), old_1.clone(), "match"),
            (new.clone(), "not-an-id".to_owned(), "match"),
        ]),
        "not a UUID",
    );
    assert_eq!(
        count(
            &engine,
            &format!("SELECT count(*) FROM spool.judgement WHERE run_id = {run}")
        ),
        0
    );

    let scored = Rows::from_text(
        &["left_id", "right_id", "verdict", "score", "detail"],
        &[vec![
            Some(new.as_str()),
            Some(old_1.as_str()),
            Some("unsure"),
            Some("0.4"),
            Some(r#"{"why": "too little to go on"}"#),
        ]],
    )
    .unwrap();
    assert_eq!(engine.submit_judgements(run, scored).unwrap(), 1);

    // `unsure` decides nothing, so the new observation stands alone.
    let draft = engine.reconcile(run, &Policy::default()).unwrap();
    engine.commit(draft.changeset).unwrap();
    assert_ne!(
        identity(&engine, "depot", "9"),
        identity(&engine, "shop", "1")
    );
}

// Commit

#[test]
fn a_stale_draft_is_rejected_and_names_the_operation() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();

    ingest(&engine, "depot", &[("9", Some("anvil"), None)]);
    let run = engine
        .begin_run(&StrategyRef::rules(), &thing_scope())
        .unwrap();
    let new = observation(&engine, "depot", "9");
    engine
        .submit_judgements(
            run,
            judgements(&[(
                new.to_string(),
                observation(&engine, "shop", "1").to_string(),
                "match",
            )]),
        )
        .unwrap();
    let draft = engine.reconcile(run, &Policy::default()).unwrap();
    assert_eq!(
        draft.changeset.ops,
        [Op::Assign {
            observation: new,
            from: None,
            to: Some(anvil)
        }]
    );

    // While the draft waits, someone puts the observation elsewhere.
    let mut ids = SeededIds::new(99);
    let elsewhere = ids.identity_id();
    let correction = Changeset {
        id: ids.changeset_id(),
        base: engine.head().unwrap(),
        actor: Actor::User("reviewer".to_owned()),
        reason: Some("a different anvil".to_owned()),
        run: None,
        ops: vec![
            Op::CreateIdentity {
                identity: elsewhere,
                entity_type: EntityType::new("thing").unwrap(),
            },
            Op::Assign {
                observation: new,
                from: None,
                to: Some(elsewhere),
            },
        ],
    };
    engine.commit(correction.clone()).unwrap();
    let head = engine.head().unwrap();

    let error = engine.commit(draft.changeset.clone()).unwrap_err();
    let Error::Rejected(rejections) = &error else {
        panic!("the draft is rejected: {error}");
    };
    assert_eq!(rejections.len(), 1);
    assert_eq!(rejections[0].op_index, Some(0));
    assert_eq!(rejections[0].op.as_ref(), Some(&draft.changeset.ops[0]));
    assert_eq!(
        rejections[0].reason,
        Reason::StaleAssignment {
            observation: new,
            expected: None,
            actual: Some(elsewhere)
        }
    );
    assert!(
        error
            .to_string()
            .contains("operation 0 (assign): stale precondition"),
        "{error}"
    );
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(engine.identity_of(new).unwrap(), Some(elsewhere));

    // A changeset ID is an idempotency key: it commits once.
    let error = engine.commit(correction).unwrap_err();
    assert!(
        matches!(&error, Error::Rejected(r) if matches!(r[0].reason, Reason::AlreadyCommitted { .. })),
        "{error}"
    );
}

// The store

fn two_batches(engine: &Engine) {
    ingest(
        engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", Some("bellows"), None),
        ],
    );
    resolve(engine);
    ingest(
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
        features: SqlFile::new(
            "features",
            "SELECT observation_id, upper(payload->>'name') AS name, payload->>'code' AS code
             FROM observation",
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
        count(&engine, "SELECT count(*) FROM spool.observation")
    );
    assert_eq!(engine.snapshot().unwrap(), snapshot);
    assert_eq!(identity(&engine, "shop", "1"), anvil);
    assert_eq!(
        engine.info().unwrap().entity_types[0].declaration_version,
        2
    );
    engine.verify().unwrap();

    // The entity view follows the new columns, and later ingests use them.
    ingest(&engine, "kiosk", &[("1", Some("anvil"), None)]);
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

    ingest(&engine, "depot", &[("9", Some("anvil"), None)]);
    resolve(&engine);
    assert_eq!(identity(&engine, "depot", "9"), anvil);
    engine.verify().unwrap();
}

#[test]
fn query_runs_selects_only() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);

    for statement in [
        "DELETE FROM spool.observation",
        "DROP TABLE spool.observation",
        "SELECT 1; DELETE FROM spool.observation",
        "",
    ] {
        let error = engine.query(statement).unwrap_err();
        assert!(matches!(error, Error::Query(_)), "{statement:?}: {error}");
    }
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.observation"), 1);
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
