//! Earlier versions of a row, moved along with the latest when it is placed.

use super::*;

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
fn an_undo_that_was_returned_is_committed_as_it_was_returned() {
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
    let moved = author(
        &engine,
        vec![Op::Assign {
            record: second,
            from: Some(anvil),
            to: Some(hammer),
        }],
    );

    let preview = engine.undo(moved.seq, reviewer(), None, false).unwrap();
    assert!(preview.committed.is_none());
    let committed = engine.commit(preview.changeset.clone()).unwrap();
    let entry = engine.log().unwrap().pop().unwrap();
    assert_eq!(entry.seq, committed.seq);
    assert_eq!(entry.changeset.ops, preview.changeset.ops);
    assert_eq!(placed_versions(&engine).1, [Some(hammer), Some(anvil)]);
    engine.verify().unwrap();
}

#[test]
fn a_draft_is_committed_as_it_was_stored() {
    for approve in [true, false] {
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
        // The draft places the new version with the earlier one, which is
        // where it goes already, so the draft moves nothing along.
        load(&engine, "shop", &[("1", Some("anvil"), Some("K9"))]);
        let options = ResolveOptions {
            leave_for_review: true,
            ..ResolveOptions::default()
        };
        let report = engine.resolve(&options).unwrap();
        let draft = engine.draft(report.resolved[0].draft.unwrap()).unwrap();
        let (versions, _) = placed_versions(&engine);
        assert_eq!(
            draft.changeset.ops,
            [Op::Assign {
                record: versions[1],
                from: None,
                to: Some(anvil),
            }]
        );

        // A reviewer then moves the earlier version, and the draft that is
        // approved after that does not take it back.
        author(
            &engine,
            vec![Op::Assign {
                record: versions[0],
                from: Some(anvil),
                to: Some(hammer),
            }],
        );
        let committed = match approve {
            true => engine.approve_draft(draft.changeset.id).unwrap(),
            false => engine
                .commit_with_versions(draft.changeset.clone())
                .unwrap(),
        };
        let entry = engine.log().unwrap().pop().unwrap();
        assert_eq!(entry.seq, committed.seq);
        assert_eq!(entry.changeset.ops, draft.changeset.ops);
        assert_eq!(placed_versions(&engine).1, [Some(hammer), Some(anvil)]);
        engine.verify().unwrap();
    }
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
