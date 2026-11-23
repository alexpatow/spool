//! What happens to an identity after it exists: merging, splitting, the
//! constraints that keep decisions, undo, rows that change at their source,
//! and review items a reviewer closes.

use spool::core::{
    Actor, Changeset, Committed, ConstraintKind, IdGenerator, IdentityId, Op, Reason, RecordId,
    ReviewItem, SeededIds, Seq,
};
use spool::{
    ConstraintFilter, Engine, Error, Options, RecordFilter, ResolveOptions, ResolveReport,
    ReviewFilter, ReviewKind, ReviewStatus, TypeResolution,
};
use spool_testing::*;

mod moved_along;
mod split;
mod versions;

fn resolve(engine: &Engine) -> ResolveReport {
    engine.resolve(&ResolveOptions::default()).unwrap()
}

fn reviewer() -> Actor {
    Actor::User("reviewer".to_owned())
}

fn author(engine: &Engine, ops: Vec<Op>) -> Committed {
    engine
        .author(reviewer(), Some("reviewed".to_owned()), ops)
        .unwrap()
        .committed
        .unwrap()
}

fn merge(from: IdentityId, into: IdentityId) -> Op {
    Op::Merge { from, into }
}

fn constrain(kind: ConstraintKind, a: RecordId, b: RecordId) -> Op {
    Op::Constrain { kind, a, b }
}

fn rejections(error: Error) -> Vec<Reason> {
    match error {
        Error::Rejected(rejections) => rejections.into_iter().map(|r| r.reason).collect(),
        other => panic!("expected a rejected changeset, not: {other}"),
    }
}

fn set(members: &[&str]) -> std::collections::BTreeSet<String> {
    members.iter().map(|member| (*member).to_owned()).collect()
}

/// An anvil and a hammer from the shop: two identities.
fn anvil_and_hammer(engine: &Engine) -> (IdentityId, IdentityId) {
    load(
        engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("hammer"), None)],
    );
    resolve(engine);
    (
        identity(engine, "shop", "1").unwrap(),
        identity(engine, "shop", "2").unwrap(),
    )
}

// Merge

#[test]
fn a_merge_moves_the_members_and_the_merged_id_still_answers() {
    let engine = engine();
    let (anvil, hammer) = anvil_and_hammer(&engine);

    let committed = author(&engine, vec![merge(anvil, hammer)]);

    assert_eq!(members(&engine, hammer), set(&["shop:1", "shop:2"]));
    assert_eq!(identity(&engine, "shop", "1"), Some(hammer));
    // Whoever kept the merged ID is told which identity stands for it.
    assert_eq!(engine.current_id(anvil).unwrap(), hammer);
    let view = engine.identity(anvil).unwrap();
    assert_eq!((view.id, view.current), (anvil, hammer));
    assert_eq!(view.members.len(), 2);
    // It is no longer listed, and it keeps its history.
    let listed = engine.identities(&Default::default()).unwrap();
    assert_eq!(
        listed.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        [hammer]
    );
    assert_eq!(engine.info().unwrap().entity_types[0].identities, 1);
    let history = engine.history(anvil).unwrap();
    assert_eq!(history.last().unwrap().seq, committed.seq);
    assert_eq!(history.last().unwrap().ops, [merge(anvil, hammer)]);
    assert_eq!(history.last().unwrap().actor, reviewer());
    engine.verify().unwrap();

    // Nothing joins a merged identity.
    load(&engine, "depot", &[("9", Some("tongs"), None)]);
    let tongs = record_id(&engine, "depot", "9");
    let error = engine
        .author(
            reviewer(),
            None,
            vec![Op::Assign {
                record: tongs,
                from: None,
                to: Some(anvil),
            }],
        )
        .unwrap_err();
    assert_eq!(
        rejections(error),
        [Reason::IdentityNotActive {
            identity: anvil,
            merged_into: hammer
        }]
    );
}

#[test]
fn merging_the_identities_of_a_review_item_lets_the_next_run_place_its_record() {
    let engine = engine();
    // An anvil known by name, a thing known by code, and a record that
    // has the name of one and the code of the other.
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", None, Some("K7"))],
    );
    resolve(&engine);
    load(&engine, "depot", &[("9", Some("anvil"), Some("K7"))]);
    let report = resolve(&engine);
    let (by_name, by_code) = (
        identity(&engine, "shop", "1").unwrap(),
        identity(&engine, "shop", "2").unwrap(),
    );
    assert!(matches!(
        report.resolved[0].review[..],
        [ReviewItem::PossibleMerge { .. }]
    ));

    // The reviewer decides they are one thing. The merge is the whole
    // decision: the record is placed by the run that looks at it next.
    author(&engine, vec![merge(by_name, by_code)]);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].records_assigned, 1);
    assert_eq!(
        members(&engine, by_code),
        set(&["shop:1", "shop:2", "depot:9"])
    );
    let items = engine.review_items(&ReviewFilter::default()).unwrap();
    assert_eq!(items[0].status, ReviewStatus::Resolved);
    engine.verify().unwrap();
}

#[test]
fn a_merge_written_before_a_member_arrived_is_rejected() {
    let engine = engine();
    let (anvil, hammer) = anvil_and_hammer(&engine);

    // The reviewer reads the two identities and writes the merge.
    let written = Changeset {
        id: SeededIds::new(9).changeset_id(),
        base: engine.head().unwrap(),
        actor: reviewer(),
        reason: None,
        run: None,
        ops: vec![merge(anvil, hammer)],
    };
    // Before it is committed, the anvil gains a member the reviewer has not
    // seen.
    load(&engine, "depot", &[("9", Some("anvil"), None)]);
    resolve(&engine);
    let changed = engine.head().unwrap();

    let error = engine.commit(written.clone()).unwrap_err();
    assert_eq!(
        rejections(error),
        [Reason::StaleMerge {
            identity: anvil,
            changed,
            base: written.base
        }]
    );
    // Written again against what is there now, it commits.
    author(&engine, written.ops);
    assert_eq!(
        members(&engine, hammer),
        set(&["shop:1", "shop:2", "depot:9"])
    );
}

// Undo

#[test]
fn undoing_a_merge_brings_the_identity_back_with_the_members_it_had() {
    let engine = engine();
    let (anvil, hammer) = anvil_and_hammer(&engine);
    let merged = author(&engine, vec![merge(anvil, hammer)]).seq;
    // After the merge, the survivor gains a member of its own.
    load(&engine, "depot", &[("9", Some("hammer"), None)]);
    resolve(&engine);
    assert_eq!(
        members(&engine, hammer),
        set(&["shop:1", "shop:2", "depot:9"])
    );

    // Looking at the undo first changes nothing.
    let head = engine.head().unwrap();
    let preview = engine.undo(merged, reviewer(), None, false).unwrap();
    assert_eq!(preview.committed, None);
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(
        preview.changeset.ops,
        [
            Op::Reinstate { identity: anvil },
            Op::Assign {
                record: record_id(&engine, "shop", "1"),
                from: Some(hammer),
                to: Some(anvil)
            }
        ]
    );

    let undone = engine.undo(merged, reviewer(), None, true).unwrap();
    assert_eq!(undone.changeset.ops, preview.changeset.ops);
    assert_eq!(
        undone.changeset.reason.as_deref(),
        Some(format!("undo of the changeset at position {merged}").as_str())
    );
    assert_eq!(engine.current_id(anvil).unwrap(), anvil);
    assert_eq!(members(&engine, anvil), set(&["shop:1"]));
    assert_eq!(members(&engine, hammer), set(&["shop:2", "depot:9"]));
    engine.verify().unwrap();

    // The undo is a changeset like any other, so it can be undone too.
    let again = undone.committed.unwrap().seq;
    engine.undo(again, reviewer(), None, true).unwrap();
    assert_eq!(engine.current_id(anvil).unwrap(), hammer);
    assert_eq!(
        members(&engine, hammer),
        set(&["shop:1", "shop:2", "depot:9"])
    );
    engine.verify().unwrap();
}

#[test]
fn an_undo_that_cannot_be_made_says_why() {
    let engine = engine();
    let (anvil, hammer) = anvil_and_hammer(&engine);

    assert!(matches!(
        engine.undo(Seq(0), reviewer(), None, true),
        Err(Error::UnknownChangeset(Seq(0)))
    ));
    assert!(matches!(
        engine.undo(Seq(99), reviewer(), None, true),
        Err(Error::UnknownChangeset(Seq(99)))
    ));

    // A changeset that only made an identity has nothing to take back.
    let made = engine.mint_ids(1, 0).identities[0];
    let created = author(
        &engine,
        vec![Op::CreateIdentity {
            identity: made,
            entity_type: spool::core::EntityType::new("thing").unwrap(),
        }],
    );
    assert!(matches!(
        engine.undo(created.seq, reviewer(), None, true),
        Err(Error::NothingToUndo(seq)) if seq == created.seq
    ));

    // The first run placed the anvil. Once the anvil's identity is merged
    // away, taking that run back would move the anvil out of an identity it
    // is no longer in.
    author(&engine, vec![merge(anvil, hammer)]);
    let error = engine.undo(Seq(1), reviewer(), None, true).unwrap_err();
    assert!(
        rejections(error)
            .iter()
            .any(|reason| matches!(reason, Reason::StaleAssignment { .. }))
    );
    engine.verify().unwrap();
}

/// Three anvils the rules put in one identity.
fn three_anvils(engine: &Engine) -> IdentityId {
    load(
        engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", Some("anvil"), None),
        ],
    );
    resolve(engine);
    identity(engine, "shop", "1").unwrap()
}

// Constraints and the runs that follow

#[test]
fn a_cannot_link_keeps_a_new_record_out_of_the_identity_the_rules_chose() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();

    // A second anvil arrives, and before any run looks at it someone says it
    // is not the first one.
    load(&engine, "depot", &[("9", Some("anvil"), None)]);
    author(
        &engine,
        vec![constrain(
            ConstraintKind::CannotLink,
            record_id(&engine, "depot", "9"),
            record_id(&engine, "shop", "1"),
        )],
    );

    let report = resolve(&engine);
    // The rules still say the two match. The constraint speaks for the pair.
    assert_eq!(report.resolved[0].judgements.match_, 1);
    assert_eq!(report.resolved[0].identities_created, 1);
    let other = identity(&engine, "depot", "9").unwrap();
    assert_ne!(other, anvil);
    assert_eq!(members(&engine, anvil), set(&["shop:1"]));
    engine.verify().unwrap();
}

#[test]
fn a_cannot_link_against_one_member_of_a_matched_identity_is_left_for_review() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    load(&engine, "depot", &[("9", Some("anvil"), None)]);
    author(
        &engine,
        vec![constrain(
            ConstraintKind::CannotLink,
            record_id(&engine, "shop", "1"),
            record_id(&engine, "depot", "9"),
        )],
    );

    // It matches the other member, so the evidence contradicts itself.
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].records_assigned, 0);
    let items = engine.review_items(&ReviewFilter::open()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ReviewKind::Conflict);
    assert_eq!(identity(&engine, "depot", "9"), None);
    engine.verify().unwrap();
}

#[test]
fn a_must_link_places_a_new_record_the_rules_would_not() {
    let engine = engine();
    let (anvil, hammer) = anvil_and_hammer(&engine);
    // Nothing about these tongs resembles the anvil.
    load(&engine, "depot", &[("9", Some("tongs"), None)]);
    let tongs = record_id(&engine, "depot", "9");
    author(
        &engine,
        vec![constrain(
            ConstraintKind::MustLink,
            tongs,
            record_id(&engine, "shop", "1"),
        )],
    );

    let report = resolve(&engine);
    assert_eq!(report.resolved[0].candidates, 0);
    assert_eq!(report.resolved[0].records_assigned, 1);
    assert_eq!(identity(&engine, "depot", "9"), Some(anvil));

    // From then on the constraint holds the two together.
    let error = engine
        .author(
            reviewer(),
            None,
            vec![Op::Assign {
                record: tongs,
                from: Some(anvil),
                to: Some(hammer),
            }],
        )
        .unwrap_err();
    assert!(matches!(
        rejections(error)[..],
        [Reason::ConstraintViolated {
            kind: ConstraintKind::MustLink,
            ..
        }]
    ));
    // Removing it is a changeset too, and its history is kept.
    author(
        &engine,
        vec![
            Op::Unconstrain {
                kind: ConstraintKind::MustLink,
                a: tongs,
                b: record_id(&engine, "shop", "1"),
            },
            Op::Assign {
                record: tongs,
                from: Some(anvil),
                to: Some(hammer),
            },
        ],
    );
    assert_eq!(
        engine.constraints(&ConstraintFilter::default()).unwrap(),
        []
    );
    let removed = engine
        .constraints(&ConstraintFilter {
            record: Some(tongs),
            include_removed: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].to, Some(engine.head().unwrap()));
    engine.verify().unwrap();
}

/// The names of a row's versions, from the one nothing replaced back to
/// the first.
fn history(engine: &Engine, source: &str, key: &str) -> Vec<String> {
    let versions = engine.records(&RecordFilter::keyed(source, key)).unwrap();
    let mut at = versions
        .iter()
        .find(|version| version.superseded_by.is_none());
    let mut names = Vec::new();
    while let Some(version) = at {
        names.push(version.payload["name"].as_str().unwrap().to_owned());
        at = versions
            .iter()
            .find(|earlier| Some(earlier.id) == version.supersedes);
    }
    assert_eq!(names.len(), versions.len(), "the versions are one chain");
    names
}

// Review items a reviewer closes

#[test]
fn a_dismissed_review_item_is_closed_and_not_examined_again() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", None, Some("K7"))],
    );
    resolve(&engine);
    load(&engine, "depot", &[("9", Some("anvil"), Some("K7"))]);
    resolve(&engine);
    let waiting = record_id(&engine, "depot", "9");
    let item = engine
        .review_items(&ReviewFilter::open())
        .unwrap()
        .remove(0);

    let head = engine.head().unwrap();
    let dismissed = engine
        .dismiss_review(item.id, &reviewer(), Some("a test record"))
        .unwrap();

    assert_eq!(dismissed.status, ReviewStatus::Dismissed);
    let decision = dismissed.dismissed.as_ref().unwrap();
    assert_eq!(decision.by, reviewer());
    assert_eq!(decision.reason.as_deref(), Some("a test record"));
    assert!(decision.at.starts_with("2026-01-01T"));
    // Nothing was committed: the record has no identity, by decision.
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(engine.identity_of(waiting).unwrap(), None);
    assert_eq!(engine.review_items(&ReviewFilter::open()).unwrap(), []);
    // There is nothing left for a run to look at.
    assert_eq!(resolve(&engine).resolved, []);

    // It is closed for good.
    assert!(matches!(
        engine.dismiss_review(item.id, &reviewer(), None),
        Err(Error::Review { id, .. }) if id == item.id
    ));
    assert!(matches!(
        engine.dismiss_review(99, &reviewer(), None),
        Err(Error::UnknownReviewItem(99))
    ));
    engine.verify().unwrap();
}

#[test]
fn a_dismissed_review_item_stays_closed_when_its_run_never_committed() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", None, Some("K7"))],
    );
    resolve(&engine);
    // One record fits both identities, and one is new. The run's draft is
    // left for review, so the run has not succeeded and both records are
    // still new to the next one.
    load(
        &engine,
        "depot",
        &[
            ("9", Some("anvil"), Some("K7")),
            ("10", Some("tongs"), None),
        ],
    );
    let drafted = engine
        .resolve(&ResolveOptions {
            leave_for_review: true,
            ..ResolveOptions::default()
        })
        .unwrap();
    let draft = drafted.resolved[0].draft.unwrap();
    let item = engine
        .review_items(&ReviewFilter::open())
        .unwrap()
        .remove(0);
    engine.dismiss_review(item.id, &reviewer(), None).unwrap();
    engine.reject_draft(draft, None).unwrap();

    // The next run places the new record and does not ask again about the
    // one the reviewer dismissed.
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].new_records, 1);
    assert_eq!(report.resolved[0].review, []);
    assert!(identity(&engine, "depot", "10").is_some());
    assert_eq!(identity(&engine, "depot", "9"), None);
    assert_eq!(engine.review_items(&ReviewFilter::open()).unwrap(), []);
    engine.verify().unwrap();
}

// The store

#[test]
fn every_kind_of_change_survives_a_rebuild_and_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    let before = {
        let engine = Engine::open(&path, Options::seeded(1)).unwrap();
        engine.apply(&things()).unwrap();
        let anvils = three_anvils(&engine);
        load(
            &engine,
            "depot",
            &[("8", Some("hammer"), None), ("9", Some("tongs"), None)],
        );
        resolve(&engine);
        let (hammer, tongs) = (
            identity(&engine, "depot", "8").unwrap(),
            identity(&engine, "depot", "9").unwrap(),
        );

        // A chain of merges, a reinstatement, a split with its constraint, a
        // constraint that is removed again, and a row that changed.
        author(&engine, vec![merge(hammer, tongs)]);
        let merged = author(&engine, vec![merge(tongs, anvils)]).seq;
        assert_eq!(engine.current_id(hammer).unwrap(), anvils);
        engine.undo(merged, reviewer(), None, true).unwrap();
        assert_eq!(engine.current_id(hammer).unwrap(), tongs);
        let one = record_id(&engine, "shop", "1");
        engine
            .split(anvils, &[one], true, None, reviewer(), None)
            .unwrap();
        let pair = (
            record_id(&engine, "depot", "8"),
            record_id(&engine, "depot", "9"),
        );
        author(
            &engine,
            vec![constrain(ConstraintKind::MustLink, pair.0, pair.1)],
        );
        author(
            &engine,
            vec![Op::Unconstrain {
                kind: ConstraintKind::MustLink,
                a: pair.1,
                b: pair.0,
            }],
        );
        load(&engine, "shop", &[("2", Some("anvil, large"), None)]);
        resolve(&engine);
        engine.verify().unwrap();

        // The projection is the log applied, so it can be thrown away.
        let snapshot = engine.snapshot().unwrap();
        assert_eq!(snapshot.constraints.len(), 2);
        engine.rebuild_projection().unwrap();
        assert_eq!(engine.snapshot().unwrap(), snapshot);
        engine.verify().unwrap();
        (snapshot, engine.log().unwrap())
    };

    let engine = Engine::open(&path, Options::seeded(2)).unwrap();
    assert_eq!(engine.snapshot().unwrap(), before.0);
    assert_eq!(engine.log().unwrap(), before.1);
    engine.verify().unwrap();
}
