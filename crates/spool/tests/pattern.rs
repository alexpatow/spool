//! Pattern extractors: a file under `extractors/` that holds a regular
//! expression, which the engine runs itself. Nothing here starts a process.

mod common;

use std::fs;
use std::path::Path;

use serde_json::json;
use spool::core::DocumentId;
use spool::{
    Engine, Error, ExtractOptions, Manifest, NewDocument, Options, Project, RecordEntry,
    RecordFilter, ResolveOptions, Rows, RunKind, RunStatus, Runtime,
};

fn write(root: &Path, file: &str, contents: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Finds a run of capitalised words that ends in a legal form. The whole
/// match is the `name`, and the legal form is the `form`.
const COMPANIES: &str = "\
# Company names, by their legal form.
entity_type: company
attribute: name
pattern: '(?:\\p{Lu}\\p{L}+ )+(?P<form>Inc|Ltd)\\b'
";

/// A project with a source of rows, `crm`, and a source of documents,
/// `notes`, whose records are companies compared by their name without its
/// legal form.
fn companies() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "sources/crm.yaml", "key: id\ncolumns: [id, name]\n");
    write(root, "sources/notes.yaml", "columns: [name, form]\n");
    write(
        root,
        "transforms/plain.sql",
        "lambda text: lower(regexp_replace(text, ' (Inc|Ltd)$', ''))\n",
    );
    write(
        root,
        "entities/company/entity.yaml",
        "label: name\nfeatures: [name, plain]\nmapping:\n  \
         crm:\n    name: name\n    plain: { from: name, transform: plain }\n  \
         notes:\n    name: name\n    plain: { from: name, transform: plain }\n",
    );
    for kind in ["block", "match"] {
        write(
            root,
            &format!("entities/company/rules/{kind}/same_name.sql"),
            "l.plain = r.plain\n",
        );
    }
    write(root, "extractors/companies.yaml", COMPANIES);
    dir
}

/// An engine with the project applied the way a deployed server has it:
/// from a manifest that was written and read, with no project directory and
/// nothing to run user code with.
fn engine_of(root: &Path) -> Engine {
    let file = root.join(".spool/manifest.json");
    Manifest::load(root).unwrap().write_file(&file).unwrap();
    let manifest = Manifest::read_file(&file).unwrap();
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&manifest).unwrap();
    engine
}

fn add(engine: &Engine, source: &str, documents: &[(&str, &str)]) -> spool::IngestReport {
    let documents: Vec<NewDocument> = documents
        .iter()
        .map(|(key, text)| NewDocument {
            key: (*key).to_owned(),
            uri: None,
            text: (*text).to_owned(),
        })
        .collect();
    engine.add_documents(source, &documents).unwrap()
}

/// The records found in a document, in the order of the text.
fn mentions(engine: &Engine, document: DocumentId) -> Vec<RecordEntry> {
    let mut found = engine
        .records(&RecordFilter {
            document: Some(document),
            ..RecordFilter::default()
        })
        .unwrap();
    found.sort_by_key(|record| record.evidence.as_ref().unwrap()["start"].as_u64());
    found
}

/// Where each record of a document was found and what it says.
fn places(engine: &Engine, document: DocumentId) -> Vec<(u64, u64, String, serde_json::Value)> {
    mentions(engine, document)
        .into_iter()
        .map(|record| {
            let evidence = record.evidence.unwrap();
            (
                evidence["start"].as_u64().unwrap(),
                evidence["end"].as_u64().unwrap(),
                evidence["text"].as_str().unwrap().to_owned(),
                record.payload,
            )
        })
        .collect()
}

fn project_error(root: &Path) -> (String, String) {
    match Manifest::load(root) {
        Err(Error::Project { file, message }) => (
            file.strip_prefix(root)
                .unwrap_or(&file)
                .to_string_lossy()
                .into_owned(),
            message,
        ),
        other => panic!("expected a project error, got {other:?}"),
    }
}

const CALL: &str = "🚀 Société Générale Ltd rang, then Acme Inc and acme inc, and Ltd alone.";

#[test]
fn a_pattern_is_an_extractor_of_the_project() {
    let dir = companies();
    let root = dir.path();
    // A file beside it that is no extractor is listed and left alone, and
    // so is a directory of its name: a pattern is its file.
    write(root, "extractors/readme.md", "");
    write(root, "extractors/companies/notes.txt", "");
    let project = Project::read(root).unwrap();
    let mut ignored: Vec<String> = project
        .ignored
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    ignored.sort();
    assert_eq!(ignored, ["extractors/companies", "extractors/readme.md"]);

    let manifest = project.compile().unwrap();
    let extractor = manifest.extractor("companies").unwrap();
    assert_eq!(extractor.runtime, Runtime::Pattern);
    assert_eq!(extractor.runtime.as_str(), "pattern");
    assert_eq!(extractor.file, "extractors/companies.yaml");
    // The manifest holds what the pattern looks for, so a server that has
    // the manifest and not the project runs it.
    assert_eq!(
        serde_json::to_value(extractor).unwrap()["pattern"],
        json!({
            "entity_type": "company",
            "attribute": "name",
            "pattern": "(?:\\p{Lu}\\p{L}+ )+(?P<form>Inc|Ltd)\\b",
        })
    );

    // Its version is its file: the manifest and the extractor change with it.
    write(
        root,
        "extractors/companies.yaml",
        &COMPANIES.replace("Inc|Ltd", "Inc|Ltd|GmbH"),
    );
    let changed = Manifest::load(root).unwrap();
    assert_ne!(changed.hash, manifest.hash);
    assert_ne!(changed.extractor("companies").unwrap().hash, extractor.hash);

    // One name is one extractor, whatever runs it.
    write(root, "extractors/companies.py", "");
    let (file, message) = project_error(root);
    assert_eq!(file, "extractors/companies.yaml");
    assert!(message.contains("defined twice"), "{message}");
}

#[test]
fn a_pattern_that_cannot_find_records_fails_the_build_and_names_its_file() {
    for (yaml, said) in [
        (
            "entity_type: person\nattribute: name\npattern: '[A-Z]+'\n",
            "the project declares no entity type `person`",
        ),
        (
            "entity_type: company\nattribute: name\npattern: '(?:[A-Z]+'\n",
            "the pattern is not a regular expression Spool can run: ",
        ),
        // The expressions run in time that grows with the text and no
        // faster, which rules out looking back at what a group matched.
        (
            "entity_type: company\nattribute: name\npattern: '([A-Z]+) \\1'\n",
            "backreferences are not supported",
        ),
        (
            "entity_type: company\nattribute: name\npattern: '[A-Z]*'\n",
            "the pattern matches text of no length, so it would find something at every \
             character",
        ),
        (
            "entity_type: company\npattern: '[A-Z]+'\n",
            "what the pattern finds would say nothing: name the `attribute` that holds the \
             matched text, or a group in the pattern, as in `(?P<name>...)`",
        ),
        (
            "entity_type: company\nattribute: name\npattern: '(?P<name>[A-Z]+) Ltd'\n",
            "`name` is the attribute of the whole match and the name of a group of the \
             pattern; it can be one of them",
        ),
        (
            "entity_type: company\nattribute: title\npattern: '[A-Z]+'\n",
            "attribute `title` is not a column of any source that `company` is mapped from",
        ),
        (
            "entity_type: company\nattribute: name\npattern: '[A-Z]+ (?P<kind>Ltd)'\n",
            "attribute `kind` is not a column of any source that `company` is mapped from",
        ),
        ("entity_type: company\nattribute: name\n", "pattern"),
        (
            "entity_type: company\nattribute: name\npattern: '[A-Z]+'\nflags: i\n",
            "flags",
        ),
    ] {
        let dir = companies();
        write(dir.path(), "extractors/companies.yaml", yaml);
        let (file, message) = project_error(dir.path());
        assert_eq!(file, "extractors/companies.yaml", "{yaml}");
        assert!(message.contains(said), "{yaml}: {message}");
    }
}

#[test]
fn a_pattern_finds_a_record_wherever_it_matches() {
    let dir = companies();
    let engine = engine_of(dir.path());
    add(
        &engine,
        "notes",
        &[("call.txt", CALL), ("empty.txt", "nothing here")],
    );

    let report = engine.extract(&ExtractOptions::with("companies")).unwrap();
    assert_eq!(
        (
            report.documents,
            report.mentions,
            report.inserted,
            report.duplicates
        ),
        (2, 2, 2, 0)
    );

    // A place is counted in code points, so what stands after an emoji and
    // after letters of two bytes is where a reader would count it. The whole
    // match is the attribute the file names, and a named group is one more.
    let call = DocumentId::derive("notes", "call.txt", CALL);
    assert_eq!(
        places(&engine, call),
        [
            (
                2,
                22,
                "Société Générale Ltd".to_owned(),
                json!({"form": "Ltd", "name": "Société Générale Ltd"})
            ),
            (
                34,
                42,
                "Acme Inc".to_owned(),
                json!({"form": "Inc", "name": "Acme Inc"})
            ),
        ]
    );
    let text: Vec<char> = CALL.chars().collect();
    assert_eq!(
        text[2..22].iter().collect::<String>(),
        "Société Générale Ltd"
    );
    let found = &mentions(&engine, call)[0];
    assert_eq!(found.source, "notes");
    assert_eq!(found.entity_type.as_str(), "company");
    assert_eq!(found.name(), "notes:call.txt[2-22]");
    let evidence = found.evidence.as_ref().unwrap();
    assert_eq!(evidence["extractor"], "companies");

    // The run is an extract run like any other, with the ingest run that
    // stored what it found.
    let run = engine.run(report.run).unwrap();
    assert_eq!(
        (run.kind, run.status),
        (RunKind::Extract, RunStatus::Succeeded)
    );
    assert_eq!(run.extractor.as_ref().unwrap().name, "companies");
    let stored = engine.run(report.ingest_run.unwrap()).unwrap();
    assert_eq!(stored.kind, RunKind::Ingest);
    assert_eq!(stored.ingest.unwrap().inserted, 2);

    // What was read is not read again, unless it is asked for, and then
    // the same places are the same records.
    let nothing = engine.extract(&ExtractOptions::with("companies")).unwrap();
    assert_eq!((nothing.documents, nothing.mentions), (0, 0));
    let again = engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("companies")
        })
        .unwrap();
    assert_eq!(
        (
            again.documents,
            again.mentions,
            again.inserted,
            again.duplicates
        ),
        (2, 2, 0, 2)
    );
}

#[test]
fn what_a_pattern_finds_joins_the_identities_of_rows() {
    let dir = companies();
    let engine = engine_of(dir.path());
    let rows = Rows::from_text(
        &["id", "name"],
        &[
            vec![Some("c-1"), Some("Acme")],
            vec![Some("c-2"), Some("ACME Inc")],
            vec![Some("c-3"), Some("Globex")],
        ],
    )
    .unwrap();
    engine.ingest("crm", "company", rows).unwrap();
    engine.resolve(&ResolveOptions::default()).unwrap();
    let acme = common::identity(&engine, "crm", "c-1").unwrap();
    assert_eq!(common::identity(&engine, "crm", "c-2"), Some(acme));

    let text = "Acme Inc called about Initech Ltd.";
    add(&engine, "notes", &[("call.txt", text)]);
    let report = engine.extract(&ExtractOptions::with("companies")).unwrap();
    assert_eq!(report.inserted, 2);
    // Nothing is matched by an extraction: the records wait for a run.
    let call = DocumentId::derive("notes", "call.txt", text);
    assert!(
        mentions(&engine, call)
            .iter()
            .all(|record| record.identity.is_none())
    );

    let resolved = engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(resolved.resolved[0].records_assigned, 2);
    assert_eq!(resolved.resolved[0].identities_created, 1);
    let found = mentions(&engine, call);
    // The company the rows knew keeps its identity, and the note is one
    // more record of it. The one no row had is new.
    assert_eq!(found[0].identity, Some(acme));
    assert_eq!(common::identity(&engine, "crm", "c-1"), Some(acme));
    let initech = found[1].identity.unwrap();
    assert_ne!(initech, acme);
    assert_eq!(common::members(&engine, initech).len(), 1);
    assert_eq!(common::members(&engine, acme).len(), 3);
}

#[test]
fn a_record_a_pattern_finds_may_say_only_what_its_documents_source_lists() {
    // The pattern compiles, since `form` is a column of `notes`, and what
    // it finds in a document of `crm` is refused: `crm` lists no `form`.
    let dir = companies();
    let engine = engine_of(dir.path());
    add(&engine, "crm", &[("export.txt", "Acme Inc is a customer.")]);
    let refused = engine.extract(&ExtractOptions::with("companies"));
    let Err(Error::Extractor { extractor, message }) = refused else {
        panic!("expected the extractor to fail, got {refused:?}");
    };
    assert_eq!(extractor, "companies");
    assert!(
        message.starts_with("what it found was refused: "),
        "{message}"
    );
    assert!(message.contains("`form`"), "{message}");
    // The run failed, nothing was stored, and the document is still unread.
    let runs = engine
        .runs(&spool::RunFilter {
            kind: Some(RunKind::Extract),
            ..spool::RunFilter::default()
        })
        .unwrap();
    assert_eq!(runs[0].status, RunStatus::Failed);
    assert_eq!(engine.records(&RecordFilter::default()).unwrap(), []);

    // Limited to the source it fits, it reads that source's documents.
    add(&engine, "notes", &[("call.txt", "Acme Inc called.")]);
    let report = engine
        .extract(&ExtractOptions {
            source: Some("notes".to_owned()),
            ..ExtractOptions::with("companies")
        })
        .unwrap();
    assert_eq!((report.documents, report.inserted), (1, 1));
}
