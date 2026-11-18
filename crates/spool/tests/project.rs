//! Project discovery and compilation.

mod common;

use std::fs;
use std::path::Path;

use common::benchmarks;
use spool::{Error, Manifest, Project};

const SOURCE: &str = "key: id\ncolumns: [id, title]\n";
const ENTITY: &str = "\
features: [title]
mapping:
  journal:
    title: { from: title, transform: folded }
";

/// An `entity.yaml` of one feature, `title`, mapped from the journal as given.
fn mapped(how: &str) -> String {
    format!("features: [title]\nmapping:\n  journal:\n    title: {how}\n")
}

fn write(root: &Path, file: &str, contents: &str) {
    let path = root.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A project with one entity type, `paper`.
fn papers() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "sources/journal.yaml", SOURCE);
    write(root, "transforms/folded.sql", "lambda text: lower(text)\n");
    write(root, "entities/paper/entity.yaml", ENTITY);
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
    assert_eq!(columns, [("record_id", "UUID"), ("title", "VARCHAR")]);

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
    write(root, "entities/paper/strategies/notes.md", "");
    write(root, "entities/paper/strategies/weights/model.json", "{}");
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
            "entities/paper/strategies/notes.md",
            "entities/paper/strategies/weights",
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

    write(dir.path(), "entities/paper/entity.yaml", &mapped("title"));
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

    let entity = "entities/paper/entity.yaml";
    let cases: [(&str, String, &str); 22] = [
        // A source.
        (
            "sources/journal.yaml",
            "key: id\n".into(),
            "sources/journal.yaml",
        ),
        (
            "sources/journal.yaml",
            "key: doi\ncolumns: [id, title]\n".into(),
            "sources/journal.yaml",
        ),
        (
            "sources/journal.yaml",
            "key: id\ncolumns: [id, title, id]\n".into(),
            "sources/journal.yaml",
        ),
        (
            "sources/journal.yaml",
            "key: id\nfields: [id, title]\n".into(),
            "sources/journal.yaml",
        ),
        ("sources/it's.yaml", SOURCE.into(), "sources/it's.yaml"),
        // A transform.
        (
            "transforms/folded.sql",
            "lower(text)".into(),
            "transforms/folded.sql",
        ),
        (
            "transforms/folded.sql",
            "lambda text: lowr(text)".into(),
            "transforms/folded.sql",
        ),
        (
            "transforms/folded.sql",
            "lambda text: (SELECT max(source_key) FROM spool.record)".into(),
            "transforms/folded.sql",
        ),
        // An entity type.
        (
            entity,
            "features: [is_new]\nmapping:\n  journal:\n    is_new: title\n".into(),
            entity,
        ),
        (
            entity,
            "features: [title, title]\nmapping:\n  journal:\n    title: title\n".into(),
            entity,
        ),
        (entity, format!("label: name\n{}", mapped("title")), entity),
        (entity, mapped("heading"), entity),
        (
            entity,
            mapped("{ from: [id, title], transform: folded }"),
            entity,
        ),
        (entity, mapped("{ from: [id, title] }"), entity),
        (
            entity,
            "features: [title, year]\nmapping:\n  journal:\n    title: title\n".into(),
            entity,
        ),
        (
            entity,
            "features: [title]\nmapping:\n  preprints:\n    title: title\n".into(),
            entity,
        ),
        (
            "entities/Paper-2/entity.yaml",
            ENTITY.into(),
            "entities/Paper-2",
        ),
        (
            "entities/book/rules/match/any.sql",
            "true".into(),
            "entities/book/entity.yaml",
        ),
        // A strategy and a rule.
        (
            "entities/paper/strategies/rules.py",
            String::new(),
            "entities/paper/strategies/rules.py",
        ),
        (
            "entities/paper/rules/match/typo.sql",
            "l.titel = r.title".into(),
            "entities/paper/rules/match/typo.sql",
        ),
        (
            "entities/paper/rules/block/two.sql",
            "l.title = r.title; DROP TABLE spool.record".into(),
            "entities/paper/rules/block/two.sql",
        ),
        (
            "entities/paper/rules/no_match/empty.sql",
            "  \n".into(),
            "entities/paper/rules/no_match/empty.sql",
        ),
    ];
    for (path, contents, expected_file) in cases {
        let dir = papers();
        write(dir.path(), path, &contents);
        let (file, message) = project_error(dir.path());
        assert_eq!(file, expected_file, "{path}: {contents}: {message}");
        assert!(!message.is_empty());
    }
}

#[test]
fn a_transform_may_hold_any_text_in_its_strings() {
    for body in [
        "replace(lower(text), $$é$$, 'e')",
        "replace(text, 'é — select', $word$ select ü $word$)",
        "replace(text, 'ü', 'u') -- ö\n",
        "text /* ø */ || \"text\"",
    ] {
        let dir = papers();
        write(
            dir.path(),
            "transforms/folded.sql",
            &format!("lambda text: {body}"),
        );
        Manifest::load(dir.path()).unwrap_or_else(|error| panic!("{body}: {error}"));
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
fn a_strategy_file_is_named_by_its_path() {
    let dir = papers();
    let root = dir.path();
    let plain = Manifest::load(root).unwrap();
    // A project of SQL files has the manifest it always had.
    assert!(
        !serde_json::to_string(&plain)
            .unwrap()
            .contains("strategies")
    );

    write(root, "entities/paper/strategies/model.py", "# a model\n");
    write(root, "entities/paper/strategies/model/weights.json", "{}");
    write(root, "entities/paper/strategies/nearest.ts", "// nearest\n");
    let project = Project::read(root).unwrap();
    assert!(project.ignored.is_empty(), "{:?}", project.ignored);
    let manifest = project.compile().unwrap();
    assert_ne!(manifest.hash, plain.hash);

    let paper = manifest.entity_type("paper").unwrap();
    let listed: Vec<(&str, spool::Runtime, &str)> = paper
        .strategies
        .iter()
        .map(|strategy| {
            (
                strategy.name.as_str(),
                strategy.runtime,
                strategy.file.as_str(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (
                "model",
                spool::Runtime::Python,
                "entities/paper/strategies/model.py"
            ),
            (
                "nearest",
                spool::Runtime::Typescript,
                "entities/paper/strategies/nearest.ts"
            ),
        ]
    );
    assert_eq!(paper.strategy("model").unwrap().hash.len(), 64);
    assert!(paper.strategy("rules").is_none());
    // The declaration is the features query alone.
    assert_eq!(
        paper.declaration_hash,
        plain.entity_types[0].declaration_hash
    );

    // A manifest with strategies reads back as it was written.
    let file = root.join(".spool/manifest.json");
    manifest.write_file(&file).unwrap();
    assert_eq!(Manifest::read_file(&file).unwrap(), manifest);
}

#[test]
fn a_strategy_has_the_hash_of_its_file_and_of_what_belongs_to_it() {
    let dir = papers();
    let root = dir.path();
    write(root, "entities/paper/strategies/model.py", "# a model\n");
    write(root, "entities/paper/strategies/model/weights.json", "{}");
    write(root, "entities/paper/strategies/nearest.ts", "// nearest\n");
    let hashes = || {
        let manifest = Manifest::load(root).unwrap();
        let paper = manifest.entity_type("paper").unwrap();
        (
            paper.strategy("model").unwrap().hash.clone(),
            paper.strategy("nearest").unwrap().hash.clone(),
            manifest.hash,
        )
    };
    let (model, nearest, manifest) = hashes();
    assert_ne!(model, nearest);

    // The file itself.
    write(
        root,
        "entities/paper/strategies/model.py",
        "# a better model\n",
    );
    let edited = hashes();
    assert_ne!(edited.0, model);
    assert_eq!(edited.1, nearest);
    assert_ne!(edited.2, manifest);

    // A file in the directory that shares its name, at any depth.
    write(
        root,
        "entities/paper/strategies/model/weights.json",
        "{\"w\": 1}",
    );
    let retrained = hashes();
    assert_ne!(retrained.0, edited.0);
    write(root, "entities/paper/strategies/model/lib/helper.py", "");
    let with_helper = hashes();
    assert_ne!(with_helper.0, retrained.0);
    assert_eq!(with_helper.1, nearest);

    // What tools leave behind is not part of a strategy.
    write(
        root,
        "entities/paper/strategies/model/__pycache__/helper.pyc",
        "compiled",
    );
    write(
        root,
        "entities/paper/strategies/model/node_modules/dep/index.js",
        "",
    );
    write(root, "entities/paper/strategies/model/.cache", "");
    assert_eq!(hashes(), with_helper);
}

#[test]
fn an_extractor_belongs_to_the_project_and_instructions_to_a_type() {
    let dir = papers();
    let root = dir.path();
    let plain = Manifest::load(root).unwrap();
    // A project without extractors has the manifest it always had.
    let text = serde_json::to_string(&plain).unwrap();
    assert!(!text.contains("extractors"), "{text}");

    write(root, "extractors/citations.py", "# citations\n");
    write(root, "extractors/citations/prompt.md", "Find the papers.\n");
    write(root, "extractors/titles.ts", "// titles\n");
    write(root, "extractors/notes.md", "");
    write(
        root,
        "entities/paper/extract.md",
        "\nA paper is cited by its title.\n\n",
    );
    let project = Project::read(root).unwrap();
    let ignored: Vec<String> = project
        .ignored
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(ignored, ["extractors/notes.md"]);
    let manifest = project.compile().unwrap();
    assert_ne!(manifest.hash, plain.hash);

    let listed: Vec<(&str, spool::Runtime, &str)> = manifest
        .extractors
        .iter()
        .map(|extractor| {
            (
                extractor.name.as_str(),
                extractor.runtime,
                extractor.file.as_str(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (
                "citations",
                spool::Runtime::Python,
                "extractors/citations.py"
            ),
            ("titles", spool::Runtime::Typescript, "extractors/titles.ts"),
        ]
    );
    assert!(manifest.extractor("missing").is_none());
    let paper = manifest.entity_type("paper").unwrap();
    assert_eq!(
        paper.extract.as_deref(),
        Some("A paper is cited by its title.")
    );
    // What to look for is no part of the declaration: features are not
    // computed again when the instructions change.
    assert_eq!(
        paper.declaration_hash,
        plain.entity_types[0].declaration_hash
    );

    let file = root.join(".spool/manifest.json");
    manifest.write_file(&file).unwrap();
    assert_eq!(Manifest::read_file(&file).unwrap(), manifest);

    // The manifest's hash covers the extractor, what belongs to it, and
    // the instructions.
    let hash = |root: &Path| Manifest::load(root).unwrap().hash;
    let before = hash(root);
    write(
        root,
        "extractors/citations/prompt.md",
        "Find every paper.\n",
    );
    let prompt = hash(root);
    assert_ne!(prompt, before);
    assert_ne!(
        Manifest::load(root)
            .unwrap()
            .extractor("citations")
            .unwrap()
            .hash,
        manifest.extractor("citations").unwrap().hash
    );
    write(root, "entities/paper/extract.md", "A paper has a title.\n");
    assert_ne!(hash(root), prompt);
}

#[test]
fn two_files_cannot_be_one_extractor() {
    let dir = papers();
    write(dir.path(), "extractors/titles.py", "");
    write(dir.path(), "extractors/titles.ts", "");
    let (file, message) = project_error(dir.path());
    assert_eq!(file, "extractors/titles.ts");
    assert!(message.contains("defined twice"), "{message}");

    let dir = papers();
    write(dir.path(), "extractors/Find All.py", "");
    let (file, message) = project_error(dir.path());
    assert_eq!(file, "extractors/Find All.py");
    assert!(message.contains("name"), "{message}");
}

#[test]
fn two_files_cannot_be_one_strategy() {
    let dir = papers();
    write(dir.path(), "entities/paper/strategies/model.py", "");
    write(dir.path(), "entities/paper/strategies/model.ts", "");
    let (file, message) = project_error(dir.path());
    assert_eq!(file, "entities/paper/strategies/model.ts");
    assert!(message.contains("defined twice"), "{message}");
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
    for (project, entity_type) in [("voters", "person"), ("music", "song")] {
        let manifest = Manifest::load(benchmarks().join("resolution").join(project)).unwrap();
        assert_eq!(types(&manifest), [entity_type]);
        assert_eq!(manifest.sources.len(), 5);
    }
    assert_ne!(companies.hash, people.hash);
}
