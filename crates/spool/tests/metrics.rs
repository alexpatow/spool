//! Metrics against a partition small enough to score by hand.

mod common;

use common::*;
use spool::core::{Actor, Changeset, EntityType, IdGenerator, Op, SeededIds};
use spool::{Engine, Rows};

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
    ingest(&engine, "shop", &things);
    let o = |name: &str| observation(&engine, "shop", name);

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
        observation: o(name),
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
        Rows::from_text(&["observation_id", "true_entity"], &rows).unwrap()
    };
    let metrics = engine.score(truth()).unwrap();

    assert_eq!(metrics.observations, 6);
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

    // Per observation, precision: 1, 1, 1, 1/2, 1/2, 1. Recall: 2/3, 2/3, 1/3, 1, 1/2, 1/2.
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
    assert_eq!(stability.observations, 5);
    assert_eq!(stability.unchanged, 4);
    close(stability.strict, 0.8);
    close(stability.following_redirects, 0.8);

    let metrics = engine.score(truth()).unwrap();
    assert_eq!(metrics.unassigned, 0);
    assert_eq!(metrics.false_splits, 0);
    assert_eq!(metrics.false_merges, 1);
    close(metrics.pairs.recall, 1.0);
}

#[test]
fn an_empty_truth_has_no_ratios() {
    let engine = engine();
    let truth = Rows::from_text(&["observation_id", "true_entity"], &[]).unwrap();
    let metrics = engine.score(truth).unwrap();
    assert_eq!(metrics.observations, 0);
    assert_eq!(metrics.pairs.precision, None);
    assert_eq!(metrics.b_cubed.recall, None);
    assert_eq!(metrics.unassigned_share, None);
}
