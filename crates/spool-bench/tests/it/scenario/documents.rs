//! Documents a scenario stores, and the mentions labelled in them.

use super::*;

#[test]
fn documents_are_stored_and_their_labelled_mentions_are_what_an_extractor_should_find() {
    let dir = tempfile::tempdir().unwrap();
    shelf(dir.path(), LABELS);
    let report = run(dir.path()).unwrap();
    let step = &report.steps[0];

    // Every file but a hidden one is a document, and its name is its key.
    assert_eq!(step.documents.len(), 1);
    let stored = &step.documents[0];
    assert_eq!(
        (stored.source.as_str(), stored.received, stored.inserted),
        ("shelf", 2, 2)
    );
    assert!(step.ingest.is_empty() && step.extract.is_empty());

    // Nothing read the documents, so every labelled mention is missed, and
    // precision has nothing to be a share of.
    let extraction = step.extraction.as_ref().unwrap();
    assert_eq!(
        (
            extraction.labelled,
            extraction.extracted,
            extraction.correct
        ),
        (3, 0, 0)
    );
    assert_eq!(extraction.precision, None);
    assert_eq!(extraction.recall, Some(0.0));
    assert_eq!(extraction.f1, Some(0.0));
    // And there is no record for resolution to be scored on.
    assert_eq!(step.metrics.records, 0);
    // End to end, neither paper has a mention that was found, and the one
    // pair of mentions of a paper is missing.
    let whole = step.end_to_end.as_ref().unwrap();
    assert_eq!((whole.labelled, whole.extracted), (3, 0));
    assert_eq!((whole.true_entities, whole.entities_found), (2, 0));
    assert_eq!((whole.pairs.actual, whole.pairs.recall), (1, Some(0.0)));
    assert_eq!(whole.b_cubed.recall, Some(0.0));

    // A scenario without labelled mentions reports neither.
    let rows = tempfile::tempdir().unwrap();
    project(rows.path());
    write(rows.path(), "bench/papers.csv", PAPERS);
    write(
        rows.path(),
        "bench/scenario.toml",
        &scenario("file = \"papers.csv\""),
    );
    let step = &run(rows.path()).unwrap().steps[0];
    assert_eq!(step.extraction, None);
    assert_eq!(step.end_to_end, None);
}

#[test]
fn labelled_mentions_are_checked_against_the_documents() {
    let cases = [
        (
            "source,document,start,end,true_entity\nshelf,a.txt,2,11,w1\n",
            "needs the columns `source`, `document`, `start`, `end`, `entity_type` and `true_entity`",
        ),
        (
            "source,document,start,end,entity_type,true_entity\nshelf,a.txt,two,11,paper,w1\n",
            "line 2: `start` is not a position",
        ),
        (
            "source,document,start,end,entity_type,true_entity\nshelf,a.txt,2,11,paper,\n",
            "line 2 has no `true_entity`",
        ),
        // Positions count code points: the emoji is one, and the label that
        // counts it as two is off by one.
        (
            "source,document,start,end,entity_type,true_entity,text\n\
             shelf,a.txt,3,12,paper,w1,On Spools\n",
            "the labelled mention of `w1` at 3-12 in document `a.txt` of `shelf`: \
             the label says the text there is `On Spools`, and it is `n Spools `",
        ),
        (
            "source,document,start,end,entity_type,true_entity\nshelf,a.txt,20,40,paper,w1\n",
            "the document is 26 long",
        ),
        (
            "source,document,start,end,entity_type,true_entity\nshelf,a.txt,11,11,paper,w1\n",
            "the document is 26 long",
        ),
    ];
    for (labels, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        shelf(dir.path(), labels);
        let message = scenario_error(run(dir.path()));
        assert!(
            message.contains(expected),
            "expected `{expected}` in: {message}"
        );
    }
}
