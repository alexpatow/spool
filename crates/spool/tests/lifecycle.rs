//! What happens to an identity after it exists: merging, splitting, the
//! constraints that keep decisions, undo, rows that change at their source,
//! and review items a reviewer closes.

mod common;

use common::*;
use spool::core::{
    Actor, Changeset, Committed, ConstraintKind, IdGenerator, IdentityId, Op, Reason, RecordId,
    ReviewItem, SeededIds, Seq,
};
use spool::{
    ConstraintFilter, Engine, Error, Options, RecordFilter, ResolveOptions, ResolveReport,
    ReviewFilter, ReviewKind, ReviewStatus, TypeResolution,
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

// Split

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

#[test]
fn a_split_gives_the_smaller_side_a_new_identity_and_keeps_the_sides_apart() {
    let engine = engine();
    let anvils = three_anvils(&engine);
    let one = record_id(&engine, "shop", "1");

    let split = engine
        .split(
            anvils,
            &[one],
            true,
            None,
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
    load(&engine, "depot", &[("9", Some("anvil"), None)]);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].records_assigned, 0);
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
        record_id(&engine, "shop", "2"),
        record_id(&engine, "shop", "3"),
    );

    let split = engine
        .split(anvils, &[three, two], false, None, reviewer(), None)
        .unwrap();

    assert_eq!(split.moved, [record_id(&engine, "shop", "1")]);
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
    load(&engine, "depot", &[("9", Some("hammer"), None)]);
    resolve(&engine);
    let hammer = identity(&engine, "depot", "9").unwrap();
    let all: Vec<_> = ["1", "2", "3"]
        .iter()
        .map(|key| record_id(&engine, "shop", key))
        .collect();
    let refused = |identity, records: &[RecordId]| -> String {
        match engine.split(identity, records, true, None, reviewer(), None) {
            Err(error @ Error::Split { .. }) => error.to_string(),
            other => panic!("expected the split to be refused: {other:?}"),
        }
    };

    assert!(refused(anvils, &all).contains("every member was named"));
    assert!(refused(anvils, &[]).contains("no records were named"));
    assert!(refused(anvils, &[record_id(&engine, "depot", "9")]).contains("does not belong to it"));
    author(&engine, vec![merge(hammer, anvils)]);
    assert!(refused(hammer, &all[..1]).contains("was merged into"));
    let unknown = engine.mint_ids(1, 0).identities[0];
    assert!(matches!(
        engine.split(unknown, &all[..1], true, None, reviewer(), None),
        Err(Error::UnknownIdentity(_))
    ));
}

#[test]
fn a_split_based_on_a_log_position_is_refused_when_the_members_changed_after_it() {
    let engine = engine();
    let anvils = three_anvils(&engine);
    let (two, three) = (
        record_id(&engine, "shop", "2"),
        record_id(&engine, "shop", "3"),
    );
    // Whoever asks for the split read three members: the two they name are
    // the larger side, and would keep the ID.
    let read = engine.head().unwrap();
    load(&engine, "depot", &[("9", Some("hammer"), None)]);
    resolve(&engine);
    let hammer = identity(&engine, "depot", "9").unwrap();
    let elsewhere = engine.head().unwrap();
    let refused = |base| -> String {
        match engine.split(anvils, &[two, three], true, Some(base), reviewer(), None) {
            Err(error @ Error::Split { .. }) => error.to_string(),
            other => panic!("expected the split to be refused: {other:?}"),
        }
    };

    // A fourth member arrives. The two named are now half, and would move.
    let grown = author(&engine, vec![merge(hammer, anvils)]).seq;
    assert_eq!(
        refused(read),
        format!(
            "identity {anvils} cannot be split: its members changed at log position {grown}, after log position {read} that the split is based on"
        )
    );
    // So is one based on any later position before the member arrived.
    assert!(refused(elsewhere).contains(&format!("changed at log position {grown}")));
    assert!(refused(Seq(grown.0 + 1)).contains(&format!(
        "the split is based on log position {}, and the log ends at {grown}",
        grown.0 + 1
    )));
    assert_eq!(engine.head().unwrap(), grown);
    assert_eq!(members(&engine, anvils).len(), 4);

    // Based on what the identity holds now, the split is made.
    let split = engine
        .split(anvils, &[two, three], true, Some(grown), reviewer(), None)
        .unwrap();
    assert_eq!(split.moved, [two, three]);
    engine.verify().unwrap();
}

#[test]
fn a_split_based_on_a_log_position_is_made_when_only_other_identities_changed_after_it() {
    let engine = engine();
    let anvils = three_anvils(&engine);
    let read = engine.head().unwrap();
    load(&engine, "depot", &[("9", Some("hammer"), None)]);
    resolve(&engine);
    assert!(engine.head().unwrap() > read);

    let split = engine
        .split(
            anvils,
            &[record_id(&engine, "shop", "1")],
            false,
            Some(read),
            reviewer(),
            None,
        )
        .unwrap();
    assert_eq!(split.moved, [record_id(&engine, "shop", "1")]);
    engine.verify().unwrap();
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

// Rows that change at their source

#[test]
fn a_changed_row_supersedes_the_one_before_and_keeps_its_identity() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), Some("K1"))]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();
    let first = record_id(&engine, "shop", "1");

    // The shop renames the row and gives it another code. No rule connects
    // the new version with the old one: the row's key does.
    let report = load(&engine, "shop", &[("1", Some("hammer"), Some("K2"))]);
    assert_eq!((report.inserted, report.superseded), (1, 1));
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].candidates, 0);
    assert_eq!(report.resolved[0].identities_created, 0);

    let versions = engine.records(&RecordFilter::keyed("shop", "1")).unwrap();
    assert_eq!(versions.len(), 2);
    let second = versions.iter().find(|version| version.id != first).unwrap();
    assert_eq!(second.supersedes, Some(first));
    assert_eq!(second.identity, Some(anvil));
    let old = engine.record(first).unwrap().unwrap();
    assert_eq!((old.supersedes, old.superseded_by), (None, Some(second.id)));
    // Both versions are evidence about the identity, and both stay.
    assert_eq!(engine.identity(anvil).unwrap().members.len(), 2);

    // The same content again is the same record, and nothing new.
    let report = load(&engine, "shop", &[("1", Some("hammer"), Some("K2"))]);
    assert_eq!((report.inserted, report.superseded), (0, 0));
    engine.verify().unwrap();
}

#[test]
fn versions_sent_in_one_call_follow_each_other_in_row_order() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    let report = load(
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
    let report = load(&engine, "depot", &[("1", Some("bellows"), None)]);
    assert_eq!(report.superseded, 0);

    let chain = select(
        &engine,
        "SELECT o.payload->>'name', p.payload->>'name'
         FROM spool.record o
         LEFT JOIN spool.record p ON p.record_id = o.supersedes
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
    let versions = records(&engine, "shop", "1");
    let identities = engine.identities_of(&versions).unwrap();
    assert!(identities[0].is_some());
    assert!(identities.iter().all(|found| *found == identities[0]));
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

#[test]
fn a_row_that_changes_back_is_what_it_was_again_and_keeps_its_identity() {
    let engine = engine();
    let anvil = [("1", Some("anvil"), Some("K1"))];
    let hammer = [("1", Some("hammer"), Some("K2"))];
    load(&engine, "shop", &anvil);
    resolve(&engine);
    let identity = identity(&engine, "shop", "1").unwrap();
    let first = record_id(&engine, "shop", "1");
    load(&engine, "shop", &hammer);
    resolve(&engine);
    let second = engine
        .record(first)
        .unwrap()
        .unwrap()
        .superseded_by
        .unwrap();

    // The shop has the row as it was at first. Its content is that of a
    // record the store holds, and the row is that record's no longer: the
    // row has a third version, which says what the first said.
    let report = load(&engine, "shop", &anvil);
    assert_eq!(
        (report.inserted, report.duplicates, report.superseded),
        (1, 0, 1)
    );
    assert_eq!(history(&engine, "shop", "1"), ["anvil", "hammer", "anvil"]);
    let third = engine
        .record(second)
        .unwrap()
        .unwrap()
        .superseded_by
        .unwrap();
    let entry = engine.record(third).unwrap().unwrap();
    assert_eq!(
        entry.payload,
        engine.record(first).unwrap().unwrap().payload
    );
    assert_eq!(
        (entry.supersedes, entry.superseded_by),
        (Some(second), None)
    );
    // Its ID is the content's and the ID of what it follows, so it is the
    // same in any store with this history.
    assert_eq!(third, first.held_again(second));
    // The first version is as it was: one version replaced it.
    let entry = engine.record(first).unwrap().unwrap();
    assert_eq!(
        (entry.supersedes, entry.superseded_by),
        (None, Some(second))
    );

    // Like any version, it joins the identity of the one before it.
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].new_records, 1);
    assert_eq!(report.resolved[0].identities_created, 0);
    assert_eq!(
        engine.identities_of(&[first, second, third]).unwrap(),
        [Some(identity); 3]
    );
    assert_eq!(engine.identity(identity).unwrap().members.len(), 3);

    // What the row holds now, sent again, is nothing new.
    let report = load(&engine, "shop", &anvil);
    assert_eq!((report.inserted, report.duplicates), (0, 1));
    // And the row can change the same way again.
    let report = load(&engine, "shop", &hammer);
    assert_eq!((report.inserted, report.superseded), (1, 1));
    assert_eq!(
        history(&engine, "shop", "1"),
        ["hammer", "anvil", "hammer", "anvil"]
    );
    let report = load(&engine, "shop", &hammer);
    assert_eq!((report.inserted, report.duplicates), (0, 1));
    resolve(&engine);
    assert_eq!(engine.identity(identity).unwrap().members.len(), 4);
    engine.verify().unwrap();
}

#[test]
fn a_row_that_changes_back_within_one_call_ends_as_the_last_row_says() {
    let engine = engine();
    // Two rows in a row that say the same are one version.
    let report = load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("1", Some("hammer"), None),
            ("1", Some("hammer"), None),
            ("1", Some("anvil"), None),
            ("2", Some("tongs"), None),
        ],
    );
    assert_eq!(
        (report.received, report.inserted, report.duplicates),
        (5, 4, 1)
    );
    assert_eq!(report.superseded, 2);
    assert_eq!(history(&engine, "shop", "1"), ["anvil", "hammer", "anvil"]);

    // The store holds the hammer as a version, and the call begins with it:
    // that is a change, and the anvil after it is what the row holds.
    let report = load(
        &engine,
        "shop",
        &[("1", Some("hammer"), None), ("1", Some("anvil"), None)],
    );
    assert_eq!((report.inserted, report.superseded), (2, 2));
    assert_eq!(
        history(&engine, "shop", "1"),
        ["anvil", "hammer", "anvil", "hammer", "anvil"]
    );

    resolve(&engine);
    let versions = records(&engine, "shop", "1");
    let identities = engine.identities_of(&versions).unwrap();
    assert!(identities[0].is_some());
    assert!(identities.iter().all(|found| *found == identities[0]));
    engine.verify().unwrap();
}

#[test]
fn a_row_without_a_key_has_no_versions_and_is_stored_once() {
    let engine = engine();
    let keyless = |name: &'static str| -> spool::Rows {
        spool::Rows::from_text(&["name"], &[vec![Some(name)]]).unwrap()
    };
    for (name, inserted) in [("anvil", 1), ("hammer", 1), ("anvil", 0)] {
        let report = engine.load("shop", "thing", keyless(name)).unwrap();
        assert_eq!(report.inserted, inserted, "{name}");
        assert_eq!(report.superseded, 0);
    }
    assert_eq!(count(&engine, "SELECT count(*) FROM spool.record"), 2);
}

#[test]
fn a_new_version_that_matches_another_identity_is_left_for_review() {
    let engine = engine();
    load(
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
    load(&engine, "shop", &[("1", Some("hammer"), None)]);
    let report = resolve(&engine);
    assert_eq!(report.resolved[0].records_assigned, 0);
    let items = engine.review_items(&ReviewFilter::open()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, ReviewKind::PossibleMerge);
    assert_eq!(items[0].identities.len(), 2);
    assert!(items[0].identities.contains(&anvil) && items[0].identities.contains(&hammer));
    engine.verify().unwrap();
}

/// The versions of `shop:1` from the first, and the identity of each.
fn placed_versions(engine: &Engine) -> (Vec<RecordId>, Vec<Option<IdentityId>>) {
    let versions = records(engine, "shop", "1");
    let identities = engine.identities_of(&versions).unwrap();
    (versions, identities)
}

#[test]
fn a_reviewer_who_places_a_new_version_takes_its_earlier_versions_along() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("hammer"), None)],
    );
    resolve(&engine);
    let (anvil, hammer) = (
        identity(&engine, "shop", "1").unwrap(),
        identity(&engine, "shop", "2").unwrap(),
    );
    load(&engine, "shop", &[("1", Some("hammer"), None)]);
    resolve(&engine);
    let (versions, _) = placed_versions(&engine);
    let (first, second) = (versions[0], versions[1]);

    // The reviewer says the row is the hammer. The version before it says
    // what the same row said earlier, so it goes too, and leaves the anvil
    // with nothing.
    let committed = author(
        &engine,
        vec![Op::Assign {
            record: second,
            from: None,
            to: Some(hammer),
        }],
    );
    let entry = engine.log().unwrap().pop().unwrap();
    assert_eq!(entry.seq, committed.seq);
    assert_eq!(
        entry.changeset.ops,
        [
            Op::Assign {
                record: second,
                from: None,
                to: Some(hammer),
            },
            Op::Assign {
                record: first,
                from: Some(anvil),
                to: Some(hammer),
            },
        ]
    );
    assert_eq!(placed_versions(&engine).1, [Some(hammer); 2]);
    assert_eq!(members(&engine, anvil), set(&[]));
    assert_eq!(members(&engine, hammer), set(&["shop:1", "shop:2"]));
    assert_eq!(engine.review_items(&ReviewFilter::open()).unwrap(), []);

    // Undoing the decision puts each version back where it was.
    engine.undo(committed.seq, reviewer(), None, true).unwrap();
    assert_eq!(placed_versions(&engine).1, [Some(anvil), None]);
    engine.verify().unwrap();
}

#[test]
fn an_earlier_version_a_reviewer_moves_or_a_constraint_holds_stays_where_it_is() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("hammer"), None)],
    );
    load(&engine, "depot", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let (anvil, hammer) = (
        identity(&engine, "shop", "1").unwrap(),
        identity(&engine, "shop", "2").unwrap(),
    );
    load(&engine, "shop", &[("1", Some("hammer"), None)]);
    resolve(&engine);
    let (versions, _) = placed_versions(&engine);
    let (first, second) = (versions[0], versions[1]);
    let place = Op::Assign {
        record: second,
        from: None,
        to: Some(hammer),
    };

    // Where the changeset itself puts an earlier version is where it goes.
    let kept = author(
        &engine,
        vec![
            place.clone(),
            Op::Assign {
                record: first,
                from: Some(anvil),
                to: None,
            },
        ],
    );
    assert_eq!(placed_versions(&engine).1, [None, Some(hammer)]);
    engine.undo(kept.seq, reviewer(), None, true).unwrap();

    // A constraint that keeps the earlier version with the anvil keeps it
    // there, and the row goes where the reviewer says.
    let depot = record_id(&engine, "depot", "1");
    author(
        &engine,
        vec![constrain(ConstraintKind::MustLink, first, depot)],
    );
    author(&engine, vec![place]);
    assert_eq!(placed_versions(&engine).1, [Some(anvil), Some(hammer)]);
    engine.verify().unwrap();
}

#[test]
fn an_undo_leaves_an_earlier_version_where_the_changeset_found_it() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("hammer"), None)],
    );
    resolve(&engine);
    let (anvil, hammer) = (
        identity(&engine, "shop", "1").unwrap(),
        identity(&engine, "shop", "2").unwrap(),
    );
    load(&engine, "shop", &[("1", Some("anvil"), Some("K9"))]);
    resolve(&engine);
    let (versions, _) = placed_versions(&engine);
    let (first, second) = (versions[0], versions[1]);
    author(
        &engine,
        vec![Op::Assign {
            record: first,
            from: Some(anvil),
            to: Some(hammer),
        }],
    );

    // The earlier version is where the latest goes already, so the change
    // moves only the latest, and taking it back moves only the latest.
    let moved = author(
        &engine,
        vec![Op::Assign {
            record: second,
            from: Some(anvil),
            to: Some(hammer),
        }],
    );
    assert_eq!(placed_versions(&engine).1, [Some(hammer); 2]);
    let undone = engine.undo(moved.seq, reviewer(), None, true).unwrap();
    assert_eq!(
        undone.changeset.ops,
        [Op::Assign {
            record: second,
            from: Some(hammer),
            to: Some(anvil),
        }]
    );
    assert_eq!(placed_versions(&engine).1, [Some(hammer), Some(anvil)]);
    engine.verify().unwrap();
}

#[test]
fn a_row_placed_more_than_once_in_a_changeset_is_refused_with_nothing_moved_along() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("hammer"), None)],
    );
    resolve(&engine);
    let hammer = identity(&engine, "shop", "2").unwrap();
    for name in ["anvil mk2", "anvil mk3", "anvil mk4"] {
        load(&engine, "shop", &[("1", Some(name), None)]);
        resolve(&engine);
    }
    let (versions, before) = placed_versions(&engine);
    let latest = *versions.last().unwrap();
    let place = Op::Assign {
        record: latest,
        from: before[3],
        to: Some(hammer),
    };

    // The refusal is the changeset's own, and names no move of an earlier
    // version, however often the row is named.
    let head = engine.head().unwrap();
    let error = engine
        .author(reviewer(), None, vec![place; 50])
        .unwrap_err();
    let reasons = rejections(error);
    assert_eq!(reasons.len(), 49);
    assert!(reasons.iter().all(|reason| matches!(
        reason,
        Reason::RecordAssignedTwice { record, first_op: 0 } if *record == latest
    )));
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(placed_versions(&engine).1, before);
}

#[test]
fn a_split_moves_the_versions_it_was_given_and_no_other() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();
    for name in ["anvil mk2", "anvil mk3"] {
        load(&engine, "shop", &[("1", Some(name), None)]);
        resolve(&engine);
    }
    let (versions, identities) = placed_versions(&engine);
    assert_eq!(identities, [Some(anvil); 3]);

    // The reviewer splits the latest version from the two before it.
    let split = engine
        .split(anvil, &versions[2..], false, None, reviewer(), None)
        .unwrap();
    assert_eq!(split.kept, anvil);
    assert_eq!(split.moved, versions[2..]);
    assert_eq!(
        placed_versions(&engine).1,
        [Some(anvil), Some(anvil), Some(split.identity)]
    );
    engine.verify().unwrap();
}

#[test]
fn a_run_that_places_a_new_version_takes_along_an_earlier_one_that_waits() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[
            ("2", Some("anvil"), Some("K1")),
            ("3", Some("anvil"), Some("K2")),
        ],
    );
    resolve(&engine);
    let anvil = identity(&engine, "shop", "2").unwrap();

    // The first version of row 1 fits both anvils, and the reviewer leaves
    // it without an identity.
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    let item = engine
        .review_items(&ReviewFilter::open())
        .unwrap()
        .remove(0);
    engine.dismiss_review(item.id, &reviewer(), None).unwrap();

    // Its next version fits one of them. The run places it there, and its
    // earlier version with it, in the draft a reviewer approves.
    load(&engine, "shop", &[("1", Some("anvil"), Some("K1"))]);
    let options = ResolveOptions {
        leave_for_review: true,
        ..ResolveOptions::default()
    };
    let report = engine.resolve(&options).unwrap();
    let resolution = &report.resolved[0];
    assert_eq!(
        (
            resolution.new_records,
            resolution.records_assigned,
            resolution.versions_moved
        ),
        (1, 1, 1)
    );
    // A run recorded before runs counted the versions they moved along
    // reads as having moved none.
    let mut recorded = serde_json::to_value(resolution).unwrap();
    recorded.as_object_mut().unwrap().remove("versions_moved");
    let read: TypeResolution = serde_json::from_value(recorded).unwrap();
    assert_eq!((read.records_assigned, read.versions_moved), (1, 0));
    let draft = engine.draft(resolution.draft.unwrap()).unwrap();
    let (versions, _) = placed_versions(&engine);
    assert_eq!(
        draft.changeset.ops,
        [
            Op::Assign {
                record: versions[1],
                from: None,
                to: Some(anvil),
            },
            Op::Assign {
                record: versions[0],
                from: None,
                to: Some(anvil),
            },
        ]
    );
    engine.approve_draft(draft.changeset.id).unwrap();
    assert_eq!(placed_versions(&engine).1, [Some(anvil); 2]);
    engine.verify().unwrap();
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
