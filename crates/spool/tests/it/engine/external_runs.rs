//! A run filled from outside the engine, and committing what it drafted.

use super::*;

fn thing_scope() -> Scope {
    Scope::new(EntityType::new("thing").unwrap())
}

fn by_hand() -> StrategyRef {
    StrategyRef::external("by-hand", None)
}

#[test]
fn an_external_strategy_fills_a_run_and_nothing_changes_until_commit() {
    let engine = engine();
    load(
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
            [Some("load".to_owned()), Some("succeeded".to_owned()), None],
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
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
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
fn judgements_must_name_known_records_and_a_new_one() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    load(&engine, "depot", &[("9", Some("anvil"), None)]);
    let run = engine.begin_run(&by_hand(), &thing_scope()).unwrap();

    let id = |source: &str, key: &str| record_id(&engine, source, key).to_string();
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
        "neither record is new",
    );
    rejected(
        judgements(&[(new.clone(), old_1.clone(), "maybe")]),
        "verdict",
    );
    rejected(
        judgements(&[(new.clone(), new.clone(), "match")]),
        "same record",
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

    // `unsure` decides nothing, so the new record stands alone.
    let draft = engine.reconcile(run, &Policy::default()).unwrap();
    engine.commit(draft.changeset).unwrap();
    assert_ne!(
        identity(&engine, "depot", "9"),
        identity(&engine, "shop", "1")
    );
}

#[test]
fn a_stale_draft_is_rejected_and_names_the_operation() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();

    load(&engine, "depot", &[("9", Some("anvil"), None)]);
    let run = engine
        .begin_run(&StrategyRef::rules(), &thing_scope())
        .unwrap();
    let new = record_id(&engine, "depot", "9");
    engine
        .submit_judgements(
            run,
            judgements(&[(
                new.to_string(),
                record_id(&engine, "shop", "1").to_string(),
                "match",
            )]),
        )
        .unwrap();
    let draft = engine.reconcile(run, &Policy::default()).unwrap();
    assert_eq!(
        draft.changeset.ops,
        [Op::Assign {
            record: new,
            from: None,
            to: Some(anvil)
        }]
    );

    // While the draft waits, someone puts the record elsewhere.
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
                record: new,
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
            record: new,
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
