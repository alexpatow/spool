//! Recomputing: what the identities would be under the rules as they are
//! now, shown first and applied only when asked.

mod common;

use common::{SOURCES, entity, identity, load, manifest_of, members, record_id, thing};
use spool::core::{Actor, ConstraintKind, EntityType, Op};
use spool::{
    ChangeKind, Engine, EntityTypeSource, Error, Fate, Options, RecomputeRequest, ResolveOptions,
    RunStatus, SqlFile,
};

fn reviewer() -> Actor {
    Actor::User("reviewer".to_owned())
}

/// Things that match on their name alone: the loose rules a project starts
/// with.
fn loose() -> EntityTypeSource {
    EntityTypeSource {
        block: vec![SqlFile::new("same_name", "l.name = r.name")],
        no_match: Vec::new(),
        match_: vec![SqlFile::new("same_name", "l.name = r.name")],
        ..thing()
    }
}

fn everything() -> RecomputeRequest {
    RecomputeRequest {
        limit: 100,
        ..RecomputeRequest::default()
    }
}

/// A store resolved by the loose rules, with a decision of each kind a
/// reviewer makes, then given the stricter rules of `thing`, which also
/// match on a code and keep apart things whose codes differ.
///
/// - `wilson` 1, 2 and 3 are one identity. 1 and 3 share a code and 2 has
///   another, so the strict rules split it.
/// - `lee` X, Y and Z are one identity, X and Z with codes that differ and
///   Y with none: Y joins both, and the strict rules contradict themselves.
/// - `halden` P and `the halden` Q were placed apart, and share a code:
///   the strict rules merge them.
/// - `kestrel` R and T and `kestrel us` S share a code, and a reviewer
///   said R and S differ with a `cannot_link`. The strict rules join all
///   three through T, which the constraint contradicts.
/// - `osprey` O1 and O2 share a code, and a reviewer said they differ.
///   Nothing else joins them, so each stays an identity of its own.
/// - `alpha` U and `beta` V have codes that differ, and a reviewer merged
///   them.
fn store() -> Engine {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&manifest_of(vec![loose()])).unwrap();
    load(
        &engine,
        "shop",
        &[
            ("1", Some("wilson"), Some("W1")),
            ("2", Some("wilson"), Some("W2")),
            ("3", Some("wilson"), Some("W1")),
            ("X", Some("lee"), Some("L1")),
            ("Y", Some("lee"), None),
            ("Z", Some("lee"), Some("L2")),
            ("P", Some("halden"), Some("H1")),
            ("R", Some("kestrel"), Some("K1")),
            ("S", Some("kestrel us"), Some("K1")),
            ("T", Some("kestrel"), Some("K1")),
            ("O1", Some("osprey"), Some("O")),
            ("O2", Some("osprey ltd"), Some("O")),
            ("U", Some("alpha"), Some("A1")),
            ("V", Some("beta"), Some("B1")),
        ],
    );
    engine.resolve(&ResolveOptions::default()).unwrap();
    load(&engine, "depot", &[("Q", Some("the halden"), Some("H1"))]);
    engine.resolve(&ResolveOptions::default()).unwrap();

    let r = record_id(&engine, "shop", "R");
    let s = record_id(&engine, "shop", "S");
    let u = identity(&engine, "shop", "U").unwrap();
    let v = identity(&engine, "shop", "V").unwrap();
    engine
        .author(
            reviewer(),
            Some("two different kestrels".to_owned()),
            vec![
                Op::Constrain {
                    kind: ConstraintKind::CannotLink,
                    a: r,
                    b: s,
                },
                Op::Constrain {
                    kind: ConstraintKind::CannotLink,
                    a: record_id(&engine, "shop", "O1"),
                    b: record_id(&engine, "shop", "O2"),
                },
            ],
        )
        .unwrap();
    engine
        .author(
            reviewer(),
            Some("beta is alpha's old name".to_owned()),
            vec![Op::Merge { from: v, into: u }],
        )
        .unwrap();

    let snapshot = engine.snapshot().unwrap();
    engine.apply(&manifest_of(vec![thing()])).unwrap();
    // A rule change alone moves nothing.
    assert_eq!(engine.snapshot().unwrap(), snapshot);
    engine
}

#[test]
fn a_preview_shows_every_kind_of_change_and_changes_nothing() {
    let engine = store();
    let head = engine.head().unwrap();
    let snapshot = engine.snapshot().unwrap();
    let runs = engine.runs(&Default::default()).unwrap().len();

    let report = engine.recompute(&everything()).unwrap();
    let [thing] = &report.types[..] else {
        panic!("one type was recomputed");
    };
    let wilson = identity(&engine, "shop", "1").unwrap();
    let lee = identity(&engine, "shop", "X").unwrap();
    let halden = identity(&engine, "shop", "P").unwrap();
    let the_halden = identity(&engine, "depot", "Q").unwrap();
    let alpha = identity(&engine, "shop", "U").unwrap();

    let summary = &thing.summary;
    assert_eq!(summary.records, 15);
    // wilson, lee, halden, the halden, kestrel, kestrel us, osprey, osprey
    // ltd, alpha.
    assert_eq!(summary.identities_now, 9);
    // wilson 1 and 3, wilson 2, halden, osprey, osprey ltd, alpha.
    assert_eq!(summary.identities_after, 6);
    assert_eq!(summary.unchanged, 3);
    assert_eq!(summary.splitting, 1);
    assert_eq!(summary.merging, 1);
    assert_eq!(summary.merged_away, 1);
    assert_eq!(summary.new_identities, 1);
    // lee, kestrel and kestrel us.
    assert_eq!(summary.emptied, 3);
    assert_eq!(summary.records_unplaced, 6);
    assert_eq!(summary.records_waiting, 6);
    assert_eq!(summary.review_items, 2);
    // wilson 2 moves to a new identity, the halden Q by the merge.
    assert_eq!(summary.records_moved, 2);
    // wilson 3, lee 3, kestrel R and T 1, alpha 1.
    assert_eq!(summary.pairs_now, 8);
    // wilson 1 and 3, halden P and Q, alpha U and V.
    assert_eq!(summary.pairs_after, 3);
    assert_eq!(summary.pairs_kept, 2);

    let decisions = &thing.decisions;
    assert_eq!(decisions.cannot_link, 2);
    assert_eq!(decisions.blocked_by_constraints, 1);
    assert_eq!(decisions.blocked_by_rules, 1);
    assert_eq!(decisions.held_together, 1);

    let change_of = |identity| {
        thing
            .changes
            .iter()
            .find(|change| change.before.iter().any(|b| b.identity == Some(identity)))
            .unwrap_or_else(|| panic!("no change has {identity}"))
    };
    let mut kinds: Vec<(u64, ChangeKind)> = thing
        .changes
        .iter()
        .map(|change| (change.records, change.kind))
        .collect();
    kinds.sort_by_key(|(records, kind)| (std::cmp::Reverse(*records), kind.as_str()));
    assert_eq!(
        kinds,
        [
            (3, ChangeKind::Review),
            (3, ChangeKind::Review),
            (3, ChangeKind::Split),
            (2, ChangeKind::Merge),
        ]
    );
    assert_eq!(thing.changes_total, 4);
    assert!(thing.changes.windows(2).all(|pair| pair[0].records >= pair[1].records));

    // The split keeps the ID with the larger side.
    let split = change_of(wilson);
    assert_eq!(split.kind, ChangeKind::Split);
    assert_eq!(split.after[0].fate, Fate::Kept);
    assert_eq!(split.after[0].identity, Some(wilson));
    assert_eq!(split.after[0].records, 2);
    assert_eq!(split.after[1].fate, Fate::New);
    assert_eq!(split.after[1].records, 1);

    // What stops lee is the rules; what stops the kestrels is a person.
    let lee_change = change_of(lee);
    assert_eq!(lee_change.after[0].fate, Fate::Waiting);
    assert_eq!(lee_change.blocked_by[0].rules, ["different_code"]);
    assert_eq!(lee_change.blocked_by[0].constraint, None);
    let kestrels = change_of(identity(&engine, "shop", "R").unwrap());
    assert_eq!(kestrels.before.len(), 2);
    let constraint = kestrels.blocked_by[0].constraint.as_ref().unwrap();
    assert_eq!(constraint.actor, reviewer());
    assert_eq!(constraint.reason.as_deref(), Some("two different kestrels"));

    // The older identity keeps its ID and the other stands for it.
    let merge = change_of(halden);
    assert_eq!(merge.kind, ChangeKind::Merge);
    assert_eq!(merge.after[0].identity, Some(halden));
    assert_eq!(merge.after[0].merged, [the_halden]);

    // What the reviewer merged stays merged, though the rules say they
    // differ, and what they kept apart stays apart.
    for key in ["U", "O1", "O2"] {
        let unchanged = identity(&engine, "shop", key).unwrap();
        assert!(
            thing
                .changes
                .iter()
                .all(|change| change.before.iter().all(|b| b.identity != Some(unchanged))),
            "shop:{key} changes"
        );
    }
    let _ = alpha;

    // Nothing changed, and nothing was recorded.
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(engine.snapshot().unwrap(), snapshot);
    assert_eq!(engine.runs(&Default::default()).unwrap().len(), runs);
    assert_eq!(
        engine.recompute(&everything()).unwrap().plan,
        report.plan,
        "the same store gives the same plan"
    );
}

#[test]
fn the_detail_of_a_change_names_each_record_and_where_it_goes() {
    let engine = store();
    let wilson = identity(&engine, "shop", "1").unwrap();
    let report = engine
        .recompute(&RecomputeRequest {
            identity: Some(wilson),
            ..RecomputeRequest::default()
        })
        .unwrap();
    let detail = report.types[0].detail.as_ref().unwrap();
    let moves: Vec<(String, u64)> = detail
        .records
        .iter()
        .map(|record| (record.key.clone().unwrap(), record.to))
        .collect();
    assert_eq!(
        moves,
        [("1".to_owned(), 0), ("3".to_owned(), 0), ("2".to_owned(), 1)]
    );
    assert!(detail.records.iter().all(|record| record.from == Some(wilson)));
    assert_eq!(detail.records[0].label.as_deref(), Some("wilson"));

    // An identity that does not change has no detail.
    let alpha = identity(&engine, "shop", "U").unwrap();
    let report = engine
        .recompute(&RecomputeRequest {
            identity: Some(alpha),
            ..RecomputeRequest::default()
        })
        .unwrap();
    assert_eq!(report.types[0].detail, None);
}

#[test]
fn applying_makes_the_identities_what_the_preview_showed() {
    let engine = store();
    let preview = engine.recompute(&everything()).unwrap();
    let wilson = identity(&engine, "shop", "1").unwrap();
    let halden = identity(&engine, "shop", "P").unwrap();
    let the_halden = identity(&engine, "depot", "Q").unwrap();
    let alpha = identity(&engine, "shop", "U").unwrap();
    let r = record_id(&engine, "shop", "R");
    let s = record_id(&engine, "shop", "S");

    let applied = engine
        .acting(reviewer())
        .apply_recompute(None, &preview.plan, Some("stricter codes".to_owned()))
        .unwrap();
    let [thing] = &applied.types[..] else {
        panic!("one type was applied");
    };
    assert_eq!(thing.summary, preview.types[0].summary);
    let committed = thing.committed.as_ref().unwrap();

    // The split kept its ID on the larger side.
    assert_eq!(
        members(&engine, wilson),
        ["shop:1".to_owned(), "shop:3".to_owned()].into()
    );
    let other = identity(&engine, "shop", "2").unwrap();
    assert_ne!(other, wilson);
    // The merge: the halden's ID stands for halden.
    assert_eq!(engine.current_id(the_halden).unwrap(), halden);
    assert_eq!(
        members(&engine, halden),
        ["shop:P".to_owned(), "depot:Q".to_owned()].into()
    );
    // The reviewer's merge survived, and so did the `cannot_link`: the
    // kestrels wait for review apart from any identity, as do the lees.
    assert_eq!(
        members(&engine, alpha),
        ["shop:U".to_owned(), "shop:V".to_owned()].into()
    );
    for key in ["R", "S", "T", "X", "Y", "Z"] {
        assert_eq!(identity(&engine, "shop", key), None, "shop:{key}");
    }
    assert_ne!(identity(&engine, "shop", "O1"), identity(&engine, "shop", "O2"));
    assert!(identity(&engine, "shop", "O1").is_some());
    let constraints = engine.constraints(&Default::default()).unwrap();
    assert_eq!(constraints.len(), 2);
    let mut pair = [r, s];
    pair.sort();
    assert!(
        constraints
            .iter()
            .any(|entry| [entry.constraint.a, entry.constraint.b] == pair)
    );
    let open = engine
        .review_items(&spool::ReviewFilter::open())
        .unwrap();
    assert_eq!(open.len(), 2);
    assert_eq!(thing.review.len(), 2);

    // Through the log, by the engine, from runs the reviewer started.
    let entry = engine.log().unwrap().pop().unwrap();
    assert_eq!(entry.seq, committed.seq);
    assert_eq!(entry.changeset.actor, Actor::Engine);
    assert_eq!(entry.changeset.run, Some(thing.reconcile_run));
    assert!(
        entry
            .changeset
            .reason
            .as_deref()
            .unwrap()
            .contains("stricter codes")
    );
    let run = engine.run(thing.match_run).unwrap();
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(run.started_by, reviewer());
    engine.verify().unwrap();

    // What the rules say now is what there is.
    let again = engine.recompute(&everything()).unwrap();
    assert_eq!(again.types[0].changes_total, 0);
    assert_eq!(again.types[0].summary.records_waiting, 6);
    // And a run that is not a recompute leaves it so.
    engine.resolve(&ResolveOptions::default()).unwrap();
    for key in ["R", "S", "T"] {
        assert_eq!(identity(&engine, "shop", key), None, "shop:{key}");
    }
    engine.verify().unwrap();
}

#[test]
fn a_plan_that_no_longer_holds_applies_nothing() {
    let engine = store();
    let preview = engine.recompute(&everything()).unwrap();
    // Someone places a record after the preview.
    load(&engine, "kiosk", &[("W", Some("wilson"), Some("W2"))]);
    let head = engine.head().unwrap();
    let snapshot = engine.snapshot().unwrap();

    let refused = engine
        .apply_recompute(None, &preview.plan, None)
        .unwrap_err();
    let Error::StalePlan { plan, now } = refused else {
        panic!("refused with {refused}");
    };
    assert_eq!(plan, preview.plan);
    assert_ne!(now, preview.plan);
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(engine.snapshot().unwrap(), snapshot);

    // A plan for another type is refused too.
    assert!(matches!(
        engine.apply_recompute(None, "0000000000000000", None),
        Err(Error::StalePlan { .. })
    ));
    let fresh = engine.recompute(&everything()).unwrap();
    assert_eq!(fresh.plan, now);
    engine.apply_recompute(None, &fresh.plan, None).unwrap();
    engine.verify().unwrap();
}

#[test]
fn a_change_touching_what_an_approved_draft_placed_is_flagged() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&manifest_of(vec![loose()])).unwrap();
    load(
        &engine,
        "shop",
        &[
            ("1", Some("wilson"), Some("W1")),
            ("2", Some("wilson"), Some("W2")),
        ],
    );
    let report = engine
        .resolve(&ResolveOptions {
            leave_for_review: true,
            ..ResolveOptions::default()
        })
        .unwrap();
    engine
        .approve_draft(report.resolved[0].draft.unwrap())
        .unwrap();
    engine.apply(&manifest_of(vec![thing()])).unwrap();

    let preview = engine.recompute(&everything()).unwrap();
    let thing = &preview.types[0];
    // The approval binds no more than the run: the rules split it.
    assert_eq!(thing.changes[0].kind, ChangeKind::Split);
    assert_eq!(thing.changes[0].approved_drafts, 2);
    assert_eq!(thing.decisions.changes_with_approved_drafts, 1);
}

#[test]
fn records_a_person_placed_stay_with_the_identity_they_placed_them_in() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&manifest_of(vec![thing()])).unwrap();
    load(
        &engine,
        "shop",
        &[
            ("1", Some("wilson"), Some("W1")),
            ("2", Some("wilson"), Some("W1")),
            ("3", Some("woodrow"), Some("W9")),
        ],
    );
    engine.resolve(&ResolveOptions::default()).unwrap();
    let wilson = identity(&engine, "shop", "1").unwrap();
    let woodrow = identity(&engine, "shop", "3").unwrap();
    engine
        .author(
            reviewer(),
            Some("the same president".to_owned()),
            vec![Op::Assign {
                record: record_id(&engine, "shop", "3"),
                from: Some(woodrow),
                to: Some(wilson),
            }],
        )
        .unwrap();

    let preview = engine.recompute(&everything()).unwrap();
    assert_eq!(preview.types[0].changes_total, 0);
    assert_eq!(preview.types[0].decisions.held_together, 1);

    // A split without a `cannot_link` leaves nothing that says the sides
    // differ: the rules join them again, and the change says who split.
    let split = engine
        .split(
            wilson,
            &[record_id(&engine, "shop", "2")],
            false,
            None,
            reviewer(),
            None,
        )
        .unwrap();
    let preview = engine.recompute(&everything()).unwrap();
    let change = &preview.types[0].changes[0];
    assert_eq!(change.kind, ChangeKind::Merge);
    assert!(
        change
            .shaped_by
            .iter()
            .any(|decision| decision.seq == split.committed.seq)
    );
}

#[test]
fn records_taken_back_have_no_part_in_it() {
    let engine = store();
    engine
        .retract("shop", "thing", &["Y".to_owned()], reviewer(), None)
        .unwrap();
    let preview = engine.recompute(&everything()).unwrap();
    let thing = &preview.types[0];
    assert_eq!(thing.summary.records, 14);
    // Without Y nothing joins X and Z: lee splits instead of waiting.
    let lee = identity(&engine, "shop", "X").unwrap();
    let change = thing
        .changes
        .iter()
        .find(|change| change.before.iter().any(|b| b.identity == Some(lee)))
        .unwrap();
    assert_eq!(change.kind, ChangeKind::Split);
    engine.apply_recompute(None, &preview.plan, None).unwrap();
    assert_eq!(identity(&engine, "shop", "Y"), None);
    engine.verify().unwrap();
}

#[test]
fn every_type_is_recomputed_unless_one_is_named() {
    let other = EntityTypeSource {
        name: "gadget".to_owned(),
        entity: entity(Some("name"), &[("name", "name"), ("code", "code")], &SOURCES),
        ..thing()
    };
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&manifest_of(vec![thing(), other])).unwrap();
    let all = engine.recompute(&everything()).unwrap();
    assert_eq!(all.types.len(), 2);
    let one = engine
        .recompute(&RecomputeRequest {
            entity_type: Some(EntityType::new("gadget").unwrap()),
            ..everything()
        })
        .unwrap();
    assert_eq!(one.types.len(), 1);
    assert_ne!(one.plan, all.plan);
    assert!(matches!(
        engine.recompute(&RecomputeRequest {
            entity_type: Some(EntityType::new("widget").unwrap()),
            ..everything()
        }),
        Err(Error::UndeclaredType(_))
    ));
}
