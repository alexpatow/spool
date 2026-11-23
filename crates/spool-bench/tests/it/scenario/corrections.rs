//! Corrections a scenario makes as a reviewer would.

use super::*;

fn reviewed(root: &Path) {
    project(root);
    write(
        root,
        "bench/papers.csv",
        &PAPERS.replace("3,Threads,", "3,Threads,w2"),
    );
    write(root, "bench/more.csv", MORE);
    write(root, "bench/scenario.toml", REVIEWED);
}

#[test]
fn corrections_are_applied_as_a_reviewer_would_and_measured_after_each_step() {
    let dir = tempfile::tempdir().unwrap();
    reviewed(dir.path());
    let report = run(dir.path()).unwrap();
    let [first, mistake, right, second, review] = &report.steps[..] else {
        panic!("five steps");
    };

    assert_eq!(first.corrections, []);
    assert_eq!(first.churn.new_records, 4);
    assert_eq!(first.churn.identities_created, 3);
    assert_eq!(first.churn.created_per_thousand, Some(750.0));
    assert_eq!(first.split_recovery.splits, 0);
    assert_eq!(first.override_persistence.share, None);

    // The merge puts two works in one identity. Whoever held the merged ID
    // is redirected, so only the strict measure counts it as a change.
    let kinds = |step: &StepReport| -> Vec<String> {
        step.corrections
            .iter()
            .map(|correction| correction.kind.clone())
            .collect()
    };
    assert_eq!(kinds(mistake), ["merge"]);
    assert_eq!(mistake.corrections[0].name.as_deref(), Some("wrong"));
    assert_eq!(mistake.corrections[0].seq, mistake.head);
    assert_eq!(mistake.metrics.false_merges, 1);
    let stability = &mistake.id_stability;
    assert_eq!(
        (
            stability.records,
            stability.unchanged,
            stability.unchanged_following_redirects
        ),
        (4, 3, 4)
    );
    let churn = &mistake.churn;
    assert_eq!(
        (
            churn.identities_created,
            churn.identities_merged,
            churn.records_reassigned
        ),
        (0, 1, 1)
    );
    // Nothing arrived, so there is no rate.
    assert_eq!(churn.merged_per_thousand, None);

    // The undo brings the merged identity back, and the split makes one.
    assert_eq!(kinds(right), ["undo", "split"]);
    assert_eq!(right.metrics.false_merges, 0);
    assert_eq!(right.metrics.identities, 4);
    let churn = &right.churn;
    assert_eq!(
        (
            churn.identities_created,
            churn.identities_merged,
            churn.records_reassigned
        ),
        (1, 0, 2)
    );
    assert_eq!(right.split_recovery.splits, 1);
    assert_eq!(right.split_recovery.share, Some(1.0));
    assert_eq!(right.override_persistence.constraints, 1);
    assert_eq!(right.override_persistence.share, Some(1.0));

    // The next batch has a paper that matches both sides of the split. It is
    // left for review, and the sides stay apart.
    assert_eq!(second.ingest[0].inserted, 3);
    assert_eq!(second.ingest[0].superseded, 1);
    assert_eq!(second.resolve.resolved[0].review.len(), 1);
    assert_eq!(second.metrics.unassigned, 1);
    assert_eq!(second.split_recovery.kept_apart, 1);
    assert_eq!(second.override_persistence.honoured, 1);
    assert_eq!(second.id_stability.strict, Some(1.0));
    // The row that changed went where its earlier version is, though the
    // titles no longer match.
    assert_eq!(second.metrics.records, 7);
    assert_eq!(second.metrics.false_splits, 1);

    // The reviewer places it, and a run has nothing left to do.
    assert_eq!(kinds(review), ["assign"]);
    assert_eq!(review.metrics.unassigned, 0);
    assert_eq!(review.resolve.resolved, []);
    assert_eq!(review.split_recovery.share, Some(1.0));

    // Without the corrections, these batches give the same groups in either
    // order: seven records in three identities.
    let order = report.order_sensitivity.as_ref().unwrap();
    assert_eq!((order.records, order.same_group), (7, 7));
    assert_eq!((order.pairs_in_both, order.pairs_in_either), (5, 5));
    assert_eq!(order.pair_agreement, Some(1.0));

    // The same scenario gives the same report.
    assert_eq!(run(dir.path()).unwrap(), report);
}

#[test]
fn a_correction_that_cannot_be_made_names_its_step() {
    let cases = [
        (
            "merge = [\"library:3\", \"library:9\"]",
            "no record is `library:9`",
        ),
        // The two are one identity already.
        (
            "merge = [\"library:1\", \"library:2\"]",
            "changeset rejected",
        ),
        ("split = [\"library:3\"]", "identity"),
    ];
    for (correction, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        reviewed(dir.path());
        write(
            dir.path(),
            "bench/scenario.toml",
            &REVIEWED.replace("merge = [\"library:3\", \"library:4\"]", correction),
        );
        let message = scenario_error(run(dir.path()));
        assert!(
            message.contains("step `a mistake`, correction 1"),
            "{message}"
        );
        assert!(
            message.contains(expected),
            "expected `{expected}` in: {message}"
        );
    }
}

#[test]
fn corrections_are_checked_when_the_scenario_is_loaded() {
    let cases = [
        (
            "merge = [\"a:1\", \"a:2\"]\nsplit = [\"a:1\"]",
            "exactly one of",
        ),
        ("reason = \"none\"", "exactly one of"),
        ("split = []", "a split needs records"),
        ("merge = [\"a:1\", \"2\"]", "`2` does not name a record"),
        ("merge = [\"a:1\"]", "scenario.toml"),
        ("undo = \"later\"", "no earlier correction is named `later`"),
        (
            "name = \"x\"\nmerge = [\"a:1\", \"a:2\"]\n[[step.correct]]\nname = \"x\"\nundo = \"x\"",
            "two corrections are named `x`",
        ),
        ("dismiss = [\"a:1\"]", "unknown field `dismiss`"),
    ];
    for (correction, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "bench/scenario.toml",
            &format!(
                "[truth]\ncolumn = \"work\"\n[[step]]\nname = \"review\"\n[[step.correct]]\n{correction}\n"
            ),
        );
        let message = scenario_error(Scenario::load(dir.path().join("bench")));
        assert!(
            message.contains(expected),
            "expected `{expected}` in: {message}"
        );
    }
}
