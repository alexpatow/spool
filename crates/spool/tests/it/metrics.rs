//! Metrics against a partition small enough to score by hand.

use spool::core::{Actor, Changeset, EntityType, IdGenerator, Op, SeededIds};
use spool::{Engine, Rows};
use spool_testing::*;

fn close(actual: Option<f64>, expected: f64) {
    let actual = actual.expect("the ratio has a denominator");
    assert!(
        (actual - expected).abs() < 1e-12,
        "{actual} is not {expected}"
    );
}

fn commit(engine: &Engine, ids: &mut SeededIds, ops: Vec<Op>) {
    engine
        .commit(Changeset {
            id: ids.changeset_id(),
            base: engine.head().unwrap(),
            actor: Actor::User("tester".to_owned()),
            reason: None,
            run: None,
            ops,
        })
        .unwrap();
}

#[test]
fn metrics_match_a_hand_scored_partition() {
    let engine = engine();
    let names = ["o1", "o2", "o3", "o4", "o5", "o6"];
    let things: Vec<_> = names
        .iter()
        .map(|name| (*name, Some(*name), None))
        .collect();
    load(&engine, "shop", &things);
    let o = |name: &str| record_id(&engine, "shop", name);

    // Identities:    i1 {o1 o2}   i2 {o3}   i3 {o4 o5}   and o6 has none
    // True entities: A  {o1 o2 o3}          B {o4}  C {o5 o6}
    let mut ids = SeededIds::new(42);
    let (i1, i2, i3) = (ids.identity_id(), ids.identity_id(), ids.identity_id());
    let thing = EntityType::new("thing").unwrap();
    let create = |identity| Op::CreateIdentity {
        identity,
        entity_type: thing.clone(),
    };
    let assign = |name: &str, from, to| Op::Assign {
        record: o(name),
        from,
        to,
    };
    commit(
        &engine,
        &mut ids,
        vec![
            create(i1),
            create(i2),
            create(i3),
            assign("o1", None, Some(i1)),
            assign("o2", None, Some(i1)),
            assign("o3", None, Some(i2)),
            assign("o4", None, Some(i3)),
            assign("o5", None, Some(i3)),
        ],
    );

    let truth = || {
        let ids: Vec<String> = names.iter().map(|name| o(name).to_string()).collect();
        let rows: Vec<Vec<Option<&str>>> = ids
            .iter()
            .zip(["A", "A", "A", "B", "C", "C"])
            .map(|(id, entity)| vec![Some(id.as_str()), Some(entity)])
            .collect();
        Rows::from_text(&["record_id", "true_entity"], &rows).unwrap()
    };
    let metrics = engine.score(truth()).unwrap();

    assert_eq!(metrics.records, 6);
    assert_eq!(metrics.true_entities, 3);
    assert_eq!(metrics.identities, 3);
    assert_eq!(metrics.unassigned, 1);
    close(metrics.unassigned_share, 1.0 / 6.0);

    // Pairs together in an identity: o1-o2 (right) and o4-o5 (wrong).
    // Pairs that share a true entity: o1-o2, o1-o3, o2-o3 and o5-o6.
    assert_eq!(metrics.pairs.true_positives, 1);
    assert_eq!(metrics.pairs.predicted, 2);
    assert_eq!(metrics.pairs.actual, 4);
    close(metrics.pairs.precision, 0.5);
    close(metrics.pairs.recall, 0.25);
    close(metrics.pairs.f1, 1.0 / 3.0);

    // Per record, precision: 1, 1, 1, 1/2, 1/2, 1. Recall: 2/3, 2/3, 1/3, 1, 1/2, 1/2.
    close(metrics.b_cubed.precision, 5.0 / 6.0);
    close(metrics.b_cubed.recall, 11.0 / 18.0);

    // i3 holds B and C. A is spread over i1 and i2. C is in one identity only.
    assert_eq!(metrics.false_merges, 1);
    assert_eq!(metrics.false_splits, 1);

    // Moving o3 changes its ID; giving o6 an identity changes no existing one.
    let before = engine.head().unwrap();
    commit(
        &engine,
        &mut ids,
        vec![
            assign("o3", Some(i2), Some(i1)),
            assign("o6", None, Some(i3)),
        ],
    );
    let stability = engine.id_stability(before, engine.head().unwrap()).unwrap();
    assert_eq!(stability.records, 5);
    assert_eq!(stability.unchanged, 4);
    close(stability.strict, 0.8);
    close(stability.following_redirects, 0.8);

    let metrics = engine.score(truth()).unwrap();
    assert_eq!(metrics.unassigned, 0);
    assert_eq!(metrics.false_splits, 0);
    assert_eq!(metrics.false_merges, 1);
    close(metrics.pairs.recall, 1.0);
}

/// Puts each listed group of `shop` records in an identity of its own and
/// scores the result against `truth`, a true entity per record.
fn scored(groups: &[&[&str]], truth: &[(&str, &str)]) -> spool::Metrics {
    let engine = engine();
    let things: Vec<_> = truth
        .iter()
        .map(|(name, _)| (*name, Some(*name), None))
        .collect();
    load(&engine, "shop", &things);

    let mut ids = SeededIds::new(42);
    let mut ops = Vec::new();
    for group in groups {
        let identity = ids.identity_id();
        ops.push(Op::CreateIdentity {
            identity,
            entity_type: EntityType::new("thing").unwrap(),
        });
        ops.extend(group.iter().map(|name| Op::Assign {
            record: record_id(&engine, "shop", name),
            from: None,
            to: Some(identity),
        }));
    }
    if !ops.is_empty() {
        commit(&engine, &mut ids, ops);
    }

    let record_ids: Vec<String> = truth
        .iter()
        .map(|(name, _)| record_id(&engine, "shop", name).to_string())
        .collect();
    let rows: Vec<Vec<Option<&str>>> = record_ids
        .iter()
        .zip(truth)
        .map(|(id, (_, entity))| vec![Some(id.as_str()), Some(*entity)])
        .collect();
    engine
        .score(Rows::from_text(&["record_id", "true_entity"], &rows).unwrap())
        .unwrap()
}

#[test]
fn f1_is_zero_when_no_predicted_pair_is_right() {
    // Identities:    {a c} {b d}
    // True entities: {a b} {c d}
    let metrics = scored(
        &[&["a", "c"], &["b", "d"]],
        &[("a", "A"), ("b", "A"), ("c", "B"), ("d", "B")],
    );
    assert_eq!(metrics.pairs.true_positives, 0);
    assert_eq!((metrics.pairs.predicted, metrics.pairs.actual), (2, 2));
    assert_eq!(metrics.pairs.precision, Some(0.0));
    assert_eq!(metrics.pairs.recall, Some(0.0));
    assert_eq!(metrics.pairs.f1, Some(0.0));
}

#[test]
fn f1_is_zero_when_only_one_side_has_pairs() {
    // Nothing is paired, and the truth has a pair: every true pair is missed.
    let metrics = scored(&[&["a"], &["b"]], &[("a", "A"), ("b", "A")]);
    assert_eq!((metrics.pairs.predicted, metrics.pairs.actual), (0, 1));
    assert_eq!(metrics.pairs.precision, None);
    assert_eq!(metrics.pairs.recall, Some(0.0));
    assert_eq!(metrics.pairs.f1, Some(0.0));

    // A pair is predicted, and the truth has none: every predicted pair is wrong.
    let metrics = scored(&[&["a", "b"]], &[("a", "A"), ("b", "B")]);
    assert_eq!((metrics.pairs.predicted, metrics.pairs.actual), (1, 0));
    assert_eq!(metrics.pairs.precision, Some(0.0));
    assert_eq!(metrics.pairs.recall, None);
    assert_eq!(metrics.pairs.f1, Some(0.0));
}

#[test]
fn f1_is_undefined_only_when_neither_side_has_pairs() {
    // Every record is alone, in the result and in the truth.
    let metrics = scored(&[&["a"], &["b"]], &[("a", "A"), ("b", "B")]);
    assert_eq!((metrics.pairs.predicted, metrics.pairs.actual), (0, 0));
    assert_eq!(metrics.pairs.precision, None);
    assert_eq!(metrics.pairs.recall, None);
    assert_eq!(metrics.pairs.f1, None);
    // The other ratios have denominators and are right.
    assert_eq!(metrics.b_cubed.precision, Some(1.0));
    assert_eq!(metrics.b_cubed.recall, Some(1.0));
    assert_eq!(metrics.unassigned_share, Some(0.0));
}

#[test]
fn an_empty_truth_has_no_ratios() {
    let engine = engine();
    let truth = Rows::from_text(&["record_id", "true_entity"], &[]).unwrap();
    let metrics = engine.score(truth).unwrap();
    assert_eq!(metrics.records, 0);
    assert_eq!(metrics.pairs.precision, None);
    assert_eq!(metrics.b_cubed.recall, None);
    assert_eq!(metrics.unassigned_share, None);
}

/// An anvil and a hammer, each an identity of its own at log position 1,
/// and what a reviewer then did: merged the anvil into the hammer at 2, took
/// that back at 3, and merged them again at 4.
fn merged_undone_and_merged_again(engine: &Engine, steps: usize) {
    let reviewer = || Actor::User("reviewer".to_owned());
    load(
        engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("hammer"), None)],
    );
    engine.resolve(&spool::ResolveOptions::default()).unwrap();
    let anvil = identity(engine, "shop", "1").unwrap();
    let hammer = identity(engine, "shop", "2").unwrap();
    let merge = || {
        vec![Op::Merge {
            from: anvil,
            into: hammer,
        }]
    };
    if steps >= 1 {
        engine.author(reviewer(), None, merge()).unwrap();
    }
    if steps >= 2 {
        engine
            .undo(spool::core::Seq(2), reviewer(), None, true)
            .unwrap();
    }
    if steps >= 3 {
        engine.author(reviewer(), None, merge()).unwrap();
    }
}

#[test]
fn churn_between_two_positions_does_not_depend_on_what_was_committed_later() {
    use spool::core::Seq;
    let merged = |engine: &Engine, before: u64, after: u64| {
        engine
            .churn(Seq(before), Seq(after), 0)
            .unwrap()
            .identities_merged
    };

    for steps in 1..=3 {
        let engine = engine();
        merged_undone_and_merged_again(&engine, steps);
        // The merge at position 2 is a merge between 1 and 2, whatever
        // became of it afterwards.
        assert_eq!(merged(&engine, 1, 2), 1, "after {steps} corrections");
        assert_eq!(merged(&engine, 0, 1), 0, "after {steps} corrections");
        if steps >= 2 {
            // Merged and reinstated in between: the two positions agree.
            assert_eq!(merged(&engine, 1, 3), 0, "after {steps} corrections");
            assert_eq!(merged(&engine, 2, 3), 0, "after {steps} corrections");
        }
        if steps >= 3 {
            assert_eq!(merged(&engine, 3, 4), 1);
            assert_eq!(merged(&engine, 1, 4), 1);
        }
    }
}

#[test]
fn id_stability_follows_the_redirects_of_the_later_position() {
    use spool::core::Seq;
    for steps in 1..=3 {
        let engine = engine();
        merged_undone_and_merged_again(&engine, steps);
        // At position 2 the anvil's ID redirected to the hammer's, whatever
        // became of the merge afterwards.
        let stability = engine.id_stability(Seq(1), Seq(2)).unwrap();
        assert_eq!(
            (
                stability.records,
                stability.unchanged,
                stability.unchanged_following_redirects
            ),
            (2, 1, 2),
            "after {steps} corrections"
        );
        if steps >= 2 {
            // At position 3 the merge had been taken back.
            let stability = engine.id_stability(Seq(1), Seq(3)).unwrap();
            assert_eq!(
                (stability.unchanged, stability.unchanged_following_redirects),
                (2, 2),
                "after {steps} corrections"
            );
            // Between 2 and 3 the anvil left the hammer's identity, and
            // nothing at position 3 redirects one to the other.
            let stability = engine.id_stability(Seq(2), Seq(3)).unwrap();
            assert_eq!(
                (stability.unchanged, stability.unchanged_following_redirects),
                (1, 1),
                "after {steps} corrections"
            );
        }
    }
}

#[test]
fn id_stability_follows_a_chain_of_merges_as_far_as_it_went_by_then() {
    use spool::core::Seq;
    let engine = engine();
    load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("hammer"), None),
            ("3", Some("tongs"), None),
        ],
    );
    engine.resolve(&spool::ResolveOptions::default()).unwrap();
    let [anvil, hammer, tongs] = ["1", "2", "3"].map(|key| identity(&engine, "shop", key).unwrap());
    let merge = |from, into| {
        engine
            .author(
                Actor::User("reviewer".to_owned()),
                None,
                vec![Op::Merge { from, into }],
            )
            .unwrap();
    };
    merge(anvil, hammer);
    merge(hammer, tongs);

    let counts = |before: u64, after: u64| {
        let stability = engine.id_stability(Seq(before), Seq(after)).unwrap();
        (stability.unchanged, stability.unchanged_following_redirects)
    };
    // At position 2 only the anvil had been merged, into the hammer.
    assert_eq!(counts(1, 2), (2, 3));
    // At position 3 the anvil's ID leads through the hammer's to the tongs'.
    assert_eq!(counts(1, 3), (1, 3));
    assert_eq!(counts(2, 3), (1, 3));
    let churn = engine.churn(Seq(1), Seq(3), 0).unwrap();
    assert_eq!(churn.identities_merged, 2);
    assert_eq!(
        engine.churn(Seq(2), Seq(3), 0).unwrap().identities_merged,
        1
    );
}
