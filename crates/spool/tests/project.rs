//! Project discovery and compilation.

mod common;

use std::fs;
use std::path::Path;

use common::benchmarks;
use spool::{Error, Manifest, Project};

const FEATURES: &str =
    "SELECT observation_id, lower(payload->>'title') AS title FROM observation;\n";

fn write(root: &Path, file: &str, contents: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A project with one entity type, `paper`.
fn papers() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "entities/paper/features.sql", FEATURES);
    write(
        root,
        "entities/paper/rules/block/same_title.sql",
        "l.title = r.title\n",
    );
    write(
        root,
        "entities/paper/rules/match/same_title.sql",
        "l.title = r.title;\n",
    );
    write(
        root,
        "entities/paper/rules/match/a_prefix.sql",
        "l.title[:9] = r.title[:9]",
    );
    dir
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

#[test]
fn the_path_is_the_name() {
    let dir = papers();
    let manifest = Manifest::load(dir.path()).unwrap();

    let [paper] = &manifest.entity_types[..] else {
        panic!("one entity type");
    };
    assert_eq!(paper.name.as_str(), "paper");
    let columns: Vec<(&str, &str)> = paper
        .declaration
        .columns
        .iter()
        .map(|column| (column.name.as_str(), column.data_type.as_str()))
        .collect();
    assert_eq!(columns, [("observation_id", "UUID"), ("title", "VARCHAR")]);

    let names = |rules: &[spool::Rule]| -> Vec<String> {
        rules.iter().map(|rule| rule.name.clone()).collect()
    };
    assert_eq!(names(&paper.rules.block), ["same_title"]);
    assert_eq!(names(&paper.rules.match_), ["a_prefix", "same_title"]);
    assert!(paper.rules.no_match.is_empty());
    // A file can end the way SQL files do.
    assert_eq!(paper.rules.match_[1].predicate, "l.title = r.title");
    assert!(manifest.entity_type("paper").is_some());
    assert!(manifest.entity_type("company").is_none());
}

#[test]
fn files_this_version_does_not_use_are_listed_and_ignored() {
    let dir = papers();
    let root = dir.path();
    write(root, "spool.toml", "");
    write(root, "entities/README.md", "");
    write(root, "entities/paper/policy.toml", "");
    write(root, "entities/paper/strategies/model.py", "");
    write(root, "entities/paper/rules/match/notes.txt", "");
    write(root, "entities/paper/rules/unsure/maybe.sql", "true");

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
    assert_eq!(
        ignored,
        [
            "entities/README.md",
            "entities/paper/policy.toml",
            "entities/paper/rules/match/notes.txt",
            "entities/paper/rules/unsure",
            "entities/paper/strategies",
        ]
    );
    assert_eq!(
        project.compile().unwrap(),
        Manifest::load(papers().path()).unwrap()
    );
}

#[test]
fn the_manifest_hash_covers_rules_and_the_declaration_hash_does_not() {
    let dir = papers();
    let before = Manifest::load(dir.path()).unwrap();

    write(
        dir.path(),
        "entities/paper/rules/no_match/never.sql",
        "false",
    );
    let with_rule = Manifest::load(dir.path()).unwrap();
    assert_ne!(with_rule.hash, before.hash);
    assert_eq!(
        with_rule.entity_types[0].declaration_hash,
        before.entity_types[0].declaration_hash
    );

    write(
        dir.path(),
        "entities/paper/features.sql",
        "SELECT observation_id, payload->>'title' AS title FROM observation",
    );
    let with_features = Manifest::load(dir.path()).unwrap();
    assert_ne!(
        with_features.entity_types[0].declaration_hash,
        before.entity_types[0].declaration_hash
    );
}

#[test]
fn a_mistake_fails_the_build_and_names_the_file() {
    let no_entities = tempfile::tempdir().unwrap();
    let (file, message) = project_error(no_entities.path());
    assert_eq!(file, "entities");
    assert!(message.contains("`entities` directory"), "{message}");

    let cases: [(&str, &str, &str); 8] = [
        (
            "entities/Paper-2/features.sql",
            FEATURES,
            "entities/Paper-2",
        ),
        (
            "entities/book/rules/match/any.sql",
            "true",
            "entities/book/features.sql",
        ),
        (
            "entities/paper/features.sql",
            "SELECT payload->>'title' AS title FROM observation",
            "entities/paper/features.sql",
        ),
        (
            "entities/paper/features.sql",
            "SELECT observation_id::VARCHAR AS observation_id FROM observation",
            "entities/paper/features.sql",
        ),
        (
            "entities/paper/features.sql",
            "SELECT observation_id, 1 AS declaration_version FROM observation",
            "entities/paper/features.sql",
        ),
        (
            "entities/paper/rules/match/typo.sql",
            "l.titel = r.title",
            "entities/paper/rules/match/typo.sql",
        ),
        (
            "entities/paper/rules/block/two.sql",
            "l.title = r.title; DROP TABLE spool.observation",
            "entities/paper/rules/block/two.sql",
        ),
        (
            "entities/paper/rules/no_match/empty.sql",
            "  \n",
            "entities/paper/rules/no_match/empty.sql",
        ),
    ];
    for (path, contents, expected_file) in cases {
        let dir = papers();
        write(dir.path(), path, contents);
        let (file, message) = project_error(dir.path());
        assert_eq!(file, expected_file, "{path}: {message}");
        assert!(!message.is_empty());
    }
}

#[test]
fn a_rule_name_must_be_plain() {
    let dir = papers();
    write(dir.path(), "entities/paper/rules/match/it's.sql", "true");
    let (file, message) = project_error(dir.path());
    assert_eq!(file, "entities/paper/rules/match/it's.sql");
    assert!(message.contains("file name"), "{message}");
}

#[test]
fn the_committed_projects_compile() {
    let companies = Manifest::load(benchmarks().join("identity/companies-small")).unwrap();
    let people = Manifest::load(benchmarks().join("resolution/people")).unwrap();

    let types = |manifest: &Manifest| -> Vec<String> {
        manifest
            .entity_types
            .iter()
            .map(|entity_type| entity_type.name.to_string())
            .collect()
    };
    assert_eq!(types(&companies), ["company"]);
    assert_eq!(types(&people), ["person"]);
    assert_ne!(companies.hash, people.hash);
}
