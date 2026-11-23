//! An extraction in batches: what each reads, says and fails, and cancelling one.

use super::*;

#[test]
#[ignore = "runs uv"]
fn a_match_run_that_begins_while_an_extractor_works_does_not_hide_what_it_finds() {
    let news = News::new();
    let go = news.dir.path().join("go");
    let started = news.dir.path().join("started");
    news.extractor(
        "names.py",
        &format!(
            r#"
import os, time
from spool import extractor

@extractor
def find(documents):
    open({started:?}, "w").close()
    while not os.path.exists({go:?}):
        time.sleep(0.02)
    return [d.mention("company", s, e, name="Acme Inc") for d in documents for s, e in d.find("Acme Inc")]
"#
        ),
    );
    let engine = Arc::clone(&news.engine);
    news.add(&[("a", FIRST)]);

    let extracting = {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || engine.extract(&ExtractOptions::with("names")))
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the extractor never started");
        assert!(!extracting.is_finished(), "{:?}", extracting.join());
        std::thread::sleep(Duration::from_millis(20));
    }

    // Reads are answered and writes go through: the run is there to see,
    // more documents arrive, and a match run of the type begins and
    // succeeds. Its ID is later than the extract run's.
    let [run] = &runs(&engine, RunKind::Extract)[..] else {
        panic!("one extract run");
    };
    assert_eq!(run.status, RunStatus::Running);
    news.add(&[("b", SECOND)]);
    let company = EntityType::new("company").unwrap();
    let between = engine
        .begin_run(
            &StrategyRef::external("outside", None),
            &Scope::new(company),
        )
        .unwrap();
    engine
        .reconcile_with(between, &Policy::default(), Disposition::Commit)
        .unwrap();
    assert_eq!(engine.run(between).unwrap().status, RunStatus::Succeeded);
    assert!(between > run.run);
    assert!(!extracting.is_finished());

    fs::write(&go, "").unwrap();
    let report = extracting.join().unwrap().unwrap();
    assert_eq!((report.documents, report.mentions), (1, 1));
    assert!(report.load_run.unwrap() > between);

    // What the extractor found is new to the next resolve.
    let resolved = engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(resolved.resolved[0].new_records, 1);
    assert!(news.identity(id("a", FIRST), 2, 10).is_some());
    // The document that arrived meanwhile is the next extraction's.
    assert_eq!(news.extract("names").unwrap().documents, 1);
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn an_extraction_in_batches_reads_each_document_once_and_says_what_is_left() {
    let news = News::new();
    news.extractor("names.py", NAMES);
    news.add(&[
        ("a", FIRST),
        ("b", SECOND),
        ("c", "Initech Inc."),
        ("d", "Hooli Ltd."),
        ("e", "Nothing here."),
    ]);
    let batch = |again: bool, again_since, limit| {
        let report = news
            .engine
            .extract(&ExtractOptions {
                again,
                since: again_since,
                limit: Some(limit),
                ..ExtractOptions::with("names")
            })
            .unwrap();
        (report.documents, report.left, report.run)
    };

    // Each batch reads what the ones before it did not, and says how many
    // are left for the next.
    assert_eq!(batch(false, None, 2).0, 2);
    let (read, left, _) = batch(false, None, 2);
    assert_eq!((read, left), (2, 1));
    let (read, left, _) = batch(false, None, 2);
    assert_eq!((read, left), (1, 0));
    assert_eq!(batch(false, None, 2).0, 0);

    // Reading them all again in batches: the first batch reads what any
    // reading does, and each after it leaves out what was read since the
    // first began.
    let (read, left, first) = batch(true, None, 2);
    assert_eq!((read, left), (2, 3));
    let (read, left, _) = batch(true, Some(first), 2);
    assert_eq!((read, left), (2, 1));
    let (read, left, _) = batch(true, Some(first), 2);
    assert_eq!((read, left), (1, 0));
    assert_eq!(batch(true, Some(first), 2).0, 0);

    let read: Vec<u64> = news
        .engine
        .documents(&DocumentFilter::default())
        .unwrap()
        .iter()
        .map(|document| document.extractions.len() as u64)
        .collect();
    assert_eq!(read, vec![2; 5], "each document read once in each pass");
    news.engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn a_running_extraction_says_what_it_reads_and_can_be_cancelled() {
    let news = News::new();
    let started = news.dir.path().join("started");
    news.extractor(
        "names.py",
        &format!(
            r#"
import time
from spool import extractor

@extractor
def find(documents):
    open({started:?}, "w").close()
    time.sleep(120)
    return []
"#
        ),
    );
    news.add(&[("a", FIRST), ("b", SECOND)]);
    let extracting = {
        let engine = Arc::clone(&news.engine);
        std::thread::spawn(move || {
            engine.extract(&ExtractOptions {
                limit: Some(1),
                ..ExtractOptions::with("names")
            })
        })
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the extractor never started");
        assert!(!extracting.is_finished(), "{:?}", extracting.join());
        std::thread::sleep(Duration::from_millis(20));
    }
    let [run] = &runs(&news.engine, RunKind::Extract)[..] else {
        panic!("one extract run");
    };

    // The document the run was given says it is being read, and by which
    // run; the other is not read.
    let reading = |key: &str| {
        let documents = news
            .engine
            .documents(&DocumentFilter {
                key: Some(key.to_owned()),
                ..DocumentFilter::default()
            })
            .unwrap();
        documents[0]
            .reading
            .as_ref()
            .map(|reading| (reading.extractor.clone(), reading.run))
    };
    assert_eq!(reading("a"), Some(("names".to_owned(), run.run)));
    assert_eq!(reading("b"), None);
    assert_eq!(
        news.engine
            .document(id("a", FIRST))
            .unwrap()
            .reading
            .unwrap()
            .run,
        run.run
    );

    // Cancelled, the extractor is stopped at once, and the run fails and
    // stores nothing: the document is unread again, and not being read.
    let cancelled = Instant::now();
    news.engine.cancel_run(run.run).unwrap();
    let message = extractor_error(extracting.join().unwrap());
    assert!(cancelled.elapsed() < Duration::from_secs(10));
    assert!(message.contains("it was cancelled"), "{message}");
    let entry = news.engine.run(run.run).unwrap();
    assert_eq!(entry.status, RunStatus::Failed);
    let document = news.engine.document(id("a", FIRST)).unwrap();
    assert_eq!((document.reading, document.extractions.len()), (None, 0));
    let refused = news.engine.cancel_run(run.run).unwrap_err().to_string();
    assert!(refused.contains("cannot be cancelled"), "{refused}");

    // The next extraction reads both.
    news.extractor("names.py", NAMES);
    assert_eq!(news.extract("names").unwrap().documents, 2);
    news.engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn a_document_the_extractor_could_not_read_is_failed_and_read_again_alone() {
    let news = News::new();
    // `b` raises, which fails it and only it; `c` is failed by hand, with
    // what was found in it before.
    news.extractor(
        "names.py",
        r#"
import re
from spool import extractor

@extractor
def find(document):
    if document.key == "b":
        raise RuntimeError("the model answered 500")
    for found in re.finditer(r"[A-Z]\w+ (?:Inc|Ltd)", document.text):
        yield document.mention("company", found.start(), found.end(), name=found.group())
    if document.key == "c":
        yield document.failed("the model's answer was cut off")
"#,
    );
    news.add(&[("a", FIRST), ("b", SECOND), ("c", "Initech Inc.")]);
    let report = news.extract("names").unwrap();
    assert_eq!(report.documents, 3);
    let failed: Vec<(&str, &str)> = report
        .failed
        .iter()
        .map(|failed| (failed.key.as_str(), failed.reason.as_str()))
        .collect();
    assert_eq!(
        failed,
        [
            ("b", "RuntimeError: the model answered 500"),
            ("c", "the model's answer was cut off"),
        ]
    );
    // Nothing found in a failed document is kept.
    assert_eq!(report.mentions, 2);
    assert!(news.mentions(id("c", "Initech Inc.")).is_empty());
    let document = news.engine.document(id("b", SECOND)).unwrap();
    assert_eq!(
        document.extractions[0].failure.as_deref(),
        Some("RuntimeError: the model answered 500")
    );
    assert_eq!(
        report.stderr,
        ["news:b: RuntimeError: the model answered 500"]
    );

    // The next extraction reads the two that failed, and not the one that
    // was read.
    news.extractor("names.py", NAMES);
    let again = news.extract("names").unwrap();
    assert_eq!((again.documents, again.failed.len()), (2, 0));
    assert_eq!(again.mentions, 3);
    assert_eq!(news.extract("names").unwrap().documents, 0);
    let read = news.engine.document(id("c", "Initech Inc.")).unwrap();
    assert_eq!(read.extractions.len(), 2);
    assert_eq!(read.extractions[1].failure, None);
    news.engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn a_failure_or_a_mention_returned_on_its_own_is_one_result() {
    let news = News::new();
    news.extractor(
        "names.py",
        r#"
from spool import extractor

@extractor
def find(document):
    if document.key == "b":
        return document.failed("model unavailable")
    start, end = document.find("Acme Inc")[0]
    return document.mention("company", start, end, name="Acme Inc")
"#,
    );
    news.add(&[("a", FIRST), ("b", SECOND)]);
    let report = news.extract("names").unwrap();
    assert_eq!(report.mentions, 1, "{report:?}");
    let failed: Vec<(&str, &str)> = report
        .failed
        .iter()
        .map(|failed| (failed.key.as_str(), failed.reason.as_str()))
        .collect();
    assert_eq!(failed, [("b", "model unavailable")]);
}

#[test]
#[ignore = "runs uv"]
fn a_document_that_cannot_be_read_is_not_tried_again_in_the_same_pass() {
    let news = News::new();
    news.extractor(
        "names.py",
        r#"
from spool import extractor

@extractor
def find(document):
    raise RuntimeError("no key")
"#,
    );
    news.add(&[("a", FIRST), ("b", SECOND)]);
    let first = news
        .engine
        .extract(&ExtractOptions {
            limit: Some(1),
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!((first.documents, first.failed.len(), first.left), (1, 1, 1));
    // A batch after it names its run, and does not get `a` again.
    let second = news
        .engine
        .extract(&ExtractOptions {
            limit: Some(1),
            since: Some(first.run),
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!(second.failed[0].key, "b");
    assert_eq!(second.left, 0);
}

#[test]
#[ignore = "runs uv"]
fn a_task_written_from_an_iterator_and_written_again_keeps_the_last() {
    let news = News::new();
    // Without the decorator: a failure written first, then replaced with
    // mentions given by a generator, which gives them only once.
    news.extractor(
        "names.py",
        r#"
import os
from spool import ExtractTask

if os.environ.get("SPOOL_TASK_DIR"):
    task = ExtractTask(os.environ["SPOOL_TASK_DIR"])
    first, second = task.documents
    task.write([second.failed("the first try")])
    task.write(
        document.mention("company", start, end, name=document.text[start:end])
        for document in (first, second)
        for start, end in document.find("Acme Inc")
    )
"#,
    );
    news.add(&[("a", FIRST), ("b", SECOND)]);
    let report = news.extract("names").unwrap();
    assert_eq!((report.mentions, report.failed.len()), (2, 0), "{report:?}");
    let read = news.engine.document(id("b", SECOND)).unwrap();
    assert_eq!(read.extractions[0].failure, None);
}
