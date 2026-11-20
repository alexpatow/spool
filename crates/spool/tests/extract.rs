//! Documents, and extractors: files in the project that read documents and
//! say what they mention.
//!
//! The tests that run an extractor start `uv`, which fetches a Python and
//! `pyarrow` the first time, or `bun`, so they are ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool --test extract -- --include-ignored
//! ```

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use spool::core::{Actor, DocumentId, EntityType, IdentityId, Policy};
use spool::{
    Disposition, DocumentFilter, Engine, Error, ExtractOptions, ExtractReport, Manifest,
    NewDocument, Options, RecordEntry, RecordFilter, ResolveOptions, RunFilter, RunKind, RunStatus,
    Scope, StrategyRef, UserCode, locator,
};
use spool_testing::members;

/// A project on disk that declares a `company` by its name, and the
/// extractors a test writes into it.
struct News {
    dir: tempfile::TempDir,
    engine: Arc<Engine>,
}

/// Finds a capitalised word followed by `Inc` or `Ltd`.
const NAMES: &str = r#"
import re
from spool import extractor

@extractor
def find(document):
    for found in re.finditer(r"[A-Z]\w+ (?:Inc|Ltd)", document.text):
        yield document.mention("company", found.start(), found.end(), name=found.group())
"#;

/// Writes one mention of the first document without the helper, with
/// `START` as the column of where it starts.
const BY_HAND: &str = r#"
import json
import os
from pathlib import Path

import pyarrow
import pyarrow.parquet

directory = os.environ.get("SPOOL_TASK_DIR")
if directory:
    task = json.loads((Path(directory) / "task.json").read_text())
    documents = pyarrow.parquet.read_table(Path(directory) / task["inputs"]["documents"])
    pyarrow.parquet.write_table(
        pyarrow.table(
            {
                "document_id": documents.column("document_id").to_pylist()[:1],
                "entity_type": ["company"],
                "start": START,
                "end": pyarrow.array([10.4], pyarrow.float64()),
                "attributes": ['{"name": "Acme Inc"}'],
            }
        ),
        Path(directory) / task["output"],
    )
"#;

const FIRST: &str = "🚀 Acme Inc opened an office. Globex Ltd did not.";
const SECOND: &str = "Société Ltd bought Acme Inc.";

impl News {
    fn new() -> Self {
        Self::with_engine(
            tempfile::tempdir().unwrap(),
            Engine::open_in_memory(Options::seeded(1)).unwrap(),
        )
    }

    fn with_engine(dir: tempfile::TempDir, engine: Engine) -> Self {
        let news = Self {
            engine: Arc::new(engine),
            dir,
        };
        for source in ["news", "wire"] {
            news.write(&format!("sources/{source}.yaml"), "columns: [name, form]\n");
        }
        news.write(
            "entities/company/entity.yaml",
            "features: [name]\nmapping:\n  \
             news:\n    name: { from: name, transform: lower }\n  \
             wire:\n    name: { from: name, transform: lower }\n",
        );
        news.write(
            "entities/company/rules/block/same_name.sql",
            "l.name = r.name",
        );
        news.write(
            "entities/company/rules/match/same_name.sql",
            "l.name = r.name",
        );
        news.engine.set_user_code(UserCode {
            work_dir: news.work(),
            timeout: Duration::from_secs(300),
            ..UserCode::new(news.project())
        });
        news
    }

    fn project(&self) -> PathBuf {
        self.dir.path().join("project")
    }

    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn write(&self, file: &str, contents: &str) {
        let path = self.project().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// Writes an extractor file, compiles the project and applies it.
    fn extractor(&self, file: &str, source: &str) {
        self.write(&format!("extractors/{file}"), source);
        self.engine
            .apply(&Manifest::load(self.project()).unwrap())
            .unwrap();
    }

    fn add(&self, documents: &[(&str, &str)]) -> spool::LoadReport {
        self.engine
            .add_documents("news", &documents_of(documents))
            .unwrap()
    }

    fn extract(&self, extractor: &str) -> spool::Result<ExtractReport> {
        self.engine.extract(&ExtractOptions::with(extractor))
    }

    /// The task directories that were kept.
    fn tasks(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.work()) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("run-"))
            })
            .collect()
    }

    /// The records found in a document, in the order they appear.
    fn mentions(&self, document: DocumentId) -> Vec<RecordEntry> {
        let mut found = self
            .engine
            .records(&RecordFilter {
                document: Some(document),
                ..RecordFilter::default()
            })
            .unwrap();
        found.sort_by_key(|record| record.evidence.as_ref().unwrap()["start"].as_u64());
        found
    }

    /// The identity of the mention of a company at a place in a document.
    fn identity(&self, document: DocumentId, start: u64, end: u64) -> Option<IdentityId> {
        let found = self
            .engine
            .records(&RecordFilter::keyed(
                "news",
                locator(document, "company", start, end),
            ))
            .unwrap();
        found.last().and_then(|record| record.identity)
    }
}

fn documents_of(documents: &[(&str, &str)]) -> Vec<NewDocument> {
    documents
        .iter()
        .map(|(key, text)| NewDocument {
            key: (*key).to_owned(),
            uri: Some(format!("https://news.test/{key}")),
            text: (*text).to_owned(),
        })
        .collect()
}

fn id(key: &str, text: &str) -> DocumentId {
    DocumentId::derive("news", key, text)
}

fn extractor_error(result: spool::Result<ExtractReport>) -> String {
    match result {
        Err(Error::Extractor { extractor, message }) => {
            assert_eq!(extractor, "names");
            message
        }
        other => panic!("expected the extractor to fail, got {other:?}"),
    }
}

fn runs(engine: &Engine, kind: RunKind) -> Vec<spool::RunEntry> {
    engine
        .runs(&RunFilter {
            kind: Some(kind),
            ..RunFilter::default()
        })
        .unwrap()
}

#[test]
fn a_document_is_stored_once_and_a_changed_text_is_its_next_version() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let first = engine
        .add_documents(
            "news",
            &documents_of(&[("a", FIRST), ("b", SECOND), ("a", FIRST)]),
        )
        .unwrap();
    assert_eq!(
        (
            first.received,
            first.inserted,
            first.duplicates,
            first.superseded
        ),
        (3, 2, 1, 0)
    );
    // The store needs no manifest to hold documents, and the call is a run.
    let run = engine.run(first.run).unwrap();
    assert_eq!(run.kind, RunKind::Load);
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(run.entity_type, None);
    assert_eq!(run.load, Some(first.clone()));

    let again = engine
        .add_documents("news", &documents_of(&[("a", FIRST)]))
        .unwrap();
    assert_eq!((again.inserted, again.duplicates), (0, 1));

    // The same key with another text is the next version, and so is the one
    // after it in the same call. The same text from another source is
    // another document.
    let edited = "Acme Inc closed an office.";
    let last = "Acme Inc moved its office.";
    let changed = engine
        .add_documents("news", &documents_of(&[("a", edited), ("a", last)]))
        .unwrap();
    assert_eq!((changed.inserted, changed.superseded), (2, 2));
    let elsewhere = engine
        .add_documents("wire", &documents_of(&[("a", FIRST)]))
        .unwrap();
    assert_eq!((elsewhere.inserted, elsewhere.superseded), (1, 0));

    let original = engine.document(id("a", FIRST)).unwrap();
    assert_eq!(original.source, "news");
    assert_eq!(original.key, "a");
    assert_eq!(original.uri.as_deref(), Some("https://news.test/a"));
    assert_eq!(original.text.as_deref(), Some(FIRST));
    assert_eq!(original.length, FIRST.chars().count() as u64);
    assert_eq!(original.load_run, first.run);
    assert_eq!(original.supersedes, None);
    assert_eq!(original.superseded_by, Some(id("a", edited)));
    assert!(original.extractions.is_empty());
    let middle = engine.document(id("a", edited)).unwrap();
    assert_eq!(middle.supersedes, Some(id("a", FIRST)));
    assert_eq!(middle.superseded_by, Some(id("a", last)));

    // A list leaves the text out, and can leave out what was replaced.
    let all = engine.documents(&DocumentFilter::default()).unwrap();
    assert_eq!(all.len(), 5);
    assert!(all.iter().all(|document| document.text.is_none()));
    assert!(all.windows(2).all(|pair| pair[0].id < pair[1].id));
    let latest: BTreeSet<DocumentId> = engine
        .documents(&DocumentFilter {
            source: Some("news".to_owned()),
            latest: true,
            ..DocumentFilter::default()
        })
        .unwrap()
        .iter()
        .map(|document| document.id)
        .collect();
    assert_eq!(latest, [id("a", last), id("b", SECOND)].into());
    let versions = engine
        .documents(&DocumentFilter {
            source: Some("news".to_owned()),
            key: Some("a".to_owned()),
            ..DocumentFilter::default()
        })
        .unwrap();
    assert_eq!(versions.len(), 3);
    let page = engine
        .documents(&DocumentFilter {
            after: Some(all[1].id),
            limit: 2,
            ..DocumentFilter::default()
        })
        .unwrap();
    assert_eq!(
        page.iter().map(|document| document.id).collect::<Vec<_>>(),
        [all[2].id, all[3].id]
    );

    assert_eq!(engine.info().unwrap().documents, 5);
    assert!(matches!(
        engine.document(id("z", "nothing")),
        Err(Error::UnknownDocument(_))
    ));
    // A document that cannot be stored stores none of the call, and starts
    // no run.
    let before = engine.runs(&RunFilter::default()).unwrap().len();
    let error = engine
        .add_documents("news", &documents_of(&[("c", "text"), ("d", "")]))
        .unwrap_err();
    assert!(matches!(error, Error::Documents(_)), "{error}");
    assert_eq!(engine.runs(&RunFilter::default()).unwrap().len(), before);
    assert_eq!(engine.info().unwrap().documents, 5);
    engine.verify().unwrap();
}

#[test]
fn documents_are_added_by_whoever_is_named() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let own = engine
        .add_documents("news", &documents_of(&[("a", FIRST)]))
        .unwrap();
    let desk = Actor::Agent("desk".to_owned());
    let named = engine
        .acting(desk.clone())
        .add_documents("news", &documents_of(&[("b", SECOND)]))
        .unwrap();
    assert_eq!(engine.run(own.run).unwrap().started_by, Actor::Engine);
    assert_eq!(engine.run(named.run).unwrap().started_by, desk);
}

#[test]
fn a_document_that_holds_again_the_text_it_had_has_a_version_more() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let edited = "Acme Inc closed an office.";
    let add = |documents: &[(&str, &str)]| {
        engine
            .add_documents("news", &documents_of(documents))
            .unwrap()
    };
    add(&[("a", FIRST)]);
    add(&[("a", edited)]);

    // The text is one the store holds as the document's first version, and
    // the document has another since. This is its third.
    let back = add(&[("a", FIRST)]);
    assert_eq!((back.inserted, back.duplicates, back.superseded), (1, 0, 1));
    let third = id("a", FIRST).held_again(id("a", edited));
    let latest = engine
        .documents(&DocumentFilter {
            latest: true,
            ..DocumentFilter::default()
        })
        .unwrap();
    assert_eq!(
        latest
            .iter()
            .map(|document| document.id)
            .collect::<Vec<_>>(),
        [third]
    );
    let document = engine.document(third).unwrap();
    assert_eq!(document.text.as_deref(), Some(FIRST));
    assert_eq!(document.supersedes, Some(id("a", edited)));
    let first = engine.document(id("a", FIRST)).unwrap();
    assert_eq!(
        (first.supersedes, first.superseded_by),
        (None, Some(id("a", edited)))
    );

    // What the document holds now is nothing new, and it can change the
    // same way again, in one call too. Two that say the same one after the
    // other are one version.
    let again = add(&[("a", FIRST)]);
    assert_eq!((again.inserted, again.duplicates), (0, 1));
    let more = add(&[("a", edited), ("a", edited), ("a", FIRST)]);
    assert_eq!((more.inserted, more.duplicates, more.superseded), (2, 1, 2));
    let fourth = id("a", edited).held_again(third);
    let fifth = id("a", FIRST).held_again(fourth);
    let document = engine.document(fifth).unwrap();
    assert_eq!(
        (document.supersedes, document.superseded_by),
        (Some(fourth), None)
    );
    assert_eq!(engine.info().unwrap().documents, 5);
    engine.verify().unwrap();
}

#[test]
fn an_extractor_is_one_the_project_has_and_needs_the_projects_files() {
    let news = News::new();
    news.extractor("names.py", NAMES);
    let error = news.extract("titles").unwrap_err();
    assert_eq!(
        error.to_string(),
        "the project has no extractor `titles`; it has `names`"
    );
    assert_eq!(
        news.engine.info().unwrap().extractors,
        news.engine.manifest().unwrap().extractors
    );

    // Without documents an extraction starts nothing, and is still a run.
    let report = news.extract("names").unwrap();
    assert_eq!(
        (
            report.documents,
            report.mentions,
            report.inserted,
            report.load_run
        ),
        (0, 0, 0, None)
    );
    let run = news.engine.run(report.run).unwrap();
    assert_eq!(run.kind, RunKind::Extract);
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(run.extractor.as_ref().unwrap().name, "names");
    assert_eq!(run.extraction, Some(report));
    assert!(news.tasks().is_empty());

    // A file that changed since the project was built is not run.
    news.add(&[("a", FIRST)]);
    news.write("extractors/names.py", "raise SystemExit(1)\n");
    let message = extractor_error(news.extract("names"));
    assert!(
        message.contains("has changed since the manifest was compiled"),
        "{message}"
    );
    assert_eq!(runs(&news.engine, RunKind::Extract).len(), 1);

    // An engine that was not told where the project is runs no extractor.
    let bare = Engine::open_in_memory(Options::seeded(1)).unwrap();
    assert!(matches!(
        bare.extract(&ExtractOptions::with("names")),
        Err(Error::NoManifest)
    ));
    bare.apply(&news.engine.manifest().unwrap()).unwrap();
    let error = bare.extract(&ExtractOptions::with("names")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("was not told where the project directory is"),
        "{error}"
    );
}

#[test]
#[ignore = "runs uv"]
#[expect(clippy::too_many_lines)]
fn what_an_extractor_finds_is_records_and_resolves_like_rows() {
    let news = News::new();
    news.write(
        "entities/company/extract.md",
        "A company is named with its legal form.\n",
    );
    news.extractor("names.py", NAMES);
    let engine = &news.engine;
    news.add(&[("a", FIRST), ("b", SECOND)]);
    let (first, second) = (id("a", FIRST), id("b", SECOND));

    let report = news.extract("names").unwrap();
    assert_eq!(
        (
            report.documents,
            report.mentions,
            report.inserted,
            report.duplicates,
            report.superseded
        ),
        (2, 4, 4, 0, 0)
    );

    // Each mention is a record of the document's source, keyed by
    // where it was found. Positions count characters, not bytes: the rocket
    // before Acme is one.
    let found = news.mentions(first);
    assert_eq!(found.len(), 2);
    let acme = &found[0];
    assert_eq!(acme.entity_type.as_str(), "company");
    assert_eq!(acme.source, "news");
    assert_eq!(
        acme.key.as_deref(),
        Some(locator(first, "company", 2, 10).as_str())
    );
    assert_eq!(acme.payload, serde_json::json!({ "name": "Acme Inc" }));
    let built = engine.manifest().unwrap();
    let extractor = built.extractor("names").unwrap();
    assert_eq!(
        acme.evidence,
        Some(serde_json::json!({
            "document": first,
            "key": "a",
            "uri": "https://news.test/a",
            "start": 2,
            "end": 10,
            "text": "Acme Inc",
            "extractor": "names",
            "version": extractor.hash,
        }))
    );
    assert_eq!(
        found[1].payload,
        serde_json::json!({ "name": "Globex Ltd" })
    );
    let other = news.mentions(second);
    assert_eq!(
        other[0].payload,
        serde_json::json!({ "name": "Société Ltd" })
    );
    assert_eq!(other[0].evidence.as_ref().unwrap()["text"], "Société Ltd");
    assert_eq!(other[0].identity, None);

    // The extract run names the extractor and the hash of the file that
    // ran, and the records were stored by a load run of their own.
    let run = engine.run(report.run).unwrap();
    assert_eq!(run.kind, RunKind::Extract);
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(run.extraction, Some(report.clone()));
    let named = run.extractor.unwrap();
    assert_eq!(
        (named.name.as_str(), &named.version),
        ("names", &extractor.hash)
    );
    let stored = engine.run(report.load_run.unwrap()).unwrap();
    assert_eq!(stored.kind, RunKind::Load);
    assert_eq!(stored.status, RunStatus::Succeeded);
    assert_eq!(stored.extractor.unwrap().name, "names");
    assert_eq!(stored.load.as_ref().unwrap().inserted, 4);
    assert_eq!(acme.load_run, report.load_run.unwrap());

    // The document says who read it.
    let read = engine.document(first).unwrap().extractions;
    assert_eq!(read.len(), 1);
    assert_eq!(
        (
            read[0].extractor.as_str(),
            &read[0].version,
            read[0].run,
            read[0].mentions
        ),
        ("names", &extractor.hash, report.run, 2)
    );
    // A task that succeeded leaves nothing behind.
    assert!(news.tasks().is_empty(), "{:?}", news.tasks());

    // Nothing was matched yet. A resolve takes the mentions as new, and the
    // two mentions of Acme are one company.
    assert_eq!(engine.head().unwrap().0, 0);
    let resolved = engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(resolved.resolved[0].new_records, 4);
    assert_eq!(resolved.resolved[0].identities_created, 3);
    let acme = news.identity(first, 2, 10).unwrap();
    assert_eq!(news.identity(second, 19, 27), Some(acme));
    assert_eq!(members(engine, acme).len(), 2);
    assert!(
        news.identity(first, 29, 39)
            .is_some_and(|globex| globex != acme)
    );

    // What was read is not read again, and reading it again on purpose
    // finds what is already known.
    let nothing = news.extract("names").unwrap();
    assert_eq!((nothing.documents, nothing.mentions), (0, 0));
    let again = engine
        .extract(&ExtractOptions {
            again: true,
            source: Some("news".to_owned()),
            limit: Some(1),
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!(
        (
            again.documents,
            again.mentions,
            again.inserted,
            again.duplicates
        ),
        (1, 2, 0, 2)
    );
    assert_eq!(engine.document(first).unwrap().extractions.len(), 2);
    assert_eq!(engine.document(second).unwrap().extractions.len(), 1);
    assert!(
        !engine
            .resolve(&ResolveOptions::default())
            .unwrap()
            .resolved
            .iter()
            .any(|resolved| resolved.new_records > 0)
    );

    // An extractor that says more about a mention it found before makes
    // the next version of that record, which stays the same company.
    news.extractor(
        "names.py",
        &NAMES.replace(
            "name=found.group())",
            "name=found.group(), form=found.group().split()[-1])",
        ),
    );
    let more = engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!(
        (
            more.documents,
            more.mentions,
            more.inserted,
            more.superseded
        ),
        (2, 4, 4, 4)
    );
    let versions = engine
        .records(&RecordFilter::keyed(
            "news",
            locator(first, "company", 2, 10),
        ))
        .unwrap();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[1].supersedes, Some(versions[0].id));
    assert_eq!(
        versions[1].payload,
        serde_json::json!({ "name": "Acme Inc", "form": "Inc" })
    );
    engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(engine.identity(acme).unwrap().members.len(), 4);
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn an_extraction_and_the_load_of_what_it_found_are_started_by_whoever_is_named() {
    let news = News::new();
    news.extractor("names.py", NAMES);
    news.add(&[("a", FIRST)]);
    let desk = Actor::Agent("desk".to_owned());
    let report = news
        .engine
        .acting(desk.clone())
        .extract(&ExtractOptions::with("names"))
        .unwrap();
    let stored = report.load_run.expect("it found something");
    for run in [report.run, stored] {
        assert_eq!(news.engine.run(run).unwrap().started_by, desk);
    }
}

#[test]
#[ignore = "runs uv"]
fn only_the_latest_version_of_a_document_is_read() {
    let news = News::new();
    news.extractor("names.py", NAMES);
    let engine = &news.engine;
    news.add(&[("a", FIRST)]);
    assert_eq!(news.extract("names").unwrap().mentions, 2);

    // The document changes. Its new version is read, and what the old one
    // mentioned stays, under the old version's ID.
    let edited = "Initech Ltd opened an office. Acme Inc did not.";
    news.add(&[("a", edited)]);
    let report = news.extract("names").unwrap();
    assert_eq!(
        (report.documents, report.mentions, report.inserted),
        (1, 2, 2)
    );
    assert_eq!(report.superseded, 0);
    assert_eq!(news.mentions(id("a", FIRST)).len(), 2);
    assert_eq!(news.mentions(id("a", edited)).len(), 2);
    assert!(engine.document(id("a", edited)).unwrap().extractions.len() == 1);

    // Reading everything again reads the version that is current only.
    let again = engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!((again.documents, again.duplicates), (1, 2));
    assert_eq!(
        engine.document(id("a", FIRST)).unwrap().extractions.len(),
        1
    );

    engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(
        news.identity(id("a", FIRST), 2, 10),
        news.identity(id("a", edited), 30, 38)
    );
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn a_document_that_changes_back_is_read_as_the_version_it_is_now() {
    let news = News::new();
    news.extractor("names.py", NAMES);
    let engine = &news.engine;
    news.add(&[("a", FIRST)]);
    news.extract("names").unwrap();
    let edited = "Initech Ltd opened an office. Acme Inc did not.";
    news.add(&[("a", edited)]);
    news.extract("names").unwrap();
    engine.resolve(&ResolveOptions::default()).unwrap();

    // The first text again is the document's latest version, which the
    // extractor has not read. What it finds there is found in that
    // version, and is the company the first version mentioned.
    news.add(&[("a", FIRST)]);
    let third = id("a", FIRST).held_again(id("a", edited));
    let report = news.extract("names").unwrap();
    assert_eq!(
        (report.documents, report.mentions, report.inserted),
        (1, 2, 2)
    );
    assert_eq!(news.mentions(third).len(), 2);
    assert_eq!(news.mentions(id("a", FIRST)).len(), 2);
    assert_eq!(news.extract("names").unwrap().documents, 0);

    engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(
        news.identity(third, 2, 10),
        news.identity(id("a", FIRST), 2, 10)
    );
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn an_extractor_that_fails_fails_its_run_and_stores_nothing() {
    let news = News::new();
    news.extractor(
        "names.py",
        r#"
import sys
from spool import extractor

@extractor
def find(documents):
    print("about to fail", file=sys.stderr)
    raise ValueError(f"cannot read {len(documents)} documents")
"#,
    );
    let engine = &news.engine;
    news.add(&[("a", FIRST), ("b", SECOND)]);

    let message = extractor_error(news.extract("names"));
    assert!(
        message.contains("it failed with exit status: 1"),
        "{message}"
    );
    assert!(message.contains("about to fail"), "{message}");
    assert!(
        message.contains("ValueError: cannot read 2 documents"),
        "{message}"
    );

    let [run] = &runs(engine, RunKind::Extract)[..] else {
        panic!("one extract run");
    };
    assert_eq!(run.status, RunStatus::Failed);
    assert!(run.error.as_deref().unwrap().contains("ValueError"));
    assert_eq!(run.extractor.as_ref().unwrap().name, "names");
    assert!(engine.records(&RecordFilter::default()).unwrap().is_empty());
    assert!(
        engine
            .document(id("a", FIRST))
            .unwrap()
            .extractions
            .is_empty()
    );

    // The task is kept, with what the extractor was given and what it said.
    let [task] = &news.tasks()[..] else {
        panic!("one task was kept: {:?}", news.tasks());
    };
    assert!(message.contains(&task.display().to_string()), "{message}");
    for file in ["task.json", "documents.parquet", "stderr.log"] {
        assert!(task.join(file).is_file(), "{file}");
    }
    assert!(!task.join("mentions.parquet").exists());
    let described: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(task.join("task.json")).unwrap()).unwrap();
    assert_eq!(described["protocol"], 1);
    assert_eq!(described["kind"], "extract");
    assert_eq!(described["extractor"]["name"], "names");
    assert_eq!(described["run"], run.run.0);
    assert_eq!(described["entity_types"][0]["name"], "company");
    assert_eq!(
        described["entity_types"][0]["instructions"],
        serde_json::Value::Null
    );
    assert_eq!(described["entity_types"][0]["features"][1]["name"], "name");

    // Once the extractor is put right, the same documents are read.
    news.extractor("names.py", NAMES);
    assert_eq!(news.extract("names").unwrap().documents, 2);
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn a_mention_the_engine_refuses_refuses_them_all() {
    let news = News::new();
    let engine = &news.engine;
    news.add(&[("a", FIRST)]);
    let extractor = |body: &str| {
        news.extractor(
            "names.py",
            &format!(
                "from spool import extractor\n\n@extractor\ndef find(document, task):\n{body}\n"
            ),
        );
    };
    let refused = |expected: &str| {
        let message = extractor_error(news.extract("names"));
        assert!(message.contains("its mentions were refused"), "{message}");
        assert!(message.contains(expected), "{message}");
        assert!(message.contains("Its task is kept in"), "{message}");
        assert!(engine.records(&RecordFilter::default()).unwrap().is_empty());
        assert!(
            engine
                .document(id("a", FIRST))
                .unwrap()
                .extractions
                .is_empty()
        );
        assert!(
            runs(engine, RunKind::Load)
                .iter()
                .all(|run| run.extractor.is_none())
        );
    };

    // One good mention and one that points past the end of the document.
    extractor(
        "    return [document.mention('company', 2, 10, name='Acme Inc'),\n            \
         document.mention('company', 40, 400, name='Globex Ltd')]",
    );
    refused("row 2: 40-400 is not a span of document `a`");

    extractor("    return [document.mention('person', 2, 10, name='Acme Inc')]");
    refused("row 1: the project declares no entity type `person`");

    extractor("    return [document.mention('company', 2, 10, names=['Acme Inc'])]");
    refused("row 1: attribute `names` is nested");

    extractor(
        "    return [{'document_id': 'elsewhere', 'entity_type': 'company', 'start': 0, 'end': 1}]",
    );
    refused("row 1: `elsewhere` is not a document the extractor was given");

    // An extractor that writes its mentions without the helper can write
    // positions that are not whole numbers. They are not rounded to a place
    // the extractor did not name: 1.6 to 10.4 is not the 2 to 10 of "Acme Inc".
    for (column, expected) in [
        (
            "pyarrow.array([1.6], pyarrow.float64())",
            "`start` is a DOUBLE column, and a position is a whole number",
        ),
        (
            "pyarrow.array(['2'], pyarrow.string())",
            "`start` is a VARCHAR column, and a position is a whole number",
        ),
    ] {
        news.extractor("names.py", &BY_HAND.replace("START", column));
        refused(expected);
    }

    // The helper stops what it can tell is no mention before it is written.
    extractor("    return [{'entity_type': 'company', 'start': 0, 'where': 1}]");
    let message = extractor_error(news.extract("names"));
    assert!(
        message.contains("mention 0 has no `end` and also `where`"),
        "{message}"
    );

    // An extractor that finds nothing has read the document all the same.
    extractor("    assert task.entity_types == ['company']\n    return None");
    let report = news.extract("names").unwrap();
    assert_eq!(
        (report.documents, report.mentions, report.load_run),
        (1, 0, None)
    );
    assert_eq!(
        engine.document(id("a", FIRST)).unwrap().extractions[0].mentions,
        0
    );
    assert_eq!(news.extract("names").unwrap().documents, 0);
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn an_extractor_is_told_what_each_type_says_to_look_for() {
    let news = News::new();
    news.write(
        "entities/company/extract.md",
        "A company is named with its legal form.\n",
    );
    // The mention's name is the instruction it was given, to show it arrived.
    news.extractor(
        "names.py",
        r#"
from spool import extractor

@extractor
def find(documents, task):
    assert task.extractor == "names"
    assert [column["name"] for column in task.features("company")] == ["record_id", "name"]
    assert [document.key for document in documents] == ["a", "b"]
    assert documents[0].uri == "https://news.test/a" and documents[0].source == "news"
    for document in documents:
        for start, end in document.find("Acme Inc"):
            yield document.mention("company", start, end, name=task.instructions("company"))
"#,
    );
    news.add(&[("a", FIRST), ("b", SECOND)]);
    let report = news.extract("names").unwrap();
    assert_eq!((report.documents, report.mentions), (2, 2));
    let found = news.mentions(id("b", SECOND));
    assert_eq!(
        found[0].payload,
        serde_json::json!({ "name": "A company is named with its legal form." })
    );
    assert_eq!(found[0].evidence.as_ref().unwrap()["text"], "Acme Inc");
}

/// A model reads only so much at once, so an extractor that asks one reads
/// a long document in pieces that share some words, and what is found in
/// the words two pieces share is a mention once.
#[test]
#[ignore = "runs uv"]
fn a_long_document_is_read_in_pieces_and_what_two_pieces_share_is_found_once() {
    let news = News::new();
    news.extractor(
        "names.py",
        r#"
import re
from spool import extractor

WORD = re.compile(r"\w+(?:[-_]\w+)*|\S")

@extractor
def find(document):
    text = document.text
    if document.key == "short":
        # A text that fits is one piece, and it is the text.
        whole = list(document.pieces(300))
        assert [(piece.start, piece.text) for piece in whole] == [(0, text)]
        assert whole[0].document is document
        # One with no words in it is no piece, and a piece has a length.
        assert list(type(document)(document.id, "news", "none", None, " \n").pieces(5)) == []
        for words, shared in [(0, None), (True, None), (5, 5), (5, -1), ("5", None)]:
            try:
                document.pieces(words, shared)
            except ValueError as error:
                assert "`words` is" in str(error) or "`shared` is" in str(error), error
            else:
                raise AssertionError(f"pieces({words!r}, {shared!r}) was taken")
        # Without shared words, each word is in one piece.
        apart = list(document.pieces(3, 0))
        assert [piece.text for piece in apart] == ["🚀 Acme Inc", "opened an office", "."]
        assert [piece.start for piece in apart] == [0, 11, 27]

    # Pieces of twelve words, of which each shares four with the next.
    pieces = list(document.pieces(12, 4))
    words = [found.span() for found in WORD.finditer(text)]
    assert len(pieces) == max(1, -(-(len(words) - 4) // 8)), (len(pieces), len(words))
    assert pieces[0].start == 0
    assert pieces[-1].start + len(pieces[-1].text) == len(text)
    for index, piece in enumerate(pieces):
        # A piece is a part of the text, of no more words than was asked,
        # and it begins and ends with a word.
        assert text[piece.start : piece.start + len(piece.text)] == piece.text
        assert len(WORD.findall(piece.text)) <= 12
        if index:
            assert piece.start == words[index * 8][0]
        if index + 1 < len(pieces):
            assert piece.start + len(piece.text) == words[index * 8 + 11][1]
        for found in re.finditer(r"[A-Z]\w+ (?:Inc|Ltd)", piece.text):
            # Positions are the piece's, and the mention's are the document's.
            mention = piece.mention("company", found.start(), found.end(), name=found.group())
            if mention is not None:
                assert text[mention["start"] : mention["end"]] == found.group()
            # One that the next piece has too is `None` here or there, and
            # the helper leaves it out.
            yield mention
"#,
    );
    // Names every five words, so that they fall at every place of a piece:
    // inside it, in the words it shares, and across where a piece ends.
    let long: String = (0..40)
        .map(|at| match at % 2 {
            0 => format!("Société{at} Ltd said: «née» 🚀. "),
            _ => format!("Acme{at} Inc was not there, "),
        })
        .collect();
    let short = "🚀 Acme Inc opened an office.";
    news.add(&[("long", &long), ("short", short)]);
    let report = news.extract("names").unwrap();
    assert_eq!((report.documents, report.mentions), (2, 41));

    // Each name is a record once, at its place in the document.
    let characters: Vec<char> = long.chars().collect();
    let found = news.mentions(id("long", &long));
    let names: Vec<String> = found
        .iter()
        .map(|record| record.payload["name"].as_str().unwrap().to_owned())
        .collect();
    let expected: Vec<String> = (0..40)
        .map(|at| match at % 2 {
            0 => format!("Société{at} Ltd"),
            _ => format!("Acme{at} Inc"),
        })
        .collect();
    assert_eq!(names, expected);
    for record in &found {
        let evidence = record.evidence.as_ref().unwrap();
        let (start, end) = (
            evidence["start"].as_u64().unwrap() as usize,
            evidence["end"].as_u64().unwrap() as usize,
        );
        assert_eq!(
            characters[start..end].iter().collect::<String>(),
            record.payload["name"].as_str().unwrap()
        );
        assert_eq!(evidence["text"], record.payload["name"]);
    }
    assert_eq!(news.mentions(id("short", short)).len(), 1);
}

/// Anyone who may add a document decides how long it is and what is in it.
/// A text of nothing but words is read in pieces without a place being kept
/// for each of its words, which would be many times the text.
#[test]
#[ignore = "runs uv"]
fn a_text_of_millions_of_words_is_read_in_pieces_in_little_memory() {
    let news = News::new();
    news.extractor(
        "names.py",
        r#"
import resource
import sys
from spool import extractor

def peak():
    """The most memory this process has had, in MiB."""
    most = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return most / (1024 * 1024 if sys.platform == "darwin" else 1024)

@extractor
def find(document):
    words = len(document.text) // 2
    before = peak()
    pieces = document.pieces(300)
    # They are made one at a time, as they are asked for.
    assert iter(pieces) is pieces, type(pieces)
    count, last = 0, None
    for piece in pieces:
        count, last = count + 1, piece
        assert piece.text.count("a") <= 300, (count, len(piece.text))
    grown = peak() - before
    assert count == -(-(words - 37) // 263), (count, words)
    assert last.start + len(last.text) == len(document.text)
    # A place for each word would be some 70 bytes for each two of the text.
    assert grown < 64, f"{grown:.0f} MiB more for a text of {len(document.text) >> 20} MiB"
    yield last.mention("company", len(last.text) - 2, len(last.text) - 1)
"#,
    );
    // Eight MiB in which every other character is a word.
    let dense = "a ".repeat(4 << 20);
    news.add(&[("dense", &dense)]);
    let report = news.extract("names").unwrap();
    assert_eq!((report.documents, report.mentions), (1, 1));
}

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

#[test]
#[ignore = "runs uv"]
fn a_type_the_project_no_longer_declares_is_left_alone_by_a_reading() {
    let news = News::new();
    news.write(
        "entities/person/entity.yaml",
        "features: [name]\nmapping:\n  news:\n    name: name\n",
    );
    news.extractor(
        "names.py",
        r#"
from spool import extractor

@extractor
def find(document):
    for start, end in document.find("Acme Inc"):
        yield document.mention("company", start, end, name="Acme Inc")
    for start, end in document.find("Globex Ltd"):
        yield document.mention("person", start, end, name="Globex Ltd")
"#,
    );
    news.add(&[("a", FIRST)]);
    assert_eq!(news.extract("names").unwrap().mentions, 2);

    // The project drops `person`, and its extractor finds companies alone.
    fs::remove_dir_all(news.project().join("entities/person")).unwrap();
    news.extractor("names.py", NAMES);
    let again = news
        .engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!(
        (again.taken_back, again.retract_runs.len()),
        (0, 0),
        "{again:?}"
    );
    let person = news
        .engine
        .records(&RecordFilter::keyed(
            "news",
            locator(id("a", FIRST), "person", 29, 39),
        ))
        .unwrap();
    assert_eq!(person[0].retracted, None);
}

#[test]
#[ignore = "runs uv"]
fn a_reading_is_recorded_against_the_version_it_found() {
    let news = News::new();
    let saying = |form: &str| {
        format!(
            r#"
from spool import extractor

@extractor
def find(document):
    for start, end in document.find("Acme Inc"):
        yield document.mention("company", start, end, name="Acme Inc", form="{form}")
"#
        )
    };
    news.add(&[("a", FIRST)]);
    let document = id("a", FIRST);
    let mut runs = Vec::new();
    // The mention says Inc, then Ltd, then Inc again: three versions, the
    // last of which holds what the first held.
    for form in ["Inc", "Ltd", "Inc"] {
        news.extractor("names.py", &saying(form));
        let report = news
            .engine
            .extract(&ExtractOptions {
                again: true,
                ..ExtractOptions::with("names")
            })
            .unwrap();
        runs.push(report.run);
    }
    let versions = news
        .engine
        .records(&RecordFilter::keyed(
            "news",
            locator(document, "company", 2, 10),
        ))
        .unwrap();
    assert_eq!(versions.len(), 3);
    let current = versions
        .iter()
        .find(|record| record.superseded_by.is_none())
        .unwrap();
    let found: Vec<_> = current.found_by.iter().map(|found| found.run).collect();
    assert_eq!(found, [runs[2]], "{versions:?}");
    // Each earlier version keeps the reading that found it.
    let earlier: BTreeSet<_> = versions
        .iter()
        .filter(|record| record.superseded_by.is_some())
        .flat_map(|record| record.found_by.iter().map(|found| found.run))
        .collect();
    assert_eq!(earlier, BTreeSet::from([runs[0], runs[1]]));
}

#[test]
#[ignore = "runs uv"]
fn mentions_no_extractor_finds_any_more_are_taken_back_and_come_back() {
    let news = News::new();
    let finding = |names: &[&str]| {
        format!(
            r#"
from spool import extractor

@extractor
def find(document):
    for name in {names:?}:
        for start, end in document.find(name):
            yield document.mention("company", start, end, name=name)
"#
        )
    };
    news.extractor("names.py", &finding(&["Acme Inc", "Globex Ltd"]));
    news.extractor("others.py", &finding(&["Acme Inc"]));
    news.add(&[("a", FIRST)]);
    news.extract("names").unwrap();
    news.extract("others").unwrap();
    news.engine.resolve(&ResolveOptions::default()).unwrap();
    let document = id("a", FIRST);
    let acme = news.identity(document, 2, 10).unwrap();
    let globex = news.identity(document, 29, 39).unwrap();
    assert_ne!(acme, globex);

    // The changed extractor finds Acme alone. Globex, which no extractor
    // finds now, is taken back; its identity keeps its ID, with no records.
    news.extractor("names.py", &finding(&["Acme Inc"]));
    let again = news
        .engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!(again.taken_back, 1);
    let [retract_run] = again.retract_runs[..] else {
        panic!("one retract run: {again:?}");
    };
    let taken = news
        .engine
        .records(&RecordFilter::keyed(
            "news",
            locator(document, "company", 29, 39),
        ))
        .unwrap();
    assert_eq!(taken[0].retracted, Some(retract_run));
    assert_eq!(taken[0].identity, None);
    assert!(members(&news.engine, globex).is_empty());
    let view = news.engine.identity(globex).unwrap();
    assert_eq!((view.current, view.members.len()), (globex, 0));
    assert_eq!(
        news.engine.run(retract_run).unwrap().started_by,
        Actor::Engine
    );
    // Acme stays, found by both versions of `names` and by `others`.
    let found = &news
        .engine
        .records(&RecordFilter::keyed(
            "news",
            locator(document, "company", 2, 10),
        ))
        .unwrap()[0];
    let versions: BTreeSet<(&str, &str)> = found
        .found_by
        .iter()
        .map(|found| (found.extractor.as_str(), found.version.as_str()))
        .collect();
    assert_eq!(versions.len(), 3, "{:?}", found.found_by);
    assert_eq!(found.identity, Some(acme));

    // `others`, read again, still finds Acme, and nothing is taken back.
    let others = news
        .engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("others")
        })
        .unwrap();
    assert_eq!((others.taken_back, others.retract_runs.len()), (0, 0));

    // Found again, Globex comes back to the identity it left.
    news.extractor("names.py", &finding(&["Acme Inc", "Globex Ltd"]));
    news.engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!(news.identity(document, 29, 39), Some(globex));
    news.engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn opening_a_format_11_store_preserves_mentions_and_reading_counts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.duckdb");
    let open = || Engine::open(&path, Options::seeded(1)).unwrap();
    let mut news = News::with_engine(dir, open());
    news.extractor("names.py", NAMES);
    news.extractor("others.py", NAMES);
    news.add(&[("a", FIRST)]);
    let names = news.extract("names").unwrap();
    let others = news.extract("others").unwrap();
    assert_eq!((names.inserted, others.duplicates), (2, 2));
    news.engine.resolve(&ResolveOptions::default()).unwrap();
    let document = id("a", FIRST);
    let mut expected = news.mentions(document);
    for mention in &mut expected {
        mention.found_by.clear();
    }
    let log = news.engine.log().unwrap();
    drop(news.engine);

    // Format 11 stored the same readings and records, with no report of
    // per-document failures and no links from a reading to the mentions.
    duckdb::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "ALTER TABLE spool.extraction DROP COLUMN failure;
             DROP TABLE spool.extraction_found;
             UPDATE spool.meta SET value = '11' WHERE key = 'format_version';",
        )
        .unwrap();

    for _ in 0..2 {
        news.engine = Arc::new(open());
        assert_eq!(news.engine.info().unwrap().format_version, "12");
        assert_eq!(news.engine.log().unwrap(), log);
        let readings = news.engine.document(document).unwrap().extractions;
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].run, names.run);
        assert_eq!(readings[1].run, others.run);
        assert!(
            readings
                .iter()
                .all(|read| read.mentions == 2 && read.failure.is_none())
        );

        assert_eq!(news.mentions(document), expected);
        drop(news.engine);
    }

    news.engine = Arc::new(open());
    news.engine.set_user_code(UserCode::new(news.project()));
    let reread = || {
        news.engine
            .extract(&ExtractOptions {
                again: true,
                ..ExtractOptions::with("names")
            })
            .unwrap()
    };

    // Count-only readings remain in the history. Per-mention tracking
    // starts with readings made after the upgrade.
    news.extractor(
        "names.py",
        "from spool import extractor\n\n@extractor\ndef find(document):\n    return []\n",
    );
    assert_eq!(reread().taken_back, 0);
    assert_eq!(news.mentions(document), expected);

    news.extractor("names.py", NAMES);
    let reading = reread();
    assert_eq!((reading.inserted, reading.duplicates), (0, 2));
    for mention in news.mentions(document) {
        assert_eq!(mention.found_by.len(), 1);
        assert_eq!(mention.found_by[0].run, reading.run);
    }
    news.engine.verify().unwrap();
}

#[test]
#[ignore = "runs bun"]
fn a_typescript_extractor_finds_what_a_python_one_finds() {
    let python = News::new();
    python.extractor("names.py", NAMES);
    let typescript = News::new();
    typescript.extractor(
        "names.ts",
        r#"
import { extractor } from "spool-strategy";

await extractor(async (task) => {
  const documents = await task.documents();
  if (task.entityTypes.join() !== "company") throw new Error("the types are not there");
  return documents.flatMap((document) =>
    [...document.text.matchAll(/[A-Z]\p{L}+ (?:Inc|Ltd)/gu)].map((found) =>
      document.mention("company", found.index, found.index + found[0].length, { name: found[0] }),
    ),
  );
});
"#,
    );

    let mut found = Vec::new();
    for news in [&python, &typescript] {
        news.add(&[("a", FIRST), ("b", SECOND)]);
        let report = news.extract("names").unwrap();
        assert_eq!(
            (report.documents, report.mentions, report.inserted),
            (2, 4, 4)
        );
        // The rocket is two of JavaScript's characters and one of the
        // engine's: the helper counts as the engine does.
        assert_eq!(
            news.mentions(id("a", FIRST))[0].evidence.as_ref().unwrap()["text"],
            "Acme Inc"
        );
        let ids: BTreeSet<_> = news
            .engine
            .records(&RecordFilter::default())
            .unwrap()
            .iter()
            .map(|record| record.id)
            .collect();
        found.push(ids);
        assert!(news.tasks().is_empty(), "{:?}", news.tasks());
    }
    // The same mentions with the same attributes are the same records,
    // whichever language found them.
    assert_eq!(found[0], found[1]);

    // An extractor that throws fails its run.
    typescript.extractor(
        "names.ts",
        "import { extractor } from \"spool-strategy\";\n\
         await extractor(() => { throw new Error(\"cannot read\"); });\n",
    );
    typescript.add(&[("c", "Initech Ltd.")]);
    let message = extractor_error(typescript.extract("names"));
    assert!(message.contains("cannot read"), "{message}");
}
