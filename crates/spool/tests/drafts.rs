//! Drafts left for review: stored, approved, rejected, superseded.

mod common;

use common::*;
use spool::core::{Actor, Changeset, EntityType, IdGenerator, Op, Policy, Reason, SeededIds};
use spool::{
    Disposition, DraftFilter, DraftStatus, Engine, Error, Options, ResolveOptions, RunStatus,
    Scope, StrategyRef,
};

fn leave_for_review() -> ResolveOptions {
    ResolveOptions {
        leave_for_review: true,
        ..ResolveOptions::default()
    }
}

fn two_anvils_and_a_hammer(engine: &Engine) {
    ingest(
        engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", Some("hammer"), None),
        ],
    );
}

#[test]
fn a_draft_left_for_review_is_stored_and_approving_it_commits_it() {
    let engine = engine();
    two_anvils_and_a_hammer(&engine);

    let report = engine.resolve(&leave_for_review()).unwrap();
    let [resolution] = &report.resolved[..] else {
        panic!("one type resolved");
    };
    let id = resolution.draft.expect("a draft was stored");
    assert_eq!(resolution.committed, None);
    assert_eq!(resolution.identities_created, 2);
    assert_eq!(resolution.observations_assigned, 3);

    // Nothing is committed, and the run waits on its draft.
    assert_eq!(engine.head().unwrap().0, 0);
    assert_eq!(identity(&engine, "shop", "1"), None);
    let run = engine.run(resolution.match_run).unwrap();
    assert_eq!(run.status, RunStatus::Drafted);
    assert_eq!(run.finished_at, None);
    assert_eq!(run.resolution.as_ref(), Some(resolution));

    let [summary] = &engine.drafts(&DraftFilter::open()).unwrap()[..] else {
        panic!("one open draft");
    };
    assert_eq!(summary.id, id);
    assert_eq!(summary.status, DraftStatus::Open);
    assert_eq!(summary.match_run, resolution.match_run);
    assert_eq!(summary.reconcile_run, resolution.reconcile_run);
    assert_eq!(
        (
            summary.operations,
            summary.identities_created,
            summary.observations_assigned
        ),
        (5, 2, 3)
    );
    let record = engine.draft(id).unwrap();
    assert_eq!(record.draft, *summary);
    assert_eq!(record.changeset.id, id);
    assert_eq!(record.changeset.ops.len(), 5);
    assert_eq!(record.changeset.run, Some(resolution.reconcile_run));
    engine.verify().unwrap();

    let committed = engine.approve_draft(id).unwrap();
    assert_eq!(committed.changeset, id);
    assert_eq!(committed.content_hash, record.changeset.content_hash());
    assert_eq!(
        identity(&engine, "shop", "1"),
        identity(&engine, "shop", "2")
    );
    assert!(identity(&engine, "shop", "3").is_some());

    // The run has succeeded now, and its record says what was committed.
    let run = engine.run(resolution.match_run).unwrap();
    assert_eq!(run.status, RunStatus::Succeeded);
    assert!(run.finished_at.is_some());
    assert_eq!(run.resolution.unwrap().committed, Some(committed.clone()));
    let summary = engine.draft(id).unwrap().draft;
    assert_eq!(summary.status, DraftStatus::Committed);
    assert_eq!(summary.committed_seq, Some(committed.seq));
    assert!(engine.drafts(&DraftFilter::open()).unwrap().is_empty());

    // It cannot be approved or rejected a second time.
    for error in [
        engine.approve_draft(id).unwrap_err(),
        engine.reject_draft(id, None).unwrap_err(),
    ] {
        assert!(matches!(error, Error::Draft { .. }), "{error}");
    }
    // And with nothing new, the next resolve has nothing to do.
    assert!(
        engine
            .resolve(&ResolveOptions::default())
            .unwrap()
            .resolved
            .is_empty()
    );
    engine.verify().unwrap();
}

#[test]
fn an_approved_draft_has_the_hash_of_a_direct_commit() {
    let direct = engine();
    two_anvils_and_a_hammer(&direct);
    let committed = direct.resolve(&ResolveOptions::default()).unwrap().resolved[0]
        .committed
        .clone()
        .unwrap();

    let reviewed = engine();
    two_anvils_and_a_hammer(&reviewed);
    let draft = reviewed.resolve(&leave_for_review()).unwrap().resolved[0]
        .draft
        .unwrap();
    assert_eq!(reviewed.approve_draft(draft).unwrap(), committed);
    assert_eq!(reviewed.snapshot().unwrap(), direct.snapshot().unwrap());
}

#[test]
fn a_draft_nobody_commits_does_not_strand_its_observations() {
    let engine = engine();
    two_anvils_and_a_hammer(&engine);
    let abandoned = engine.resolve(&leave_for_review()).unwrap().resolved[0]
        .draft
        .unwrap();

    // Nobody decides. More arrives, and the next run commits.
    ingest(&engine, "depot", &[("9", Some("anvil"), None)]);
    let report = engine.resolve(&ResolveOptions::default()).unwrap();
    let resolution = &report.resolved[0];
    assert_eq!(resolution.new_observations, 4);
    assert_eq!(resolution.observations_assigned, 4);
    assert!(resolution.committed.is_some());
    assert_eq!(
        identity(&engine, "shop", "1"),
        identity(&engine, "depot", "9")
    );
    assert!(identity(&engine, "shop", "3").is_some());
    assert_eq!(
        count(
            &engine,
            "SELECT count(*) FROM spool.observation o
             WHERE NOT EXISTS (SELECT 1 FROM spool.membership_current m
                               WHERE m.observation_id = o.observation_id)"
        ),
        0
    );

    // The old draft was overtaken, and says by which run.
    let summary = engine.draft(abandoned).unwrap().draft;
    assert_eq!(summary.status, DraftStatus::Superseded);
    assert_eq!(summary.superseded_by, Some(resolution.reconcile_run));
    let error = engine.approve_draft(abandoned).unwrap_err();
    assert!(
        matches!(&error, Error::Draft { message, .. } if message.contains("superseded")),
        "{error}"
    );
    let error = engine
        .commit(engine.draft(abandoned).unwrap().changeset)
        .unwrap_err();
    assert!(matches!(error, Error::Draft { .. }), "{error}");
    engine.verify().unwrap();
}

#[test]
fn a_type_has_at_most_one_open_draft() {
    let engine = engine();
    two_anvils_and_a_hammer(&engine);
    let first = engine.resolve(&leave_for_review()).unwrap().resolved[0]
        .draft
        .unwrap();
    ingest(&engine, "depot", &[("9", Some("anvil"), None)]);
    let second = engine.resolve(&leave_for_review()).unwrap();
    let second = &second.resolved[0];

    // The second draft covers everything the first did.
    assert_eq!(second.new_observations, 4);
    let open = engine.drafts(&DraftFilter::open()).unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(Some(open[0].id), second.draft);
    assert_eq!(
        engine.draft(first).unwrap().draft.status,
        DraftStatus::Superseded
    );
    assert_eq!(engine.drafts(&DraftFilter::default()).unwrap().len(), 2);
    assert_eq!(
        engine.info().unwrap().entity_types[0].open_draft,
        second.draft
    );
    engine.verify().unwrap();

    engine.approve_draft(second.draft.unwrap()).unwrap();
    assert_eq!(engine.info().unwrap().entity_types[0].unassigned, 0);
    engine.verify().unwrap();
}

#[test]
fn a_rejected_draft_leaves_its_observations_for_the_next_run() {
    let engine = engine();
    two_anvils_and_a_hammer(&engine);
    let first = engine.resolve(&leave_for_review()).unwrap();
    let draft = first.resolved[0].draft.unwrap();

    let summary = engine.reject_draft(draft, Some("not yet")).unwrap();
    assert_eq!(summary.status, DraftStatus::Rejected);
    assert_eq!(summary.rejected_reason.as_deref(), Some("not yet"));
    assert!(summary.rejected_at.is_some());
    assert_eq!(engine.head().unwrap().0, 0);
    // The run did not succeed, so nothing counts as examined.
    assert_eq!(
        engine.run(first.resolved[0].match_run).unwrap().status,
        RunStatus::Drafted
    );
    let error = engine.approve_draft(draft).unwrap_err();
    assert!(matches!(error, Error::Draft { .. }), "{error}");
    engine.verify().unwrap();

    let again = engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(again.resolved[0].new_observations, 3);
    assert_eq!(again.resolved[0].observations_assigned, 3);
    engine.verify().unwrap();
}

#[test]
fn a_stale_draft_stays_open_and_names_the_operation() {
    let engine = engine();
    two_anvils_and_a_hammer(&engine);
    let report = engine.resolve(&leave_for_review()).unwrap();
    let draft = report.resolved[0].draft.unwrap();

    // While the draft waits, someone places one of its observations.
    let hammer = observation(&engine, "shop", "3");
    let mut ids = SeededIds::new(99);
    let elsewhere = ids.identity_id();
    engine
        .commit(Changeset {
            id: ids.changeset_id(),
            base: engine.head().unwrap(),
            actor: Actor::User("reviewer".to_owned()),
            reason: None,
            run: None,
            ops: vec![
                Op::CreateIdentity {
                    identity: elsewhere,
                    entity_type: EntityType::new("thing").unwrap(),
                },
                Op::Assign {
                    observation: hammer,
                    from: None,
                    to: Some(elsewhere),
                },
            ],
        })
        .unwrap();

    let error = engine.approve_draft(draft).unwrap_err();
    let Error::Rejected(rejections) = &error else {
        panic!("expected a rejection, got {error}");
    };
    let ops = engine.draft(draft).unwrap().changeset.ops;
    assert!(rejections.iter().any(|rejection| {
        matches!(&rejection.reason, Reason::StaleAssignment { observation, .. } if *observation == hammer)
            && rejection.op_index.map(|index| &ops[index]) == rejection.op.as_ref()
    }));

    // Nothing was committed on its account, and it can still be rejected.
    assert_eq!(engine.head().unwrap().0, 1);
    assert_eq!(engine.draft(draft).unwrap().draft.status, DraftStatus::Open);
    assert_eq!(identity(&engine, "shop", "1"), None);
    engine.verify().unwrap();

    // The next run places what is left.
    let again = engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(again.resolved[0].new_observations, 2);
    assert_eq!(
        engine.draft(draft).unwrap().draft.status,
        DraftStatus::Superseded
    );
    assert_eq!(identity(&engine, "shop", "3"), Some(elsewhere));
    engine.verify().unwrap();
}

#[test]
fn the_step_by_step_api_stores_its_draft_and_commit_approves_it() {
    let engine = engine();
    two_anvils_and_a_hammer(&engine);
    let scope = Scope::new(EntityType::new("thing").unwrap());
    let run = engine
        .begin_run(&StrategyRef::external("by-hand", None), &scope)
        .unwrap();
    assert_eq!(engine.run(run).unwrap().status, RunStatus::Open);

    let pairs = engine.candidates(run).unwrap();
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
    engine.submit_judgements(run, judgements).unwrap();

    let draft = engine.reconcile(run, &Policy::default()).unwrap();
    let id = draft.changeset.id;
    assert_eq!(engine.run(run).unwrap().status, RunStatus::Drafted);
    assert_eq!(engine.draft(id).unwrap().changeset, draft.changeset);

    // A drafted run takes no more judgements, and can be reconciled again:
    // the new draft takes the place of the old one.
    let none = spool::Rows::from_text(&["left_id", "right_id", "verdict"], &[]).unwrap();
    let error = engine.submit_judgements(run, none).unwrap_err();
    assert!(matches!(error, Error::Run { .. }), "{error}");
    let again = engine.reconcile(run, &Policy::default()).unwrap();
    assert_ne!(again.changeset.id, id);
    assert_eq!(
        engine.draft(id).unwrap().draft.status,
        DraftStatus::Superseded
    );
    let id = again.changeset.id;

    // A changeset under the draft's ID has to be the draft.
    let mut altered = again.changeset.clone();
    altered.ops.pop();
    let error = engine.commit(altered).unwrap_err();
    assert!(matches!(error, Error::Draft { .. }), "{error}");
    assert_eq!(engine.head().unwrap().0, 0);

    let committed = engine.commit(again.changeset).unwrap();
    assert_eq!(
        engine.draft(id).unwrap().draft.committed_seq,
        Some(committed.seq)
    );
    assert_eq!(engine.run(run).unwrap().status, RunStatus::Succeeded);
    assert_eq!(
        engine.run(run).unwrap().resolution.unwrap().committed,
        Some(committed)
    );
    engine.verify().unwrap();
}

#[test]
fn a_run_can_be_reconciled_and_committed_in_one_call() {
    let engine = engine();
    two_anvils_and_a_hammer(&engine);
    let run = engine
        .begin_run(
            &StrategyRef::external("by-hand", None),
            &Scope::new(EntityType::new("thing").unwrap()),
        )
        .unwrap();
    let report = engine
        .reconcile_with(run, &Policy::default(), Disposition::Commit)
        .unwrap();
    // No judgements, so every observation stands alone.
    assert_eq!(report.identities_created, 3);
    assert_eq!(report.draft, None);
    assert!(report.committed.is_some());
    assert!(engine.drafts(&DraftFilter::default()).unwrap().is_empty());
    let record = engine.run(run).unwrap();
    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(record.resolution, Some(report));
    engine.verify().unwrap();
}

#[test]
fn an_unknown_draft_is_an_error_of_its_own() {
    let engine = engine();
    let id = SeededIds::new(7).changeset_id();
    for error in [
        engine.draft(id).map(|_| ()).unwrap_err(),
        engine.approve_draft(id).map(|_| ()).unwrap_err(),
        engine.reject_draft(id, None).map(|_| ()).unwrap_err(),
    ] {
        assert!(
            matches!(error, Error::UnknownDraft(unknown) if unknown == id),
            "{error}"
        );
    }
}

#[test]
fn drafts_survive_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    let draft = {
        let engine = Engine::open(&path, Options::seeded(1)).unwrap();
        engine.apply(&things()).unwrap();
        two_anvils_and_a_hammer(&engine);
        engine.resolve(&leave_for_review()).unwrap().resolved[0]
            .draft
            .unwrap()
    };
    let engine = Engine::open(&path, Options::seeded(2)).unwrap();
    // Approving needs no manifest: the draft is a changeset like any other.
    assert_eq!(engine.draft(draft).unwrap().draft.status, DraftStatus::Open);
    engine.approve_draft(draft).unwrap();
    assert_eq!(
        identity(&engine, "shop", "1"),
        identity(&engine, "shop", "2")
    );
    engine.verify().unwrap();
}
