//! Explanations: why two records share an identity, or do not.

use serde_json::json;
use spool::core::{Actor, ConstraintKind, EntityType, IdGenerator, Op, Policy, SeededIds, Verdict};
use spool::{
    Disposition, DraftStatus, Engine, Error, Explanation, LinkKind, Options, Outcome,
    ResolveOptions, ReviewFilter, ReviewStatus, Rows, RunStatus, Scope, SqlFile, StrategyRef,
};
use spool_testing::*;

fn reviewer() -> Actor {
    Actor::User("reviewer".to_owned())
}

fn resolve(engine: &Engine) {
    engine.resolve(&ResolveOptions::default()).unwrap();
}

fn explain(engine: &Engine, a: (&str, &str), b: (&str, &str)) -> Explanation {
    engine
        .explain(record_id(engine, a.0, a.1), record_id(engine, b.0, b.1))
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
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    let (first, second) = (
        record_id(&engine, "shop", "1"),
        record_id(&engine, "shop", "2"),
    );
    let identity = identity(&engine, "shop", "1").unwrap();

    let explanation = engine.explain(first, second).unwrap();
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    // Whichever is named first, `left` is the one a rule calls `l`.
    assert_eq!(engine.explain(second, first).unwrap(), explanation);
    assert_eq!(explanation.left.record.id, first.min(second));
    assert_eq!(explanation.right.record.id, first.max(second));

    for side in [&explanation.left, &explanation.right] {
        assert_eq!(side.record.identity, Some(identity));
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
            "Both were placed at log position 1 by `engine` (reconcile run 3, \"reconcile new records\")."
                .to_owned(),
            "By the rules as they are now, block rule `same_name` makes them a candidate pair and match rule `same_name` holds, so they would be judged `match`."
                .to_owned(),
        ]
    );
}

#[test]
#[expect(clippy::too_many_lines)]
fn two_members_that_were_never_compared_are_joined_through_others() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), Some("K1")),
            ("3", Some("block"), Some("K1")),
        ],
    );
    resolve(&engine);
    let middle = record_id(&engine, "shop", "2");

    let explanation = explain(&engine, ("shop", "1"), ("shop", "3"));
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    assert!(explanation.comparisons.is_empty());
    let (left, right) = (explanation.left.record.id, explanation.right.record.id);
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
    // Each link names its records as a person knows them, the rule that
    // made it, and the values the rule read.
    let [first, second] = &explanation.path[..] else {
        panic!("two links");
    };
    assert_eq!(first.from_record.label.as_deref(), Some("anvil"));
    assert_eq!(first.to_record.name, "shop:2");
    assert_eq!(first.strategy.as_deref(), Some("rules"));
    assert_eq!(first.rules, ["same_name"]);
    assert_eq!(
        json!(first.values),
        json!([{ "feature": "name", "from": "anvil", "to": "anvil" }])
    );
    assert_eq!(second.rules, ["same_code"]);
    assert_eq!(
        json!(second.values),
        json!([{ "feature": "code", "from": "K1", "to": "K1" }])
    );
    assert_eq!(second.to_record.label.as_deref(), Some("block"));
    let (l, r) = (
        explanation.left.record.name(),
        explanation.right.record.name(),
    );
    let middle_said = "`anvil` (shop:2)";
    let steps = if l == "shop:1" {
        format!(
            "`shop:1` → {middle_said} (`match` in run 2, rule `same_name`: name \"anvil\") → `shop:3` (`match` in run 2, rule `same_code`: code \"K1\")"
        )
    } else {
        format!(
            "`shop:3` → {middle_said} (`match` in run 2, rule `same_code`: code \"K1\") → `shop:1` (`match` in run 2, rule `same_name`: name \"anvil\")"
        )
    };
    assert!(
        explanation.summary.contains(&format!(
            "Nothing links the two directly. They are joined through 1 other member: {steps}."
        )),
        "{:#?}",
        explanation.summary
    );
    // Each of the two rules made a link every chain between them goes
    // through.
    assert_eq!(explanation.held_by, ["same_code", "same_name"]);
    assert!(says(
        &explanation,
        "Without the links only rule `same_code` made, they would not be together."
    ));
    // One link parts them: the one between the anvils and the block, which
    // keeps each name together.
    let cut = explanation.cut.as_ref().expect("a cut");
    let [link] = &cut.links[..] else {
        panic!("one link parts them");
    };
    assert_eq!(link.rules, ["same_code"]);
    let anvils = "2 records with `shop:1` (2 × `anvil`)";
    let block = "1 record with `shop:3` (1 × `block`)";
    let (with_left, with_right, sizes) = match l.as_str() {
        "shop:1" => (anvils, block, [2, 1]),
        _ => (block, anvils, [1, 2]),
    };
    assert_eq!(
        cut.sides
            .iter()
            .map(|side| side.records)
            .collect::<Vec<_>>(),
        sizes
    );
    assert_eq!(link.from, if l == "shop:1" { middle } else { left });
    assert!(
        explanation.summary.contains(&format!(
            "Removing 1 link would part them, leaving {with_left} and {with_right}."
        )),
        "{:#?}",
        explanation.summary
    );
    assert!(r.starts_with("shop:"));
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
fn an_identity_is_explained_by_how_much_of_it_hangs_on_chains_and_where_it_parts() {
    let engine = engine();
    // Three anvils by name, one of which shares a code with two blocks.
    load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", Some("Anvil"), None),
            ("4", Some("anvil"), Some("K1")),
            ("5", Some("block"), Some("K1")),
            ("6", Some("block"), Some("K1")),
        ],
    );
    // `Anvil` is another name to the rules, but the same name to a reader.
    engine
        .author(
            reviewer(),
            None,
            vec![Op::Constrain {
                kind: ConstraintKind::MustLink,
                a: record_id(&engine, "shop", "3"),
                b: record_id(&engine, "shop", "1"),
            }],
        )
        .unwrap();
    resolve(&engine);
    let identity = identity(&engine, "shop", "1").unwrap();
    assert_eq!(members(&engine, identity).len(), 6);

    let explained = engine.explain_identity(identity).unwrap();
    assert_eq!((explained.id, explained.current), (identity, identity));
    assert_eq!(explained.label.as_deref(), Some("anvil"));
    assert_eq!(explained.records, 6);
    assert_eq!(explained.names, 2);
    assert_eq!(
        json!(explained.top_names),
        json!([{ "name": "anvil", "records": 4 }, { "name": "block", "records": 2 }])
    );
    // Linked: 1, 2 and 4 by name, 3 and 1 by the constraint, and 4, 5 and
    // 6 by code.
    assert_eq!(explained.pairs, 15);
    assert_eq!(explained.linked_pairs, 7);
    assert_eq!(explained.chained_pairs, 8);
    assert_eq!(explained.parts, 1);
    assert_eq!(explained.longest_chain, 3);

    // The constraint alone holds `Anvil` on, and two links hold the blocks.
    let sides = |split: &spool::Split| {
        split
            .sides
            .iter()
            .map(|side| side.records)
            .collect::<Vec<_>>()
    };
    let [blocks, anvil] = &explained.weakest[..] else {
        panic!("two places it parts: {:#?}", explained.weakest);
    };
    assert_eq!(sides(blocks), [2, 4]);
    assert_eq!(blocks.links.len(), 2);
    for link in &blocks.links {
        assert_eq!(link.to_record.name, "shop:4");
        assert_eq!(link.rules, ["same_code"]);
        assert_eq!(link.values[0].feature, "code");
    }
    assert_eq!(
        json!(blocks.sides[0].names),
        json!([{ "name": "block", "records": 2 }])
    );
    assert_eq!(sides(anvil), [1, 5]);
    let [constraint] = &anvil.links[..] else {
        panic!("one link holds `Anvil` on");
    };
    assert_eq!(constraint.kind, LinkKind::MustLink);
    assert_eq!(constraint.from_record.label.as_deref(), Some("Anvil"));
    assert!(constraint.rules.is_empty() && constraint.values.is_empty());

    assert_eq!(
        explained.summary,
        [
            "`anvil` is a `thing` identity of 6 records with 2 different names.",
            "Of its 15 pairs of records, 7 are linked directly and 8 are together only through a chain of others, the longest of at least 3 links.",
            "Without the 3 links only rule `same_name` made, it would fall into 3 pieces: 3 (2 × `block`, 1 × `anvil`), 2 (2 × `Anvil`), 1 (1 × `anvil`).",
            "Without the 2 links only rule `same_code` made, it would fall into 2 pieces: 4 (4 × `anvil`), 2 (2 × `block`).",
            "Removing 2 links would part 2 records (2 × `block`) from the other 4 (4 × `anvil`).",
            "Removing 1 link would part 1 record (1 × `Anvil`) from the other 5 (3 × `anvil`, 2 × `block`).",
        ]
    );

    // The list of identities says as much of each, without walking it.
    let listed = engine
        .identities(&spool::IdentityFilter {
            sort: Some(spool::Sort::descending(spool::IdentitySort::Names)),
            ..spool::IdentityFilter::default()
        })
        .unwrap();
    let [summary] = &listed[..] else {
        panic!("one identity");
    };
    assert_eq!(
        (
            summary.members,
            summary.names,
            summary.linked_pairs,
            summary.unlinked_pairs
        ),
        (6, 2, 7, 8)
    );

    // The blocks match each other by name and by code, so that link stays
    // without either rule.
    assert_eq!(
        explained
            .rules
            .iter()
            .map(|hold| (hold.rule.as_str(), hold.links, hold.alone, hold.pieces))
            .collect::<Vec<_>>(),
        [("same_name", 4, 3, 3), ("same_code", 3, 2, 2)]
    );

    assert!(matches!(
        engine.explain_identity("00000000-0000-7000-8000-000000000000".parse().unwrap()),
        Err(Error::UnknownIdentity(_))
    ));
}

#[test]
fn a_no_match_rule_explains_why_two_stay_apart() {
    let engine = engine();
    load(
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
    load(
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
        "`shop:1` was placed at log position 1 by `engine` (reconcile run 3, \"reconcile new records\")."
    ));
    assert!(says(&explanation, "No run takes an identity apart."));

    // The reviewer thinks again and splits them, for good.
    engine
        .split(
            kept,
            &[record_id(&engine, "shop", "2")],
            true,
            None,
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
                a: explanation.left.record.id,
                b: explanation.right.record.id,
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
fn an_record_left_for_review_is_explained_by_its_review_item() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("3", None, Some("H1"))],
    );
    resolve(&engine);
    load(&engine, "depot", &[("9", Some("anvil"), Some("H1"))]);
    resolve(&engine);
    let waiting = record_id(&engine, "depot", "9");

    let explanation = explain(&engine, ("depot", "9"), ("shop", "1"));
    assert_eq!(explanation.outcome, Outcome::Unassigned);
    let [item] = &explanation.review_items[..] else {
        panic!("one review item");
    };
    assert_eq!(item.records, [waiting]);
    assert_eq!(item.status, ReviewStatus::Open);
    assert!(says(
        &explanation,
        "Run 5 (strategy `rules`) judged them `match` (match rule `same_name` held)."
    ));
    assert!(says(
        &explanation,
        "Review item 1 (`possible_merge`) is open and names `depot:9`: what it matched belongs to more than one identity"
    ));
    let unplaced = match explanation.left.record.id == waiting {
        true => &explanation.left,
        false => &explanation.right,
    };
    assert_eq!(unplaced.placed, None);

    // Review items can be listed by the record they name.
    let named = |id| {
        engine
            .review_items(&ReviewFilter {
                record: Some(id),
                ..ReviewFilter::default()
            })
            .unwrap()
    };
    assert_eq!(named(waiting), std::slice::from_ref(item));
    assert_eq!(named(record_id(&engine, "shop", "1")), []);

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
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    load(&engine, "shop", &[("1", Some("anvil, large"), None)]);
    resolve(&engine);
    let [earlier, later] = records(&engine, "shop", "1")[..] else {
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
    load(
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
    load(
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
fn a_run_reconciled_again_is_explained_by_its_latest_draft() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    engine
        .resolve(&ResolveOptions {
            leave_for_review: true,
            ..ResolveOptions::default()
        })
        .unwrap();
    let run = explain(&engine, ("shop", "1"), ("shop", "2")).comparisons[0].run;

    // The second draft takes the place of the first, which is superseded.
    let again = engine.reconcile(run, &Policy::default()).unwrap();
    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.comparisons[0].draft, Some(DraftStatus::Open));
    assert!(says(
        &explanation,
        "Run 2 (strategy `rules`) judged them `match` (match rule `same_name` held). Its draft waits for a decision."
    ));

    engine.approve_draft(again.changeset.id).unwrap();
    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    assert_eq!(
        explanation.comparisons[0].draft,
        Some(DraftStatus::Committed)
    );
    assert!(!says(
        &explanation,
        "Run 2 (strategy `rules`) judged them `match` (match rule `same_name` held). Its draft"
    ));
}

#[test]
fn a_rule_that_cannot_be_applied_to_a_pair_does_not_end_the_explanation() {
    // The match rule is safe on what the block rule lets through, and only
    // there, which is all a run asks of it.
    let mut numbered = thing();
    numbered.block = vec![SqlFile::new(
        "same_number",
        "l.code = r.code AND TRY_CAST(l.code AS INTEGER) IS NOT NULL",
    )];
    numbered.no_match.clear();
    numbered.match_ = vec![SqlFile::new(
        "same_number",
        "CAST(l.code AS INTEGER) = CAST(r.code AS INTEGER)",
    )];
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&manifest_of(vec![numbered])).unwrap();
    load(
        &engine,
        "shop",
        &[
            ("1", None, Some("12")),
            ("2", None, Some("12")),
            ("3", None, Some("twelve")),
            ("4", None, Some("twelve")),
        ],
    );
    resolve(&engine);

    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    assert_eq!(
        held(&explanation, "match"),
        [("same_number".to_owned(), Some(true))]
    );

    let explanation = explain(&engine, ("shop", "3"), ("shop", "4"));
    assert_eq!(explanation.outcome, Outcome::DifferentIdentities);
    assert_eq!(
        held(&explanation, "block"),
        [("same_number".to_owned(), Some(false))]
    );
    assert_eq!(
        held(&explanation, "match"),
        [("same_number".to_owned(), None)]
    );
    let rules = explanation.rules.as_ref().unwrap();
    assert!(!rules.candidate);
    assert_eq!(rules.verdict, None);
    assert_eq!(
        rules.failed.as_deref(),
        Some(
            "Conversion Error: Could not convert string 'twelve' to INT32 when casting from source column code"
        )
    );
    assert_eq!(
        explanation.summary.last().unwrap(),
        "By the rules as they are now, no block rule holds for the pair, so a run would not compare them."
    );

    // A null feature is not a failure.
    load(&engine, "shop", &[("5", None, None)]);
    let explanation = explain(&engine, ("shop", "1"), ("shop", "5"));
    let rules = explanation.rules.as_ref().unwrap();
    assert_eq!((rules.match_[0].holds, &rules.failed), (None, &None));

    // The engine goes on reading after an explanation that met a rule SQL
    // refused.
    let explanation = explain(&engine, ("shop", "3"), ("shop", "4"));
    assert!(explanation.rules.as_ref().unwrap().failed.is_some());
    assert_eq!(
        engine.run(spool::core::RunId(1)).unwrap().status,
        RunStatus::Succeeded
    );

    // With a block rule that lets the pair through, a run would reach the
    // rule that fails.
    let mut unguarded = thing();
    unguarded.block = vec![SqlFile::new("same_number", "l.code = r.code")];
    unguarded.no_match.clear();
    unguarded.match_ = vec![SqlFile::new(
        "same_number",
        "CAST(l.code AS INTEGER) = CAST(r.code AS INTEGER)",
    )];
    engine.apply(&manifest_of(vec![unguarded])).unwrap();
    let explanation = explain(&engine, ("shop", "3"), ("shop", "4"));
    let rules = explanation.rules.as_ref().unwrap();
    assert!(rules.candidate);
    assert_eq!(rules.verdict, None);
    assert!(says(
        &explanation,
        "By the rules as they are now, block rule `same_number` makes them a candidate pair, and the `no_match` and `match` rules cannot be applied to the pair's values, so a run of the rules that compared them would fail (Conversion Error: "
    ));
}

#[test]
fn a_constraint_that_was_removed_leaves_two_together_with_nothing_between_them() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    resolve(&engine);
    // Nothing about these tongs resembles the anvil.
    load(&engine, "depot", &[("9", Some("tongs"), None)]);
    let (anvil, tongs) = (
        record_id(&engine, "shop", "1"),
        record_id(&engine, "depot", "9"),
    );
    let must_link = |op: fn(ConstraintKind, spool::core::RecordId, spool::core::RecordId) -> Op| {
        engine
            .author(
                reviewer(),
                None,
                vec![op(ConstraintKind::MustLink, anvil, tongs)],
            )
            .unwrap();
    };
    must_link(|kind, a, b| Op::Constrain { kind, a, b });
    resolve(&engine);
    must_link(|kind, a, b| Op::Unconstrain { kind, a, b });

    // A run placed the tongs, because of a constraint that is gone.
    let explanation = explain(&engine, ("shop", "1"), ("depot", "9"));
    assert_eq!(explanation.outcome, Outcome::SameIdentity);
    assert!(explanation.path.is_empty());
    assert!(
        explanation.right.placed.as_ref().unwrap().run.is_some()
            && explanation.left.placed.as_ref().unwrap().run.is_some()
    );
    assert!(
        !explanation
            .summary
            .iter()
            .any(|sentence| sentence.contains("by hand")),
        "{:#?}",
        explanation.summary
    );
    assert!(says(
        &explanation,
        "No chain of matches, constraints or versions joins them within the identity as it is now. Either what joined them has since been removed, or a changeset placed one of them directly."
    ));
    assert!(says(
        &explanation,
        "A `must_link` constraint between them was in force from log position 2 until position 4."
    ));
}

#[test]
fn a_strategy_of_the_callers_own_is_explained_by_what_it_stored() {
    let engine = engine();
    load(
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
        record_id(&engine, "shop", "1").to_string(),
        record_id(&engine, "shop", "2").to_string(),
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
fn records_of_different_types_are_never_compared() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let mut widget = thing();
    widget.name = "widget".to_owned();
    engine.apply(&manifest_of(vec![thing(), widget])).unwrap();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    engine
        .load("shop", "widget", rows(&[("2", Some("anvil"), None)]))
        .unwrap();
    resolve(&engine);

    let explanation = explain(&engine, ("shop", "1"), ("shop", "2"));
    assert_eq!(explanation.outcome, Outcome::DifferentTypes);
    assert_eq!(explanation.rules, None);
    assert!(explanation.comparisons.is_empty());
    assert!(explanation.path.is_empty());
    assert!(
        explanation.summary[0].ends_with("Records of different entity types are never compared."),
        "{:?}",
        explanation.summary
    );
    assert!(!says(&explanation, "No run has compared them."));
}

#[test]
fn an_explanation_reads_the_rules_of_the_declaration_its_features_were_built_from() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("anvil"), None)],
    );
    resolve(&engine);
    let (one, two) = (
        record_id(&engine, "shop", "1"),
        record_id(&engine, "shop", "2"),
    );

    // The same type with its feature columns under other names, as a
    // project saved while `spool dev` runs would have them.
    let mut renamed = thing();
    renamed.entity = entity(
        Some("label"),
        &[("label", "name"), ("number", "code")],
        &SOURCES,
    );
    renamed.block = vec![SqlFile::new("same_name", "l.label = r.label")];
    renamed.no_match = vec![SqlFile::new("different_code", "l.number <> r.number")];
    renamed.match_ = vec![SqlFile::new("same_name", "l.label = r.label")];
    let (first, second) = (things(), manifest_of(vec![renamed]));

    let applying = std::sync::atomic::AtomicBool::new(true);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..60 {
                engine.apply(&second).unwrap();
                engine.apply(&first).unwrap();
            }
            applying.store(false, std::sync::atomic::Ordering::Relaxed);
        });
        // Whichever declaration an explanation meets, its rules are those
        // its feature rows were built for.
        while applying.load(std::sync::atomic::Ordering::Relaxed) {
            let explanation = engine.explain(one, two).unwrap();
            let Some(rules) = &explanation.rules else {
                continue;
            };
            let features = explanation.left.features.as_ref().unwrap();
            let name = if features.contains_key("label") {
                "label"
            } else {
                "name"
            };
            let rule = rules
                .block
                .iter()
                .find(|rule| rule.name == "same_name")
                .unwrap();
            assert_eq!(rule.predicate, format!("l.{name} = r.{name}"));
            assert_eq!(rule.holds, Some(true));
            assert_eq!(rules.failed, None);
        }
    });
}

#[test]
fn an_explanation_is_about_two_records_the_store_holds() {
    let engine = engine();
    load(&engine, "shop", &[("1", Some("anvil"), None)]);
    let known = record_id(&engine, "shop", "1");
    let unknown: spool::core::RecordId =
        SeededIds::new(9).identity_id().to_string().parse().unwrap();

    for (a, b) in [(known, unknown), (unknown, known)] {
        let error = engine.explain(a, b).unwrap_err();
        assert!(
            matches!(error, Error::UnknownRecord(id) if id == unknown),
            "{error}"
        );
        assert_eq!(error.to_string(), format!("there is no record {unknown}"));
    }
    let error = engine.explain(known, known).unwrap_err();
    assert!(matches!(error, Error::SameRecord(id) if id == known));
}
