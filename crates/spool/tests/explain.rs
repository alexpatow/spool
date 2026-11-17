//! Explanations: why two observations share an identity, or do not.

mod common;

use common::*;
use serde_json::json;
use spool::core::{Actor, ConstraintKind, EntityType, IdGenerator, Op, Policy, SeededIds, Verdict};
use spool::{
    Disposition, DraftStatus, Engine, Error, Explanation, IngestOptions, LinkKind, Options,
    Outcome, ResolveOptions, ReviewFilter, ReviewStatus, Rows, RunStatus, Scope, SqlFile,
    StrategyRef,
};

fn reviewer() -> Actor {
    Actor::User("reviewer".to_owned())
}

fn resolve(engine: &Engine) {
    engine.resolve(&ResolveOptions::default()).unwrap();
}

fn explain(engine: &Engine, a: (&str, &str), b: (&str, &str)) -> Explanation {
    engine
        .explain(observation(engine, a.0, a.1), observation(engine, b.0, b.1))
        .unwrap()
}

/// Whether a sentence of the summary starts with `start`.
fn says(explanation: &Explanation, start: &str) -> bool {
    explanation
        .summary
        .iter()
        .any(|sentence| sentence.starts_with(start))
}

fn held(explanation: &Explanation, group: &str) -> Vec<(String, Option<bool>)> {
    let rules = explanation.rules.as_ref().expect("a reading of the rules");
    match group {
        "block" => &rules.block,
        "no_match" => &rules.no_match,
        _ => &rules.match_,
    }
    .iter()
    .map(|result| (result.name.clone(), result.holds))
    .collect()
}

#[test]
fn a_match_is_explained_by_the_run_that_judged_it_and_the_rules_that_held() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    let (first, second) = (
        observation(&engine, "shop", "1"),
        observation(&engine, "shop", "2"),
    );
    let identity = identity(&engine, "shop", "1").unwrap();

    let explanation = engine.explain(first, second).unwrap();
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    // Whichever is named first, `left` is the one a rule calls `l`.
    assert_eq!(engine.explain(second, first).unwrap(), explanation);
    assert_eq!(explanation.left.observation.id, first.min(second));
    assert_eq!(explanation.right.observation.id, first.max(second));

    for side in [&explanation.left, &explanation.right] {
        assert_eq!(side.observation.identity, Some(identity));
        assert_eq!(
            json!(side.features),
            json!({ "name": "anvil", "code": null })
        );
        let placed = side.placed.as_ref().unwrap();
        assert_eq!(placed.seq, engine.head().unwrap());
        assert_eq!(placed.actor, Actor::Engine);
        assert_eq!(placed.run.map(|run| run.0), Some(3));
    }

    let [comparison] = &explanation.comparisons[..] else {
        panic!("one run compared them");
    };
    assert_eq!(comparison.run.0, 2);
    assert_eq!(comparison.status, RunStatus::Succeeded);
    assert_eq!(comparison.strategy, Some(StrategyRef::rules()));
    assert_eq!(comparison.blocked_by, ["same_name"]);
    assert_eq!(comparison.draft, None);
    let [judgement] = &comparison.judgements[..] else {
        panic!("one judgement");
    };
    assert_eq!(judgement.verdict, Verdict::Match);
    assert_eq!(judgement.score, None);
    assert_eq!(
        judgement.detail,
        Some(json!({ "block": ["same_name"], "no_match": [], "match": ["same_name"] }))
    );

    let [link] = &explanation.path[..] else {
        panic!("they are linked directly");
    };
    assert_eq!(
        (link.from, link.to, link.kind, link.run.map(|run| run.0)),
        (
            first.min(second),
            first.max(second),
            LinkKind::Match,
            Some(2)
        )
    );

    let rules = explanation.rules.as_ref().unwrap();
    assert!(rules.candidate);
    assert_eq!(rules.verdict, Some(Verdict::Match));
    assert_eq!(rules.block[1].predicate, "l.name = r.name");
    assert_eq!(
        held(&explanation, "block"),
        [
            ("same_code".to_owned(), None),
            ("same_name".to_owned(), Some(true))
        ]
    );
    assert_eq!(
        held(&explanation, "no_match"),
        [("different_code".to_owned(), None)]
    );
    assert!(explanation.constraints.is_empty());
    assert!(explanation.review_items.is_empty());

    let (l, r) = if first < second {
        ("1", "2")
    } else {
        ("2", "1")
    };
    assert_eq!(
        explanation.summary,
        [
            format!("`shop:{l}` and `shop:{r}` belong to the same identity, {identity}."),
            "Run 2 (strategy `rules`) judged them `match` (match rule `same_name` held)."
                .to_owned(),
            "Both were placed at log position 1 by `engine` (reconcile run 3, \"reconcile new observations\")."
                .to_owned(),
            "By the rules as they are now, block rule `same_name` makes them a candidate pair and match rule `same_name` holds, so they would be judged `match`."
                .to_owned(),
        ]
    );
}

#[test]
fn two_members_that_were_never_compared_are_joined_through_others() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), Some("K1")),
            ("3", Some("block"), Some("K1")),
        ],
    );
    resolve(&engine);
    let middle = observation(&engine, "shop", "2");

    let explanation = explain(&engine, ("shop", "1"), ("shop", "3"));
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    assert!(explanation.comparisons.is_empty());
    let (left, right) = (
        explanation.left.observation.id,
        explanation.right.observation.id,
    );
    assert_eq!(
        explanation
            .path
            .iter()
            .map(|link| (link.from, link.to, link.kind))
            .collect::<Vec<_>>(),
        [
            (left, middle, LinkKind::Match),
            (middle, right, LinkKind::Match)
        ]
    );
    assert!(says(&explanation, "No run has compared them."));
    assert!(says(
        &explanation,
        "Nothing links the two directly. They are joined through 1 other member: `shop:"
    ));
    // One of the two has no code, so the rule about codes says nothing.
    let rules = explanation.rules.as_ref().unwrap();
    assert!(!rules.candidate);
    assert_eq!(rules.verdict, None);
    assert_eq!(
        held(&explanation, "block"),
        [
            ("same_code".to_owned(), None),
            ("same_name".to_owned(), Some(false))
        ]
    );
    assert!(says(
        &explanation,
        "By the rules as they are now, no block rule holds for the pair, so a run would not compare them. Block rule `same_code` cannot be decided, because a feature it reads is null."
    ));

    // A constraint between the two is a link of its own, and the one that
    // is followed.
    engine
        .author(
            reviewer(),
            Some("the same block".to_owned()),
            vec![Op::Constrain {
                kind: ConstraintKind::MustLink,
                a: left,
                b: right,
            }],
        )
        .unwrap();
    let explanation = explain(&engine, ("shop", "1"), ("shop", "3"));
    let [link] = &explanation.path[..] else {
        panic!("they are linked directly");
    };
    assert_eq!((link.kind, link.run), (LinkKind::MustLink, None));
    let [constraint] = &explanation.constraints[..] else {
        panic!("one constraint");
    };
    assert_eq!(constraint.constraint.kind, ConstraintKind::MustLink);
    assert_eq!(
        (constraint.from, constraint.to),
        (engine.head().unwrap(), None)
    );
    assert!(says(
        &explanation,
        "A `must_link` constraint committed at log position 2 says they belong together"
    ));
}

#[test]
fn a_no_match_rule_explains_why_two_stay_apart() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[
            ("1", Some("hammer"), Some("H1")),
            ("2", Some("hammer"), Some("H2")),
            ("3", Some("tongs"), None),
        ],
    );
    resolve(&engine);

    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::DifferentIdentities);
    assert!(explanation.path.is_empty());
    let [comparison] = &explanation.comparisons[..] else {
        panic!("one run compared them");
    };
    assert_eq!(comparison.judgements[0].verdict, Verdict::NoMatch);
    assert_eq!(
        explanation.rules.as_ref().unwrap().verdict,
        Some(Verdict::NoMatch)
    );
    assert!(says(&explanation, "`shop:"));
    assert!(says(
        &explanation,
        "Run 2 (strategy `rules`) judged them `no_match` (no_match rule `different_code` held)."
    ));
    assert!(says(
        &explanation,
        "By the rules as they are now, block rule `same_name` makes them a candidate pair and no_match rule `different_code` holds, so they would be judged `no_match`."
    ));

    // No block rule finds the hammer and the tongs, so no run looked.
    let explanation = explain(&engine, ("shop", "1"), ("shop", "3"));
    assert_eq!(explanation.outcome, Outcome::DifferentIdentities);
    assert!(explanation.comparisons.is_empty());
    assert!(says(&explanation, "No run has compared them."));
    assert!(!explanation.rules.as_ref().unwrap().candidate);
}

#[test]
fn what_a_reviewer_did_by_hand_is_told_apart_from_what_a_run_did() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[
            ("1", Some("hammer"), Some("H1")),
            ("2", Some("hammer"), Some("H2")),
        ],
    );
    resolve(&engine);
    let (kept, merged) = (
        identity(&engine, "shop", "1").unwrap(),
        identity(&engine, "shop", "2").unwrap(),
    );

    // The rules keep the hammers apart. A reviewer knows better.
    engine
        .author(
            reviewer(),
            Some("one hammer, relabelled".to_owned()),
            vec![Op::Merge {
                from: merged,
                into: kept,
            }],
        )
        .unwrap();
    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    assert!(explanation.path.is_empty(), "a `no_match` is not a link");
    assert!(says(
        &explanation,
        "No chain of matches, constraints or versions joins them within the identity"
    ));
    assert!(says(
        &explanation,
        "`shop:2` was placed at log position 2 by `user:reviewer` (\"one hammer, relabelled\")."
    ));
    assert!(says(
        &explanation,
        "`shop:1` was placed at log position 1 by `engine` (reconcile run 3, \"reconcile new observations\")."
    ));
    assert!(says(&explanation, "No run takes an identity apart."));

    // The reviewer thinks again and splits them, for good.
    engine
        .split(
            kept,
            &[observation(&engine, "shop", "2")],
            true,
            reviewer(),
            None,
        )
        .unwrap();
    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::DifferentIdentities);
    let [constraint] = &explanation.constraints[..] else {
        panic!("one constraint");
    };
    assert_eq!(constraint.constraint.kind, ConstraintKind::CannotLink);
    assert!(says(
        &explanation,
        "A `cannot_link` constraint committed at log position 3 keeps them apart"
    ));

    // A constraint that was taken back is still part of what happened.
    engine
        .author(
            reviewer(),
            None,
            vec![Op::Unconstrain {
                kind: ConstraintKind::CannotLink,
                a: explanation.left.observation.id,
                b: explanation.right.observation.id,
            }],
        )
        .unwrap();
    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert!(says(
        &explanation,
        "A `cannot_link` constraint between them was in force from log position 3 until position 4."
    ));
}

#[test]
fn an_observation_left_for_review_is_explained_by_its_review_item() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("3", None, Some("H1"))],
    );
    resolve(&engine);
    ingest(&engine, "depot", &[("9", Some("anvil"), Some("H1"))]);
    resolve(&engine);
    let waiting = observation(&engine, "depot", "9");

    let explanation = explain(&engine, ("depot", "9"), ("shop", "1"));
    assert_eq!(explanation.outcome, Outcome::Unassigned);
    let [item] = &explanation.review_items[..] else {
        panic!("one review item");
    };
    assert_eq!(item.observations, [waiting]);
    assert_eq!(item.status, ReviewStatus::Open);
    assert!(says(
        &explanation,
        "Run 5 (strategy `rules`) judged them `match` (match rule `same_name` held)."
    ));
    assert!(says(
        &explanation,
        "Review item 1 (`possible_merge`) is open and names `depot:9`: what it matched belongs to more than one identity"
    ));
    let unplaced = match explanation.left.observation.id == waiting {
        true => &explanation.left,
        false => &explanation.right,
    };
    assert_eq!(unplaced.placed, None);

    // Review items can be listed by the observation they name.
    let named = |id| {
        engine
            .review_items(&ReviewFilter {
                observation: Some(id),
                ..ReviewFilter::default()
            })
            .unwrap()
    };
    assert_eq!(named(waiting), std::slice::from_ref(item));
    assert_eq!(named(observation(&engine, "shop", "1")), []);

    engine
        .dismiss_review(item.id, &reviewer(), Some("neither"))
        .unwrap();
    let explanation = explain(&engine, ("depot", "9"), ("shop", "1"));
    assert!(says(
        &explanation,
        "Review item 1 (`possible_merge`), which names `depot:9`, was dismissed by `user:reviewer`."
    ));
}

#[test]
fn a_later_version_of_a_row_is_linked_to_the_one_before() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    ingest(&engine, "shop", &[("1", Some("anvil, large"), None)]);
    resolve(&engine);
    let [earlier, later] = observations(&engine, "shop", "1")[..] else {
        panic!("two versions");
    };

    let explanation = engine.explain(earlier, later).unwrap();
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    let [link] = &explanation.path[..] else {
        panic!("they are linked directly");
    };
    assert_eq!(link.kind, LinkKind::Version);
    // Both are `shop:1`, so the sentences name them by ID.
    assert!(says(
        &explanation,
        &format!("`{later}` is a later version of `{earlier}`")
    ));
}

#[test]
fn the_rules_are_read_as_they_are_now_and_the_run_as_it_was() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    resolve(&engine);

    // The project stops matching on names.
    let mut stricter = thing();
    stricter
        .match_
        .retain(|rule| rule.name.as_str() != "same_name");
    engine.apply(&manifest_of(vec![stricter])).unwrap();

    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    assert_eq!(
        explanation.comparisons[0].judgements[0].verdict,
        Verdict::Match
    );
    let rules = explanation.rules.as_ref().unwrap();
    assert!(rules.candidate);
    assert_eq!(rules.verdict, None);
    assert!(says(
        &explanation,
        "By the rules as they are now, block rule `same_name` makes them a candidate pair and no `no_match` or `match` rule holds, so they would get no judgement."
    ));
    assert!(says(
        &explanation,
        "That is not what run 2 recorded, so the rules or the features have changed since."
    ));

    // With the rule back, the two readings agree and nothing is said.
    engine.apply(&manifest_of(vec![thing()])).unwrap();
    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(
        explanation.rules.as_ref().unwrap().verdict,
        Some(Verdict::Match)
    );
    assert!(!says(&explanation, "That is not what run"));
}

#[test]
fn a_draft_that_waits_or_was_rejected_explains_why_nothing_came_of_a_match() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    let report = engine
        .resolve(&ResolveOptions {
            leave_for_review: true,
            ..ResolveOptions::default()
        })
        .unwrap();
    let draft = report.resolved[0].draft.unwrap();

    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::Unassigned);
    assert_eq!(explanation.comparisons[0].status, RunStatus::Drafted);
    assert_eq!(explanation.comparisons[0].draft, Some(DraftStatus::Open));
    assert!(says(&explanation, "Neither `shop:"));
    assert!(says(
        &explanation,
        "Run 2 (strategy `rules`) judged them `match` (match rule `same_name` held). Its draft waits for a decision."
    ));

    engine.reject_draft(draft, Some("not yet")).unwrap();
    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(
        explanation.comparisons[0].draft,
        Some(DraftStatus::Rejected)
    );
    assert!(says(
        &explanation,
        "Run 2 (strategy `rules`) judged them `match` (match rule `same_name` held). Its draft was rejected."
    ));
}

#[test]
fn a_strategy_of_the_callers_own_is_explained_by_what_it_stored() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", Some("anvil"), None),
        ],
    );
    let run = engine
        .begin_run(
            &StrategyRef::external("by-hand", Some("7".to_owned())),
            &Scope::new(EntityType::new("thing").unwrap()),
        )
        .unwrap();
    let (first, second) = (
        observation(&engine, "shop", "1").to_string(),
        observation(&engine, "shop", "2").to_string(),
    );
    let rows = Rows::from_json(
        &serde_json::from_value::<Vec<serde_json::Map<String, serde_json::Value>>>(json!([{
            "left_id": first,
            "right_id": second,
            "verdict": "match",
            "score": 0.75,
            "detail": "{\"because\":\"a hunch\"}",
        }]))
        .unwrap(),
    )
    .unwrap();
    engine.submit_judgements(run, rows).unwrap();

    // Before the run is reconciled, the judgement is there and nothing has
    // come of it.
    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::Unassigned);
    assert!(says(
        &explanation,
        "Run 2 (strategy `by-hand`) judged them `match` with score 0.75. The run has not been reconciled."
    ));

    engine
        .reconcile_with(run, &Policy::default(), Disposition::Commit)
        .unwrap();
    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    let comparison = &explanation.comparisons[0];
    assert_eq!(
        comparison.strategy,
        Some(StrategyRef::external("by-hand", Some("7".to_owned())))
    );
    assert_eq!(comparison.blocked_by, ["same_name"]);
    assert_eq!(comparison.judgements[0].score, Some(0.75));
    assert_eq!(
        comparison.judgements[0].detail,
        Some(json!({ "because": "a hunch" }))
    );

    // The strategy said nothing about the third anvil, which the block rule
    // had found.
    let explanation = explain(&engine, ("shop", "1"), ("shop", "3"));
    assert_eq!(explanation.outcome, Outcome::DifferentIdentities);
    assert!(explanation.comparisons[0].judgements.is_empty());
    assert!(says(
        &explanation,
        "Run 2 (strategy `by-hand`) made them a candidate pair (block rule `same_name`) and gave no judgement."
    ));
}

#[test]
fn observations_of_different_types_are_never_compared() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let mut widget = thing();
    widget.name = "widget".to_owned();
    widget.features = SqlFile::new(
        "features",
        "SELECT observation_id, payload->>'name' AS name, payload->>'code' AS code
         FROM observation",
    );
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
    resolve(&engine);

    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::DifferentTypes);
    assert_eq!(explanation.rules, None);
    assert!(explanation.comparisons.is_empty());
    assert!(explanation.path.is_empty());
    assert!(
        explanation.summary[0]
            .ends_with("Observations of different entity types are never compared."),
        "{:?}",
        explanation.summary
    );
    assert!(!says(&explanation, "No run has compared them."));
}

#[test]
fn an_explanation_is_about_two_observations_the_store_holds() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), None)]);
    let known = observation(&engine, "shop", "1");
    let unknown: spool::core::ObservationId =
        SeededIds::new(9).identity_id().to_string().parse().unwrap();

    for (a, b) in [(known, unknown), (unknown, known)] {
        let error = engine.explain(a, b).unwrap_err();
        assert!(
            matches!(error, Error::UnknownObservation(id) if id == unknown),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            format!("there is no observation {unknown}")
        );
    }
    let error = engine.explain(known, known).unwrap_err();
    assert!(matches!(error, Error::SameObservation(id) if id == known));
}
