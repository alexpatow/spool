//! Rematching: a run that examines every observation again, and reports
//! where its strategy disagrees with the identities as they are.

mod common;

use common::{engine, identity, ingest, members, observation};
use spool::core::{Actor, EntityType, Op, Policy};
use spool::{
    Disposition, Engine, ResolveOptions, Rows, RunStatus, Scope, StrategyRef, SuggestedMerge,
    SuggestedSplit, Suggestions,
};

fn reviewer() -> Actor {
    Actor::User("reviewer".to_owned())
}

fn rematch() -> ResolveOptions {
    ResolveOptions {
        rematch: true,
        ..ResolveOptions::default()
    }
}

/// Two anvils and two pairs of tongs, which the rules match, and two
/// hammers with different codes, which the rules keep apart.
fn workshop() -> Engine {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), Some("K1")),
            ("2", Some("anvil"), Some("K1")),
            ("3", Some("hammer"), Some("H1")),
            ("4", Some("hammer"), Some("H2")),
            ("5", Some("tongs"), None),
            ("6", Some("tongs"), None),
        ],
    );
    let report = engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(report.resolved[0].identities_created, 4);
    // Only a rematch judges pairs of observations that both have an identity.
    assert_eq!(report.resolved[0].suggestions, None);
    engine
}

#[test]
fn a_rematch_suggests_and_does_not_change() {
    let engine = workshop();
    let anvils = identity(&engine, "shop", "1").unwrap();
    let hammer = identity(&engine, "shop", "3").unwrap();
    let other_hammer = identity(&engine, "shop", "4").unwrap();
    let tongs = identity(&engine, "shop", "5").unwrap();

    // Nothing was corrected, so the rules agree with what they made.
    let report = engine.resolve(&rematch()).unwrap();
    assert_eq!(report.resolved[0].suggestions, Some(Suggestions::default()));

    // A reviewer merges the hammers, which the rules say differ, and takes
    // the pairs of tongs apart, which the rules say are the same.
    engine
        .author(
            reviewer(),
            None,
            vec![Op::Merge {
                from: other_hammer,
                into: hammer,
            }],
        )
        .unwrap();
    let split = engine
        .split(
            tongs,
            &[observation(&engine, "shop", "6")],
            false,
            reviewer(),
            None,
        )
        .unwrap();
    let head = engine.head().unwrap();
    let before = engine.snapshot().unwrap();

    let report = engine.resolve(&rematch()).unwrap();
    let [resolved] = &report.resolved[..] else {
        panic!("one entity type was rematched");
    };
    let mut merge = [tongs, split.identity];
    merge.sort();
    assert_eq!(
        resolved.suggestions,
        Some(Suggestions {
            merges: vec![SuggestedMerge {
                identities: merge,
                matches: 1,
                no_matches: 0,
            }],
            splits: vec![SuggestedSplit {
                identity: hammer,
                no_matches: 1,
                matches: 0,
            }],
        })
    );

    // It examined every pair again and changed nothing.
    assert_eq!(resolved.candidates, 3);
    assert_eq!(resolved.new_observations, 0);
    assert_eq!(resolved.committed, None);
    assert_eq!(engine.head().unwrap(), head);
    assert_eq!(engine.snapshot().unwrap(), before);
    assert_eq!(
        engine.run(resolved.match_run).unwrap().status,
        RunStatus::Succeeded
    );
    assert_eq!(
        engine.run(resolved.match_run).unwrap().resolution,
        Some(resolved.clone())
    );
    assert_eq!(
        members(&engine, anvils),
        ["shop:1".to_owned(), "shop:2".to_owned()].into()
    );

    // A run that is not a rematch has nothing to examine.
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
fn a_rematch_does_not_suggest_what_a_reviewer_ruled_out() {
    let engine = workshop();
    let anvils = identity(&engine, "shop", "1").unwrap();
    let hammer = identity(&engine, "shop", "3").unwrap();
    let other_hammer = identity(&engine, "shop", "4").unwrap();

    // The anvils are split and kept apart: the rules would put them back
    // together, and a reviewer said no.
    engine
        .split(
            anvils,
            &[observation(&engine, "shop", "2")],
            true,
            reviewer(),
            None,
        )
        .unwrap();
    // The hammers are merged and held together.
    engine
        .author(
            reviewer(),
            None,
            vec![
                Op::Merge {
                    from: other_hammer,
                    into: hammer,
                },
                Op::Constrain {
                    kind: spool::core::ConstraintKind::MustLink,
                    a: observation(&engine, "shop", "3"),
                    b: observation(&engine, "shop", "4"),
                },
            ],
        )
        .unwrap();

    let report = engine.resolve(&rematch()).unwrap();
    assert_eq!(report.resolved[0].suggestions, Some(Suggestions::default()));
}

#[test]
fn a_rematch_places_an_observation_that_has_no_identity() {
    let engine = workshop();
    let tongs = identity(&engine, "shop", "5").unwrap();
    let loose = observation(&engine, "shop", "6");
    engine
        .author(
            reviewer(),
            None,
            vec![Op::Assign {
                observation: loose,
                from: Some(tongs),
                to: None,
            }],
        )
        .unwrap();

    // It is not new, so a run leaves it where the reviewer put it.
    assert!(
        engine
            .resolve(&ResolveOptions::default())
            .unwrap()
            .resolved
            .is_empty()
    );
    assert_eq!(engine.identity_of(loose).unwrap(), None);

    // A rematch looks at everything.
    let report = engine.resolve(&rematch()).unwrap();
    assert_eq!(report.resolved[0].new_observations, 1);
    assert_eq!(report.resolved[0].observations_assigned, 1);
    assert_eq!(engine.identity_of(loose).unwrap(), Some(tongs));
    engine.verify().unwrap();
}

#[test]
fn a_caller_can_rematch_with_its_own_judgements() {
    let engine = workshop();
    let thing = EntityType::new("thing").unwrap();
    let anvils = identity(&engine, "shop", "1").unwrap();
    let (first, second) = (
        observation(&engine, "shop", "1"),
        observation(&engine, "shop", "2"),
    );
    let judgement = |verdict: &str| {
        Rows::from_text(
            &["left_id", "right_id", "verdict"],
            &[vec![
                Some(first.to_string().as_str()),
                Some(second.to_string().as_str()),
                Some(verdict),
            ]],
        )
        .unwrap()
    };

    // In a run of what is new, a pair of observations that both have an
    // identity is not the run's to judge.
    let run = engine
        .begin_run(
            &StrategyRef::external("by-hand", None),
            &Scope::new(thing.clone()),
        )
        .unwrap();
    let error = engine
        .submit_judgements(run, judgement("no_match"))
        .unwrap_err();
    assert!(error.to_string().contains("neither observation is new"));
    engine.cancel_run(run).unwrap();

    let run = engine
        .begin_run(
            &StrategyRef::external("by-hand", None),
            &Scope::rematch(thing),
        )
        .unwrap();
    let pairs: usize = engine
        .candidates(run)
        .unwrap()
        .iter()
        .map(|batch| batch.num_rows())
        .sum();
    assert_eq!(pairs, 3);
    assert_eq!(
        engine
            .submit_judgements(run, judgement("no_match"))
            .unwrap(),
        1
    );
    let resolved = engine
        .reconcile_with(run, &Policy::default(), Disposition::Commit)
        .unwrap();
    assert_eq!(
        resolved.suggestions,
        Some(Suggestions {
            merges: Vec::new(),
            splits: vec![SuggestedSplit {
                identity: anvils,
                no_matches: 1,
                matches: 0,
            }],
        })
    );
    assert_eq!(
        members(&engine, anvils),
        ["shop:1".to_owned(), "shop:2".to_owned()].into()
    );
}

#[test]
fn a_strategy_the_project_does_not_have_starts_nothing() {
    let engine = workshop();
    let runs = engine.runs(&spool::RunFilter::default()).unwrap().len();

    let error = engine
        .resolve(&ResolveOptions::with_strategy("splink"))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "entity type `thing` has no strategy `splink`; it has only the built-in `rules`"
    );
    assert_eq!(
        engine.runs(&spool::RunFilter::default()).unwrap().len(),
        runs
    );

    // Naming the built-in strategy is the same as naming none.
    ingest(&engine, "shop", &[("7", Some("anvil"), Some("K1"))]);
    let report = engine
        .resolve(&ResolveOptions::with_strategy("rules"))
        .unwrap();
    assert_eq!(report.resolved[0].observations_assigned, 1);
}
