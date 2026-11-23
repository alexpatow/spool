//! What runs and rules said about a pair, as they were and as they are now.

use super::*;

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
