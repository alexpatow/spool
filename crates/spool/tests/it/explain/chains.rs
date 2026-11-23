//! Records joined through others, and where an identity would part.

use spool::core::RecordId;

use super::*;

/// One link parts the two: the one between the anvils and the block, which
/// keeps each name together.
fn check_the_cut(explanation: &Explanation, l: &str, left: RecordId, middle: RecordId) {
    let cut = explanation.cut.as_ref().expect("a cut");
    let [link] = &cut.links[..] else {
        panic!("one link parts them");
    };
    assert_eq!(link.rules, ["same_code"]);
    let anvils = "2 records with `shop:1` (2 × `anvil`)";
    let block = "1 record with `shop:3` (1 × `block`)";
    let (with_left, with_right, sizes) = match l {
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
}

/// A constraint between the two is a link of its own, and the one that is
/// followed.
fn a_constraint_is_a_link_of_its_own(engine: &Engine, left: RecordId, right: RecordId) {
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
    let explanation = explain(engine, ("shop", "1"), ("shop", "3"));
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
    check_the_cut(&explanation, &l, left, middle);
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

    a_constraint_is_a_link_of_its_own(&engine, left, right);
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
