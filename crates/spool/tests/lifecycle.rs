//! What happens to an identity after it exists: merging, splitting, the
//! constraints that keep decisions, undo, rows that change at their source,
//! and review items a reviewer closes.

mod common;

use common::*;
use spool::core::{
    Actor, Changeset, Committed, ConstraintKind, IdGenerator, IdentityId, ObservationId, Op,
    Reason, ReviewItem, SeededIds, Seq,
};
use spool::{
    ConstraintFilter, Engine, Error, ObservationFilter, Options, ResolveOptions, ResolveReport,
    ReviewFilter, ReviewKind, ReviewStatus,
};

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

fn constrain(kind: ConstraintKind, a: ObservationId, b: ObservationId) -> Op {
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
    ingest(
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
    ingest(&engine, "depot", &[("9", Some("tongs"), None)]);
    let tongs = observation(&engine, "depot", "9");
    let error = engine
        .author(
            reviewer(),
            None,
            vec![Op::Assign {
                observation: tongs,
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
fn merging_the_identities_of_a_review_item_lets_the_next_run_place_its_observation() {
    let engine = engine();
    // An anvil known by name, a thing known by code, and an observation that
    // has the name of one and the code of the other.
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", None, Some("K7"))],
    );
    resolve(&engine);
    ingest(&engine, "depot", &[("9", Some("anvil"), Some("K7"))]);
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
    // decision: the observation is placed by the run that looks at it next.
    author(&engine, vec![merge(by_name, by_code)]);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].observations_assigned, 1);
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
    ingest(&engine, "depot", &[("9", Some("anvil"), None)]);
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
    ingest(&engine, "depot", &[("9", Some("hammer"), None)]);
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
                observation: observation(&engine, "shop", "1"),
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

// Split

/// Three anvils the rules put in one identity.
fn three_anvils(engine: &Engine) -> IdentityId {
    ingest(
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

#[test]
fn a_split_gives_the_smaller_side_a_new_identity_and_keeps_the_sides_apart() {
    let engine = engine();
    let anvils = three_anvils(&engine);
    let one = observation(&engine, "shop", "1");

    let split = engine
        .split(
            anvils,
            &[one],
            true,
            reviewer(),
            Some("not ours".to_owned()),
        )
        .unwrap();

    assert_eq!(split.kept, anvils);
    assert_eq!(split.moved, [one]);
    assert_eq!(members(&engine, anvils), set(&["shop:2", "shop:3"]));
    assert_eq!(members(&engine, split.identity), set(&["shop:1"]));
    // One changeset holds the new identity, the move and the constraint.
    let kinds: Vec<_> = split.changeset.ops.iter().map(Op::kind).collect();
    assert_eq!(kinds, ["create_identity", "assign", "constrain"]);
    let constraints = engine.constraints(&ConstraintFilter::default()).unwrap();
    assert_eq!(constraints.len(), 1);
    assert_eq!(constraints[0].constraint.kind, ConstraintKind::CannotLink);
    assert_eq!(constraints[0].from, split.committed.seq);
    assert_eq!(
        engine
            .constraints(&ConstraintFilter {
                identity: Some(split.identity),
                ..Default::default()
            })
            .unwrap(),
        constraints
    );

    // Nobody merges them back without taking the constraint away first.
    let error = engine
        .author(reviewer(), None, vec![merge(split.identity, anvils)])
        .unwrap_err();
    assert!(matches!(
        rejections(error)[..],
        [Reason::ConstraintViolated { .. }]
    ));
    // And the rules do not either: a new anvil matches both sides, and is
    // left for review.
    ingest(&engine, "depot", &[("9", Some("anvil"), None)]);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].observations_assigned, 0);
    assert!(matches!(
        report.resolved[0].review[..],
        [ReviewItem::PossibleMerge { .. }]
    ));
    engine.verify().unwrap();
}

#[test]
fn the_larger_side_of_a_split_keeps_the_id_whichever_side_was_named() {
    let engine = engine();
    let anvils = three_anvils(&engine);
    let (two, three) = (
        observation(&engine, "shop", "2"),
        observation(&engine, "shop", "3"),
    );

    let split = engine
        .split(anvils, &[three, two], false, reviewer(), None)
        .unwrap();

    assert_eq!(split.moved, [observation(&engine, "shop", "1")]);
    assert_eq!(members(&engine, anvils), set(&["shop:2", "shop:3"]));
    assert_eq!(members(&engine, split.identity), set(&["shop:1"]));
    // Nothing was asked to keep them apart.
    assert_eq!(
        engine.constraints(&ConstraintFilter::default()).unwrap(),
        []
    );
    engine.verify().unwrap();
}

#[test]
fn a_split_that_cannot_be_made_says_why() {
    let engine = engine();
    let anvils = three_anvils(&engine);
    ingest(&engine, "depot", &[("9", Some("hammer"), None)]);
    resolve(&engine);
    let hammer = identity(&engine, "depot", "9").unwrap();
    let all: Vec<_> = ["1", "2", "3"]
        .iter()
        .map(|key| observation(&engine, "shop", key))
        .collect();
    let refused = |identity, observations: &[ObservationId]| -> String {
        match engine.split(identity, observations, true, reviewer(), None) {
            Err(error @ Error::Split { .. }) => error.to_string(),
            other => panic!("expected the split to be refused: {other:?}"),
        }
    };

    assert!(refused(anvils, &all).contains("every member was named"));
    assert!(refused(anvils, &[]).contains("no observations were named"));
    assert!(
        refused(anvils, &[observation(&engine, "depot", "9")]).contains("does not belong to it")
    );
    author(&engine, vec![merge(hammer, anvils)]);
    assert!(refused(hammer, &all[..1]).contains("was merged into"));
    let unknown = engine.mint_ids(1, 0).identities[0];
    assert!(matches!(
        engine.split(unknown, &all[..1], true, reviewer(), None),
        Err(Error::UnknownIdentity(_))
    ));
}

// Constraints and the runs that follow

#[test]
fn a_cannot_link_keeps_a_new_observation_out_of_the_identity_the_rules_chose() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();

    // A second anvil arrives, and before any run looks at it someone says it
    // is not the first one.
    ingest(&engine, "depot", &[("9", Some("anvil"), None)]);
    author(
        &engine,
        vec![constrain(
            ConstraintKind::CannotLink,
            observation(&engine, "depot", "9"),
            observation(&engine, "shop", "1"),
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
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    ingest(&engine, "depot", &[("9", Some("anvil"), None)]);
    author(
        &engine,
        vec![constrain(
            ConstraintKind::CannotLink,
            observation(&engine, "shop", "1"),
            observation(&engine, "depot", "9"),
        )],
    );

    // It matches the other member, so the evidence contradicts itself.
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].observations_assigned, 0);
    let items = engine.review_items(&ReviewFilter::open()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ReviewKind::Conflict);
    assert_eq!(identity(&engine, "depot", "9"), None);
    engine.verify().unwrap();
}

#[test]
fn a_must_link_places_a_new_observation_the_rules_would_not() {
    let engine = engine();
    let (anvil, hammer) = anvil_and_hammer(&engine);
    // Nothing about these tongs resembles the anvil.
    ingest(&engine, "depot", &[("9", Some("tongs"), None)]);
    let tongs = observation(&engine, "depot", "9");
    author(
        &engine,
        vec![constrain(
            ConstraintKind::MustLink,
            tongs,
            observation(&engine, "shop", "1"),
        )],
    );

    let report = resolve(&engine);
    assert_eq!(report.resolved[0].candidates, 0);
    assert_eq!(report.resolved[0].observations_assigned, 1);
    assert_eq!(identity(&engine, "depot", "9"), Some(anvil));

    // From then on the constraint holds the two together.
    let error = engine
        .author(
            reviewer(),
            None,
            vec![Op::Assign {
                observation: tongs,
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
                b: observation(&engine, "shop", "1"),
            },
            Op::Assign {
                observation: tongs,
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
            observation: Some(tongs),
            include_removed: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].to, Some(engine.head().unwrap()));
    engine.verify().unwrap();
}

// Rows that change at their source

#[test]
fn a_changed_row_supersedes_the_one_before_and_keeps_its_identity() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), Some("K1"))]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();
    let first = observation(&engine, "shop", "1");

    // The shop renames the row and gives it another code. No rule connects
    // the new version with the old one: the row's key does.
    let report = ingest(&engine, "shop", &[("1", Some("hammer"), Some("K2"))]);
    assert_eq!((report.inserted, report.superseded), (1, 1));
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].candidates, 0);
    assert_eq!(report.resolved[0].identities_created, 0);

    let versions = engine
        .observations(&ObservationFilter::keyed("shop", "1"))
        .unwrap();
    assert_eq!(versions.len(), 2);
    let second = versions.iter().find(|version| version.id != first).unwrap();
    assert_eq!(second.supersedes, Some(first));
    assert_eq!(second.identity, Some(anvil));
    let old = engine.observation(first).unwrap().unwrap();
    assert_eq!((old.supersedes, old.superseded_by), (None, Some(second.id)));
    // Both versions are evidence about the identity, and both stay.
    assert_eq!(engine.identity(anvil).unwrap().members.len(), 2);

    // The same content again is the same observation, and nothing new.
    let report = ingest(&engine, "shop", &[("1", Some("hammer"), Some("K2"))]);
    assert_eq!((report.inserted, report.superseded), (0, 0));
    engine.verify().unwrap();
}

#[test]
fn versions_sent_in_one_call_follow_each_other_in_row_order() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    let report = ingest(
        &engine,
        "shop",
        &[
            ("1", Some("anvil mk2"), None),
            ("7", Some("tongs"), None),
            ("1", Some("anvil mk3"), None),
        ],
    );
    assert_eq!((report.inserted, report.superseded), (3, 2));
    // The same key at another source is another row.
    let report = ingest(&engine, "depot", &[("1", Some("bellows"), None)]);
    assert_eq!(report.superseded, 0);

    let chain = select(
        &engine,
        "SELECT o.payload->>'name', p.payload->>'name'
         FROM spool.observation o
         LEFT JOIN spool.observation p ON p.observation_id = o.supersedes
         ORDER BY o.source_id, o.source_key, o.payload->>'name'",
    );
    let chain: Vec<(&str, Option<&str>)> = chain
        .iter()
        .map(|row| (row[0].as_deref().unwrap(), row[1].as_deref()))
        .collect();
    assert_eq!(
        chain,
        [
            ("bellows", None),
            ("anvil", None),
            ("anvil mk2", Some("anvil")),
            ("anvil mk3", Some("anvil mk2")),
            ("tongs", None),
        ]
    );

    // All three versions were new to the run, and they are one thing.
    resolve(&engine);
    let versions = observations(&engine, "shop", "1");
    let identities = engine.identities_of(&versions).unwrap();
    assert!(identities[0].is_some());
    assert!(identities.iter().all(|found| *found == identities[0]));
    engine.verify().unwrap();
}

#[test]
fn a_new_version_that_matches_another_identity_is_left_for_review() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("hammer"), None)],
    );
    resolve(&engine);
    let (anvil, hammer) = (
        identity(&engine, "shop", "1").unwrap(),
        identity(&engine, "shop", "2").unwrap(),
    );

    // Row 1 now reads like row 2. Its key says it is still the anvil, the
    // rules say it is the hammer, and deciding would merge the two.
    ingest(&engine, "shop", &[("1", Some("hammer"), None)]);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].observations_assigned, 0);
    let items = engine.review_items(&ReviewFilter::open()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ReviewKind::PossibleMerge);
    assert_eq!(items[0].identities.len(), 2);
    assert!(items[0].identities.contains(&anvil) && items[0].identities.contains(&hammer));
    engine.verify().unwrap();
}

// Review items a reviewer closes

#[test]
fn a_dismissed_review_item_is_closed_and_not_examined_again() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", None, Some("K7"))],
    );
    resolve(&engine);
    ingest(&engine, "depot", &[("9", Some("anvil"), Some("K7"))]);
    resolve(&engine);
    let waiting = observation(&engine, "depot", "9");
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
    // Nothing was committed: the observation has no identity, by decision.
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

// The store

#[test]
fn every_kind_of_change_survives_a_rebuild_and_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool.duckdb");
    let before = {
        let engine = Engine::open(&path, Options::seeded(1)).unwrap();
        engine.apply(&things()).unwrap();
        let anvils = three_anvils(&engine);
        ingest(
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
        let one = observation(&engine, "shop", "1");
        engine
            .split(anvils, &[one], true, reviewer(), None)
            .unwrap();
        let pair = (
            observation(&engine, "depot", "8"),
            observation(&engine, "depot", "9"),
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
        ingest(&engine, "shop", &[("2", Some("anvil, large"), None)]);
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
