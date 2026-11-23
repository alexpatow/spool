//! Splitting an identity in two.

use super::*;

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
