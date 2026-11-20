//! The `extraction/company-news` benchmark: companies named in news items,
//! found by an extractor and then resolved.
//!
//! These tests run extractors, which start `uv` or `bun`, so they are
//! ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool --test news -- --include-ignored
//! ```

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use common::{benchmarks, select};
use spool::{
    DocumentFilter, Engine, ExtractOptions, Manifest, NewDocument, Options, RecordFilter, UserCode,
};
use spool_bench::{BenchReport, Scenario};

fn news() -> PathBuf {
    benchmarks().join("extraction/company-news")
}

fn user_code(work: &Path) -> UserCode {
    UserCode {
        work_dir: work.to_owned(),
        timeout: Duration::from_secs(300),
        ..UserCode::new(news())
    }
}

/// Runs one of the benchmark's scenarios, and returns the engine it ran in.
fn run(file: &str) -> (Engine, BenchReport) {
    let work = tempfile::tempdir().unwrap();
    let engine = Engine::open_in_memory(Options::seeded(0)).unwrap();
    engine.set_user_code(user_code(work.path()));
    let report = Scenario::load(news().join(file))
        .unwrap()
        .run(&engine, |_| Ok(()))
        .unwrap();
    (engine, report)
}

/// Every mention in the store, as the document's key and the text there.
fn mentions(engine: &Engine) -> BTreeSet<(String, u64, String)> {
    select(
        engine,
        "SELECT json_extract_string(evidence, '$.key'),
                json_extract_string(evidence, '$.start'),
                json_extract_string(evidence, '$.text')
         FROM spool.record",
    )
    .into_iter()
    .map(|row| {
        let mut row = row.into_iter().map(Option::unwrap);
        let key = row.next().unwrap();
        let start = row.next().unwrap().parse().unwrap();
        (key, start, row.next().unwrap())
    })
    .collect()
}

fn record_ids(engine: &Engine) -> BTreeSet<String> {
    select(engine, "SELECT record_id::VARCHAR FROM spool.record")
        .into_iter()
        .map(|row| row[0].clone().unwrap())
        .collect()
}

fn rounded(value: Option<f64>) -> Option<f64> {
    value.map(|value| (value * 10_000.0).round() / 10_000.0)
}

#[test]
#[ignore = "runs uv"]
fn extraction_and_resolution_are_measured_apart() {
    // The rule-based extractor: what it finds is scored against the labels,
    // and resolution is scored over the mentions it got right.
    let (engine, report) = run("scenario.toml");
    let (first, last) = (&report.steps[0], &report.steps[1]);
    assert_eq!(
        (first.documents[0].received, first.extract[0].documents),
        (5, 5)
    );
    assert_eq!(
        (first.extract[0].mentions, first.extract[0].inserted),
        (11, 11)
    );
    let extraction = first.extraction.as_ref().unwrap();
    assert_eq!(
        (
            extraction.labelled,
            extraction.extracted,
            extraction.correct
        ),
        (15, 11, 10)
    );

    let extraction = last.extraction.as_ref().unwrap();
    assert_eq!(
        (
            extraction.labelled,
            extraction.extracted,
            extraction.correct
        ),
        (25, 21, 18)
    );
    assert_eq!(rounded(extraction.precision), Some(0.8571));
    assert_eq!(rounded(extraction.recall), Some(0.72));
    assert_eq!(rounded(extraction.f1), Some(0.7826));

    // The three it got wrong: a word before the name, twice, and something
    // that is no company.
    let found = mentions(&engine);
    for wrong in [
        ("02-brightwave-funding.txt", 75, "Yesterday Brightwave Inc"),
        ("08-kestrel-bericht.txt", 75, "Die Kestrel Analytics AG"),
        ("09-falcon-watch.txt", 0, "The Falcon Ltd"),
    ] {
        let wrong = (wrong.0.to_owned(), wrong.1, wrong.2.to_owned());
        assert!(found.contains(&wrong), "{wrong:?} is not in {found:?}");
    }
    // Places count code points, so the name after an emoji is where the
    // label says it is.
    assert!(found.contains(&(
        "02-brightwave-funding.txt".to_owned(),
        12,
        "Brightwave Inc".to_owned()
    )));

    // Resolution is scored over the eighteen mentions that are labelled.
    // The made-up mentions have no true entity and are left out; the two
    // companies that share a name are the one false merge.
    assert_eq!(last.metrics.records, 18);
    assert_eq!(
        (last.metrics.true_entities, last.metrics.identities),
        (10, 9)
    );
    assert_eq!(
        (last.metrics.false_merges, last.metrics.false_splits),
        (1, 0)
    );
    assert_eq!(rounded(last.metrics.pairs.precision), Some(0.8333));
    assert_eq!(last.metrics.pairs.recall, Some(1.0));
    assert_eq!(last.metrics.unassigned, 0);
    // Mentions found in the second batch joined identities made from the
    // first, and none of those changed.
    assert_eq!(last.id_stability.strict, Some(1.0));
    assert_eq!(last.churn.new_records, 10);
    let order = report.order_sensitivity.as_ref().unwrap();
    assert_eq!(order.agreement, Some(1.0));

    // End to end, what extraction got wrong costs. The seven mentions it
    // missed are in no identity, so 18 of the 28 pairs of mentions of one
    // company are missing, though every company has a mention that was
    // found. The three it made up are each alone, so they are in no wrong
    // pair, and count against B-cubed precision as mentions of nothing.
    let whole = last.end_to_end.as_ref().unwrap();
    assert_eq!((whole.labelled, whole.extracted), (25, 21));
    assert_eq!((whole.true_entities, whole.entities_found), (10, 10));
    assert_eq!(
        (
            whole.pairs.true_positives,
            whole.pairs.predicted,
            whole.pairs.actual
        ),
        (10, 12, 28)
    );
    assert_eq!(rounded(whole.b_cubed.precision), Some(0.7937));
    assert_eq!(rounded(whole.b_cubed.recall), Some(0.5553));

    // The extractor that reads the labels: extraction is perfect by
    // construction, and what is left is what the rules do with the names.
    // A short name is now there to be missed: "Northwind" is split from
    // "Northwind Traders Ltd".
    let (_, gold) = run("gold.toml");
    let last = &gold.steps[1];
    let extraction = last.extraction.as_ref().unwrap();
    assert_eq!(
        (
            extraction.labelled,
            extraction.extracted,
            extraction.correct
        ),
        (25, 25, 25)
    );
    assert_eq!(extraction.f1, Some(1.0));
    assert_eq!(last.metrics.records, 25);
    assert_eq!(
        (last.metrics.true_entities, last.metrics.identities),
        (10, 10)
    );
    assert_eq!(
        (last.metrics.false_merges, last.metrics.false_splits),
        (1, 1)
    );
    assert_eq!(rounded(last.metrics.pairs.precision), Some(0.9231));
    assert_eq!(rounded(last.metrics.pairs.recall), Some(0.8571));
    // With nothing missed and nothing made up, end to end is resolution.
    let whole = last.end_to_end.as_ref().unwrap();
    assert_eq!(whole.pairs, last.metrics.pairs);
    assert_eq!(whole.b_cubed, last.metrics.b_cubed);
}

#[test]
#[ignore = "runs uv and bun"]
fn the_typescript_extractor_finds_what_the_python_one_finds() {
    let (python, python_report) = run("scenario.toml");
    let (typescript, typescript_report) = run("typescript.toml");

    // The same mentions make the same records: a record's ID
    // comes from its document, its place and its attributes, and not from
    // which extractor found it.
    assert_eq!(record_ids(&typescript), record_ids(&python));
    assert_eq!(record_ids(&python).len(), 21);
    for (typescript, python) in typescript_report.steps.iter().zip(&python_report.steps) {
        assert_eq!(typescript.extraction, python.extraction);
        assert_eq!(typescript.metrics, python.metrics);
    }
    assert_eq!(typescript_report.steps[0].extract[0].extractor, "suffix_ts");
}

/// A request a model's API was sent.
struct Asked {
    path: String,
    authorization: String,
    body: serde_json::Value,
}

/// Stands in for a model's API: answers every chat completion with
/// `answer` as the model's message, and reports what it was asked.
fn model(answer: serde_json::Value) -> (u16, mpsc::Receiver<Asked>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (asked, requests) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
            let (mut length, mut authorization) = (0, String::new());
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                let Some((name, value)) = line.trim_end().split_once(':') else {
                    break;
                };
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap(),
                    "authorization" => authorization = value.trim().to_owned(),
                    _ => {}
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let reply = serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": answer.to_string()}}]
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
            let sent = asked.send(Asked {
                path,
                authorization,
                body: serde_json::from_slice(&body).unwrap(),
            });
            if sent.is_err() {
                break;
            }
        }
    });
    (port, requests)
}

#[test]
#[ignore = "runs uv"]
fn the_example_that_asks_a_language_model_places_what_the_model_quotes() {
    // The model names each company once, as it was asked to, and also says
    // two things an extractor must not pass on: a quote the text does not
    // have, and an entity type the project does not declare.
    let (port, requests) = model(serde_json::json!({
        "mentions": [
            {"entity_type": "company", "quote": "Brightwave", "attributes": {"name": "Brightwave"}},
            {"entity_type": "company", "quote": "Brightwave Inc",
             "attributes": {"name": "Brightwave Inc", "listed": false, "aliases": ["BW"]}},
            {"entity_type": "company", "quote": "Kestrel Analytics AG",
             "attributes": {"name": "Kestrel Analytics AG"}},
            {"entity_type": "company", "quote": "Brightwave Incorporated", "attributes": {}},
            {"entity_type": "person", "quote": "university", "attributes": {}},
        ]
    }));

    // The extractor reads where the model is from its environment. The
    // engine starts it with `uv`, so a `uv` that sets the environment first
    // says so without changing this process's own.
    let work = tempfile::tempdir().unwrap();
    let uv = work.path().join("uv-with-a-model");
    fs::write(
        &uv,
        format!(
            "#!/bin/sh\nexport OPENAI_BASE_URL=http://127.0.0.1:{port}/v1\n\
             export OPENAI_API_KEY=a-key SPOOL_EXTRACT_MODEL=a-model\nexec uv \"$@\"\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&uv, fs::Permissions::from_mode(0o755)).unwrap();

    let engine = Engine::open_in_memory(Options::seeded(0)).unwrap();
    engine.set_user_code(UserCode {
        uv: uv.into(),
        ..user_code(&work.path().join("tasks"))
    });
    engine.apply(&Manifest::load(news()).unwrap()).unwrap();
    let key = "02-brightwave-funding.txt";
    let text = fs::read_to_string(news().join("documents/batch-1").join(key)).unwrap();
    engine
        .add_documents(
            "news",
            &[NewDocument {
                key: key.to_owned(),
                uri: None,
                text: text.clone(),
            }],
        )
        .unwrap();

    let report = engine.extract(&ExtractOptions::with("llm")).unwrap();
    assert_eq!((report.documents, report.mentions), (1, 4));

    // Each quote is placed wherever the text has it, the longest first, so
    // "Brightwave" is a mention only where it is not part of "Brightwave
    // Inc". These are the document's labelled mentions.
    let document = engine.documents(&DocumentFilter::default()).unwrap()[0].id;
    let mut found: Vec<(u64, u64, serde_json::Value)> = engine
        .records(&RecordFilter {
            document: Some(document),
            ..RecordFilter::default()
        })
        .unwrap()
        .into_iter()
        .map(|record| {
            let evidence = record.evidence.unwrap();
            (
                evidence["start"].as_u64().unwrap(),
                evidence["end"].as_u64().unwrap(),
                record.payload,
            )
        })
        .collect();
    found.sort_by_key(|(start, _, _)| *start);
    assert_eq!(
        found,
        vec![
            // An attribute the source does not list is left out.
            (12, 26, serde_json::json!({"name": "Brightwave Inc"})),
            (85, 99, serde_json::json!({"name": "Brightwave Inc"})),
            (
                125,
                145,
                serde_json::json!({"name": "Kestrel Analytics AG"})
            ),
            (173, 183, serde_json::json!({"name": "Brightwave"})),
        ]
    );

    // The model was asked once, about this document, with what the
    // project's `extract.md` says a company is.
    let asked = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(requests.try_recv().is_err());
    assert_eq!(asked.path, "/v1/chat/completions");
    assert_eq!(asked.authorization, "Bearer a-key");
    assert_eq!(asked.body["model"], "a-model");
    let messages = asked.body["messages"].as_array().unwrap();
    let system = messages[0]["content"].as_str().unwrap();
    assert!(system.contains("## company"), "{system}");
    assert!(
        system.contains("A product, a person, a place or a public body is not a company."),
        "{system}"
    );
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], text);
}

#[test]
#[ignore = "runs uv"]
fn a_mention_that_is_taken_back_is_left_out_of_what_the_store_is_compared_with() {
    // A mention is taken back by its key, which is its locator.
    let (engine, _) = run("gold.toml");
    let key = select(
        &engine,
        "SELECT source_key FROM spool.record
         WHERE json_extract_string(evidence, '$.key') = '01-northwind-expands.txt'
           AND json_extract_string(evidence, '$.start') = '145'",
    )[0][0]
        .clone()
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let news = news();
    let scenario = format!(
        r#"
project = "{project}"

[truth]
mentions = "{project}/mentions.csv"

[[step]]
name = "batch one"

[[step.documents]]
dir = "{project}/documents/batch-1"
source = "news"

[[step.extract]]
extractor = "gold"

[[step]]
name = "batch two"
rebuild = true

[[step.documents]]
dir = "{project}/documents/batch-2"
source = "news"

[[step.extract]]
extractor = "gold"

[[step.retract]]
source = "news"
entity_type = "company"
keys = ["{key}"]
"#,
        project = news.display(),
    );
    let file = dir.path().join("withdrawn.toml");
    fs::write(&file, scenario).unwrap();

    let work = tempfile::tempdir().unwrap();
    let engine = Engine::open_in_memory(Options::seeded(0)).unwrap();
    engine.set_user_code(user_code(work.path()));
    let report = Scenario::load(&file)
        .unwrap()
        .run(&engine, |_| Ok(()))
        .unwrap();

    let last = &report.steps[1];
    assert_eq!(
        (last.retract[0].retracted, last.retract[0].detached),
        (1, 1)
    );
    assert_eq!(last.metrics.records, 24);
    // End to end it is neither a mention that was found nor one that was
    // missed.
    let whole = last.end_to_end.as_ref().unwrap();
    assert_eq!((whole.labelled, whole.extracted), (24, 24));
    assert_eq!(whole.pairs, last.metrics.pairs);
    // The store it is set against extracts the same documents, and holds
    // the mention no more than this one does.
    let rebuild = last.rebuild.as_ref().unwrap();
    assert_eq!(rebuild.records, 24);
    assert_eq!(rebuild.metrics.records, 24);
    // Neither do the two orders of arrival.
    assert_eq!(report.order_sensitivity.as_ref().unwrap().records, 24);
    engine.verify().unwrap();
}
