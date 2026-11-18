//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use spool::arrow::array::{Array, AsArray};
use spool::arrow::record_batch::RecordBatch;
use spool::core::{IdentityId, RecordId};
use spool::{
    Engine, EntityTypeSource, LoadReport, Manifest, Options, Project, Rows, SqlFile, YamlFile,
};

/// The repository's `benchmarks` directory.
pub fn benchmarks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks")
}

/// The sources the tests take things from. Each gives rows of
/// `(id, name, code)`, keyed by `id`.
pub const SOURCES: [&str; 5] = ["shop", "depot", "kiosk", "yard", "late"];

pub fn source(name: &str) -> YamlFile {
    YamlFile::new(name, "key: id\ncolumns: [id, name, code]\n")
}

/// An `entity.yaml` that maps each feature the same way from every one of
/// the sources: `("name", "name")` is the column as it is, and
/// `("name", "{ from: name, transform: upper }")` is a transform of it.
pub fn entity(label: Option<&str>, features: &[(&str, &str)], sources: &[&str]) -> YamlFile {
    let names: Vec<&str> = features.iter().map(|(name, _)| *name).collect();
    let mut yaml = String::new();
    if let Some(label) = label {
        yaml += &format!("label: {label}\n");
    }
    yaml += &format!("features: [{}]\nmapping:\n", names.join(", "));
    for source in sources {
        yaml += &format!("  {source}:\n");
        for (name, from) in features {
            yaml += &format!("    {name}: {from}\n");
        }
    }
    YamlFile::new("entity", yaml)
}

/// An entity type with no meaning: a `thing` has a name and a code. Things
/// with the same name or code match, unless their codes differ.
pub fn thing() -> EntityTypeSource {
    EntityTypeSource {
        name: "thing".to_owned(),
        entity: entity(
            Some("name"),
            &[("name", "name"), ("code", "code")],
            &SOURCES,
        ),
        block: vec![
            SqlFile::new("same_name", "l.name = r.name"),
            SqlFile::new("same_code", "l.code = r.code"),
        ],
        no_match: vec![SqlFile::new("different_code", "l.code <> r.code")],
        match_: vec![
            SqlFile::new("same_name", "l.name = r.name"),
            SqlFile::new("same_code", "l.code = r.code"),
        ],
        strategies: Vec::new(),
        extract: None,
    }
}

/// A project of the given types and the tests' sources.
pub fn project_of(entity_types: Vec<EntityTypeSource>) -> Project {
    Project {
        sources: SOURCES.iter().map(|name| source(name)).collect(),
        transforms: Vec::new(),
        entity_types,
        extractors: Vec::new(),
        ignored: Vec::new(),
    }
}

pub fn manifest_of(entity_types: Vec<EntityTypeSource>) -> Manifest {
    project_of(entity_types)
        .compile()
        .expect("the test project compiles")
}

pub fn things() -> Manifest {
    manifest_of(vec![thing()])
}

/// An in-memory engine with the `thing` type declared and seeded IDs.
pub fn engine() -> Engine {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&things()).unwrap();
    engine
}

/// Rows of `(id, name, code)`.
pub fn rows(rows: &[(&str, Option<&str>, Option<&str>)]) -> Rows {
    let rows: Vec<Vec<Option<&str>>> = rows
        .iter()
        .map(|(id, name, code)| vec![Some(*id), *name, *code])
        .collect();
    Rows::from_text(&["id", "name", "code"], &rows).unwrap()
}

/// Loads `(id, name, code)` rows as things, keyed by `id`.
pub fn load(
    engine: &Engine,
    source: &str,
    things: &[(&str, Option<&str>, Option<&str>)],
) -> LoadReport {
    engine.load(source, "thing", rows(things)).unwrap()
}

pub fn cell(batch: &RecordBatch, column: usize, row: usize) -> Option<String> {
    let values = batch.column(column).as_string::<i32>();
    values.is_valid(row).then(|| values.value(row).to_owned())
}

/// The rows of a query whose columns are all text.
pub fn select(engine: &Engine, sql: &str) -> Vec<Vec<Option<String>>> {
    let mut rows = Vec::new();
    for batch in engine.query(sql).unwrap() {
        for row in 0..batch.num_rows() {
            rows.push(
                (0..batch.num_columns())
                    .map(|column| cell(&batch, column, row))
                    .collect(),
            );
        }
    }
    rows
}

pub fn count(engine: &Engine, sql: &str) -> u64 {
    select(engine, &format!("SELECT ({sql})::VARCHAR"))[0][0]
        .as_deref()
        .unwrap()
        .parse()
        .unwrap()
}

/// The records a source holds under a key. There is more than one when
/// the row changed between loads.
pub fn records(engine: &Engine, source: &str, key: &str) -> Vec<RecordId> {
    select(
        engine,
        &format!(
            "SELECT record_id::VARCHAR FROM spool.record
             WHERE source_id = '{source}' AND source_key = '{key}' ORDER BY load_run"
        ),
    )
    .into_iter()
    .map(|row| row[0].as_deref().unwrap().parse().unwrap())
    .collect()
}

/// The one record a source holds under a key.
pub fn record_id(engine: &Engine, source: &str, key: &str) -> RecordId {
    let found = records(engine, source, key);
    assert_eq!(found.len(), 1, "records of {source}:{key}");
    found[0]
}

/// The identity of the one record a source holds under a key.
pub fn identity(engine: &Engine, source: &str, key: &str) -> Option<IdentityId> {
    engine.identity_of(record_id(engine, source, key)).unwrap()
}

/// The members of an identity as `source:key`.
pub fn members(engine: &Engine, identity: IdentityId) -> BTreeSet<String> {
    select(
        engine,
        &format!(
            "SELECT o.source_id || ':' || o.source_key
             FROM spool.membership_current m
             JOIN spool.record o ON o.record_id = m.record_id
             WHERE m.identity_id = '{identity}'"
        ),
    )
    .into_iter()
    .map(|row| row[0].clone().unwrap())
    .collect()
}
