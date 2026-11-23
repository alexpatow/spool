//! Rows a scenario takes back, measured against resolving what is left afresh.

use super::*;

/// A library whose papers are one work when their titles or their numbers
/// agree. Paper 2 is what joins 1 and 3: it has the title of one and the
/// number of the other.
fn changing(root: &Path) {
    write(
        root,
        "bench/sources/library.yaml",
        "key: id\ncolumns: [id, title, number]\n",
    );
    write(
        root,
        "bench/entities/paper/entity.yaml",
        "label: title\nfeatures: [title, number]\nmapping:\n  library:\n    \
         title: { from: title, transform: lower }\n    number: number\n",
    );
    for (rule, sql) in [
        ("same_title", "l.title = r.title"),
        ("same_number", "l.number = r.number"),
    ] {
        write(
            root,
            &format!("bench/entities/paper/rules/block/{rule}.sql"),
            sql,
        );
        write(
            root,
            &format!("bench/entities/paper/rules/match/{rule}.sql"),
            sql,
        );
    }
    write(
        root,
        "bench/first.csv",
        "id,title,number,work\n\
         1,On Spools,,w1\n\
         2,on spools,n1,w1\n\
         3,Spools,n1,w1\n\
         4,Threads,,w2\n\
         5,Bobbins,,w3\n",
    );
    write(
        root,
        "bench/more.csv",
        "id,title,number,work\n6,threads,,w2\n7,Bobbins,,w3\n",
    );
    write(
        root,
        "bench/withdrawn.csv",
        "id,why\n5,lost\n7,lost\n4,kept\n",
    );
    write(
        root,
        "bench/back.csv",
        "id,title,number,work\n2,on spools,n1,w1\n",
    );
    write(
        root,
        "bench/scenario.toml",
        r#"
[truth]
column = "work"

[[step]]
name = "first"

[[step.ingest]]
file = "first.csv"
source = "library"
entity_type = "paper"

[[step]]
name = "more"

[[step.ingest]]
file = "more.csv"
source = "library"
entity_type = "paper"

[[step]]
name = "withdrawn"
rebuild = true

[[step.retract]]
keys = ["2", "9"]
source = "library"
entity_type = "paper"

[[step.retract]]
file = "withdrawn.csv"
where = "why = 'lost'"
source = "library"
entity_type = "paper"

[[step]]
name = "again"
rematch = true

[[step]]
name = "back"
rebuild = true

[[step.ingest]]
file = "back.csv"
source = "library"
entity_type = "paper"
"#,
    );
}

#[test]
fn a_step_takes_rows_back_and_what_is_left_is_compared_with_resolving_it_afresh() {
    let dir = tempfile::tempdir().unwrap();
    changing(dir.path());
    let report = run(dir.path()).unwrap();
    let [first, more, withdrawn, again, back] = &report.steps[..] else {
        panic!("five steps");
    };

    assert_eq!(first.retract, []);
    assert_eq!(first.rebuild, None);
    assert_eq!(more.metrics.records, 7);
    assert_eq!(more.metrics.identities, 3);
    assert_eq!(more.metrics.pairs.f1, Some(1.0));

    // Three rows are taken back, by key and by the rows of a file, and a
    // key the library never had changes nothing.
    let taken: Vec<(u64, u64, u64, u64)> = withdrawn
        .retract
        .iter()
        .map(|taken| {
            assert_eq!(
                (taken.source.as_str(), taken.entity_type.as_str()),
                ("library", "paper")
            );
            (
                taken.received,
                taken.unknown,
                taken.retracted,
                taken.detached,
            )
        })
        .collect();
    assert_eq!(taken, [(2, 1, 1, 1), (2, 0, 2, 2)]);
    assert_eq!(withdrawn.resolve.resolved, []);
    // Paper 2 was what joined 1 and 3, so their identity is put to a
    // reviewer. Papers 5 and 7 were all of theirs.
    let raised: Vec<u64> = withdrawn
        .retract
        .iter()
        .map(|taken| taken.possible_splits)
        .collect();
    assert_eq!(raised, [1, 0]);
    assert_eq!(
        (&first.possible_splits, &more.possible_splits),
        (&None, &None)
    );
    // The item asks about one pair, which the truth says is one work.
    let asked = PossibleSplits {
        open: 1,
        records: 2,
        pairs: 1,
        wrong_pairs: 0,
        wrong: 0,
    };
    assert_eq!(withdrawn.possible_splits, Some(asked.clone()));

    // What is scored is what the library still holds: papers 1 and 3 of one
    // work, and two of another. Papers 1 and 3 are still one identity,
    // which is right, though nothing that is left says so.
    let metrics = &withdrawn.metrics;
    assert_eq!((metrics.records, metrics.true_entities), (4, 2));
    assert_eq!((metrics.identities, metrics.unassigned), (2, 0));
    assert_eq!(metrics.pairs.f1, Some(1.0));
    // The three that left are the three that changed.
    let stability = &withdrawn.id_stability;
    assert_eq!((stability.records, stability.unchanged), (7, 4));
    assert_eq!(withdrawn.churn.records_reassigned, 3);

    // Resolved afresh, the four rows that are left are three identities:
    // 1 and 3 never meet.
    let rebuild = withdrawn.rebuild.as_ref().unwrap();
    assert_eq!((rebuild.records, rebuild.same_group), (4, 2));
    assert_eq!(rebuild.agreement, Some(0.5));
    assert_eq!(
        (
            rebuild.pairs_in_both,
            rebuild.pairs_only_kept,
            rebuild.pairs_only_rebuilt
        ),
        (1, 1, 0)
    );
    // The pair only the store has together is the pair the item asks about.
    assert_eq!(rebuild.pairs_only_kept_asked, 1);
    assert_eq!(rebuild.pair_agreement, Some(0.5));
    assert_eq!(rebuild.metrics.records, 4);
    assert_eq!(rebuild.metrics.identities, 3);
    assert_eq!(rebuild.metrics.false_splits, 1);
    assert_eq!(rebuild.metrics.pairs.recall, Some(0.5));

    // Matching everything again examines the four, and finds nothing to
    // change: no rule says that 1 and 3 differ.
    let rematch = &again.resolve.resolved[0];
    assert_eq!(rematch.candidates, 1);
    let suggestions = rematch.suggestions.as_ref().unwrap();
    assert_eq!((suggestions.merges.len(), suggestions.splits.len()), (0, 0));
    assert_eq!(again.metrics, withdrawn.metrics);
    assert_eq!(again.rebuild, None);
    // Nothing joins them either, so the question stays open.
    assert_eq!(again.possible_splits, Some(asked));
    assert_eq!(again.id_stability.strict, Some(1.0));

    // Paper 2 is sent again and is back where it was, and the store agrees
    // with one that was never told it had gone.
    assert_eq!(back.ingest[0].inserted, 0);
    assert_eq!(back.ingest[0].duplicates, 1);
    assert_eq!(back.ingest[0].restored, 1);
    assert_eq!(back.ingest[0].returned, 1);
    assert_eq!(back.resolve.resolved, []);
    assert_eq!(back.metrics.records, 5);
    assert_eq!(back.metrics.identities, 2);
    assert_eq!(back.metrics.pairs.f1, Some(1.0));
    let rebuild = back.rebuild.as_ref().unwrap();
    assert_eq!((rebuild.records, rebuild.same_group), (5, 5));
    assert_eq!(rebuild.pair_agreement, Some(1.0));
    assert_eq!(rebuild.metrics, back.metrics);
    // What joined 1 and 3 is back, so nothing is left to ask.
    assert_eq!(rebuild.pairs_only_kept_asked, 0);
    assert_eq!(
        back.possible_splits,
        Some(PossibleSplits {
            open: 0,
            records: 0,
            pairs: 0,
            wrong_pairs: 0,
            wrong: 0,
        })
    );

    // The other order is tried on what is left at the end: five rows.
    let order = report.order_sensitivity.as_ref().unwrap();
    assert_eq!((order.records, order.same_group), (5, 5));

    assert_eq!(run(dir.path()).unwrap(), report);
}

#[test]
fn a_retraction_in_a_scenario_is_checked_when_it_is_loaded() {
    for (retract, expected) in [
        (
            "source = \"library\"\nentity_type = \"paper\"\n",
            "a retraction needs exactly one of `keys`, `file` and `dataset`",
        ),
        (
            "keys = [\"1\"]\nfile = \"withdrawn.csv\"\nsource = \"library\"\nentity_type = \"paper\"\n",
            "a retraction needs exactly one of `keys`, `file` and `dataset`",
        ),
        (
            "keys = [\"1\"]\nwhere = \"true\"\nsource = \"library\"\nentity_type = \"paper\"\n",
            "`where` chooses among the rows of a `file` or a `dataset`",
        ),
        (
            "file = \"withdrawn.csv\"\nmember = \"a.csv\"\nsource = \"library\"\nentity_type = \"paper\"\n",
            "`member` names a file within a `dataset`",
        ),
        (
            "file = \"nowhere.csv\"\nsource = \"library\"\nentity_type = \"paper\"\n",
            "there is no file",
        ),
        (
            "keys = [\"1\"]\nsource = \"archive\"\nentity_type = \"paper\"\n",
            "the mapping of `paper` has no block for the source",
        ),
        (
            "keys = [\"1\"]\nsource = \"library\"\nentity_type = \"book\"\n",
            "the project has no entity type `book`",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        changing(dir.path());
        write(
            dir.path(),
            "bench/scenario.toml",
            &format!(
                "[truth]\ncolumn = \"work\"\n\n[[step]]\nname = \"gone\"\n\n[[step.retract]]\n{retract}"
            ),
        );
        let message = scenario_error(run(dir.path()));
        assert!(message.contains(expected), "{retract}: {message}");
        assert!(message.contains("step `gone`"), "{message}");
    }
}
