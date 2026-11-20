//! Project discovery: a directory of files in, a manifest out.
//!
//! The path is the name. `entities/company/` declares the entity type
//! `company`, and `entities/company/rules/match/same_domain.sql` is its match
//! rule `same_domain`. The engine never reads the directory, only the
//! [`Manifest`] compiled from it.
//!
//! ```text
//! my-project/
//! ├── sources/
//! │   └── crm.yaml                what comes in from the source `crm`: its key and columns
//! ├── transforms/
//! │   └── name_key.sql            a function of the values it is given from one record
//! ├── entities/
//! │   └── company/
//! │       ├── entity.yaml         the type's features, and how each source's columns become them
//! │       ├── extract.md          what an extractor is told about the type
//! │       ├── rules/
//! │       │   ├── block/*.sql     predicates over rows `l` and `r` that generate candidate pairs
//! │       │   ├── no_match/*.sql  predicates that judge a candidate `no_match`
//! │       │   └── match/*.sql     predicates that judge a candidate `match`
//! │       └── strategies/
//! │           ├── splink.py       user code: the strategy `splink`, run with `uv`
//! │           ├── splink/         files that belong to `splink`, such as its model
//! │           └── nearest.ts      the strategy `nearest`, run with `bun`
//! └── extractors/
//!     ├── names.py                user code: the extractor `names`, which finds mentions in documents
//!     └── names/                  files that belong to `names`
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spool_core::{EntityType, StepClock};

use crate::error::{Error, Result};
use crate::features;
use crate::store::DuckStore;

/// A project as read from disk, before any file has been checked.
#[derive(Clone, Debug, Default)]
pub struct Project {
    /// The files under `sources/`, one per source.
    pub sources: Vec<YamlFile>,
    /// The files under `transforms/`, one per transform.
    pub transforms: Vec<SqlFile>,
    pub entity_types: Vec<EntityTypeSource>,
    /// The files under `extractors/`.
    pub extractors: Vec<StrategySource>,
    /// Files of the project's directories that this version does not use.
    pub ignored: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct EntityTypeSource {
    pub name: String,
    /// The type's `entity.yaml`.
    pub entity: YamlFile,
    pub block: Vec<SqlFile>,
    pub no_match: Vec<SqlFile>,
    pub match_: Vec<SqlFile>,
    pub strategies: Vec<StrategySource>,
    /// The text of `extract.md`, if the type has one.
    pub extract: Option<String>,
}

/// A user-code file as found on disk: a strategy, or an extractor.
#[derive(Clone, Debug)]
pub struct StrategySource {
    /// The file's name without its extension.
    pub name: String,
    pub runtime: Runtime,
    pub path: PathBuf,
    /// The file's path from the project directory, with `/` between its
    /// parts on every platform.
    pub file: String,
    pub hash: String,
}

/// A SQL file. The name is the file's name without `.sql`.
#[derive(Clone, Debug)]
pub struct SqlFile {
    pub name: String,
    pub path: PathBuf,
    pub sql: String,
}

impl SqlFile {
    pub fn new(name: impl Into<String>, sql: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            path: PathBuf::from(format!("{name}.sql")),
            name,
            sql: sql.into(),
        }
    }
}

/// A YAML file. The name is the file's name without `.yaml`.
#[derive(Clone, Debug)]
pub struct YamlFile {
    pub name: String,
    pub path: PathBuf,
    pub yaml: String,
}

impl YamlFile {
    pub fn new(name: impl Into<String>, yaml: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            path: PathBuf::from(format!("{name}.yaml")),
            name,
            yaml: yaml.into(),
        }
    }
}

/// What the engine runs on: every source with what it sends, and every
/// entity type with its checked features and rules. The hash covers all of
/// it and is recorded with each match run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// What each source sends.
    #[serde(default)]
    pub sources: Vec<Source>,
    pub entity_types: Vec<EntityTypeManifest>,
    /// User-code extractors. Left out of the JSON when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extractors: Vec<Extractor>,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityTypeManifest {
    pub name: EntityType,
    pub declaration: Declaration,
    /// Covers the declaration and not the rules: rules can change between
    /// runs, a declaration cannot change under existing feature rows.
    pub declaration_hash: String,
    /// The sources whose columns the type's mapping reads. Rows of any
    /// other source are not records of this type.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Source, then feature, then the columns it is computed from: the
    /// mapping of `entity.yaml`. The declaration's query is written from
    /// it, so it is said here for people, not for the engine.
    #[serde(default)]
    pub mapping: BTreeMap<String, BTreeMap<String, Mapped>>,
    pub rules: RuleSet,
    /// User-code strategies. Left out of the JSON when there are none, so a
    /// project of SQL files has the manifest hash it always had.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strategies: Vec<Strategy>,
    /// The text of the type's `extract.md`: what an extractor is told about
    /// the type, in the project's own words. The engine does not read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<String>,
}

/// An extractor: a file the engine runs as a child process, which reads
/// documents and says where they mention entities. The engine runs the file
/// only while it still has this hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Extractor {
    pub name: String,
    pub runtime: Runtime,
    /// The file, as a path from the project directory with `/` between its
    /// parts.
    pub file: String,
    /// SHA-256 of the file and of every file in the directory that shares
    /// its name. It is recorded as the extractor's version with each run.
    pub hash: String,
}

/// A strategy that is user code: a file the engine runs as a child process.
/// The engine runs the file only while it still has this hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Strategy {
    pub name: String,
    pub runtime: Runtime,
    /// The file, as a path from the project directory with `/` between its
    /// parts.
    pub file: String,
    /// SHA-256 of the file and of every file in the directory that shares
    /// its name. It is recorded as the strategy's version with each run.
    pub hash: String,
}

/// What runs a strategy's or an extractor's file. The file's extension
/// decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    /// A `.py` file, run with `uv run`.
    Python,
    /// A `.ts` file, run with `bun run`.
    Typescript,
}

impl Runtime {
    fn of(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "py" => Some(Runtime::Python),
            "ts" => Some(Runtime::Typescript),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Runtime::Python => "python",
            Runtime::Typescript => "typescript",
        }
    }

    /// The command that installs what runs files of this kind, on Linux
    /// and macOS.
    pub const fn install_command(self) -> &'static str {
        match self {
            Runtime::Python => "curl -LsSf https://astral.sh/uv/install.sh | sh",
            Runtime::Typescript => "curl -fsSL https://bun.sh/install | bash",
        }
    }

    /// The page that says how else to install it.
    pub const fn install_page(self) -> &'static str {
        match self {
            Runtime::Python => "https://docs.astral.sh/uv/getting-started/installation/",
            Runtime::Typescript => "https://bun.sh/docs/installation",
        }
    }
}

impl std::fmt::Display for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl EntityTypeManifest {
    pub fn strategy(&self, name: &str) -> Option<&Strategy> {
        self.strategies
            .iter()
            .find(|strategy| strategy.name == name)
    }
}

/// What one source sends: the columns a row of it may have. A mention an
/// extractor finds in one of the source's documents may have those
/// attributes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub name: String,
    /// The column that holds each row's key within the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A column of JSON text that says where each row was found. It is
    /// stored as the record's evidence and is not part of its payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    pub columns: Vec<String>,
}

/// An entity type's features: the query compiled from its `entity.yaml`,
/// the columns DuckDB infers from it, and the transforms the query calls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    pub features: String,
    pub columns: Vec<FeatureColumn>,
    /// The feature that names a record to a person, if the type says one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<Transform>,
}

/// A transform: an expression over the values it is given from one record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transform {
    pub name: String,
    pub parameters: Vec<String>,
    pub body: String,
}

/// Where a feature comes from in one source: its columns, and the
/// transform or function they go through, if any.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Mapped {
    pub from: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FeatureColumn {
    pub name: String,
    pub data_type: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSet {
    pub block: Vec<Rule>,
    pub no_match: Vec<Rule>,
    #[serde(rename = "match")]
    pub match_: Vec<Rule>,
}

/// A named SQL predicate over a left row `l` and a right row `r` of the
/// entity type's feature table. `l` is the row with the smaller record
/// ID, so a predicate should not depend on which side is which.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub name: String,
    pub predicate: String,
}

impl Manifest {
    /// Reads and compiles the project at `dir`.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        Project::read(dir)?.compile()
    }

    /// Reads a manifest that [`write_file`](Self::write_file) wrote, and
    /// checks that its hashes are those of its content.
    pub fn read_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let invalid = |message: String| Error::Project {
            file: path.to_owned(),
            message,
        };
        let text = std::fs::read_to_string(path).map_err(Error::io(path))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|error| invalid(format!("this is not a manifest: {error}")))?;
        for entity_type in &manifest.entity_types {
            if hash_json(&entity_type.declaration) != entity_type.declaration_hash {
                return Err(invalid(format!(
                    "the declaration of `{}` does not have the hash recorded for it; build the manifest again",
                    entity_type.name
                )));
            }
        }
        if manifest_hash(
            &manifest.sources,
            &manifest.entity_types,
            &manifest.extractors,
        ) != manifest.hash
        {
            return Err(invalid(
                "the manifest does not have the hash recorded in it; build it again".to_owned(),
            ));
        }
        Ok(manifest)
    }

    /// Writes the manifest as JSON, creating the directory if it is missing.
    pub fn write_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(Error::io(dir))?;
        }
        let mut json = serde_json::to_string_pretty(self).expect("a manifest serialises to JSON");
        json.push('\n');
        std::fs::write(path, json).map_err(Error::io(path))
    }

    pub fn entity_type(&self, name: &str) -> Option<&EntityTypeManifest> {
        self.entity_types
            .iter()
            .find(|entity_type| entity_type.name.as_str() == name)
    }

    pub fn source(&self, name: &str) -> Option<&Source> {
        self.sources.iter().find(|source| source.name == name)
    }

    /// The source `name` as one whose rows or mentions become records of
    /// `entity_type`, or why they cannot.
    pub(crate) fn source_of(
        &self,
        name: &str,
        entity_type: &EntityTypeManifest,
    ) -> Result<&Source, String> {
        let source = self.source(name).ok_or_else(|| {
            let taken: Vec<String> = entity_type
                .sources
                .iter()
                .map(|source| format!("`{source}`"))
                .collect();
            let taken = match taken.is_empty() {
                true => String::new(),
                false => format!(
                    ". `{}` takes rows from {}",
                    entity_type.name,
                    taken.join(", ")
                ),
            };
            format!(
                "source `{name}` is not declared: the project has no sources/{name}.yaml{taken}"
            )
        })?;
        if !entity_type.sources.iter().any(|mapped| mapped == name) {
            return Err(format!(
                "the mapping of `{}` has nothing for source `{name}`: add it to entities/{}/entity.yaml",
                entity_type.name, entity_type.name
            ));
        }
        Ok(source)
    }

    pub fn extractor(&self, name: &str) -> Option<&Extractor> {
        self.extractors
            .iter()
            .find(|extractor| extractor.name == name)
    }
}

/// The hash of a manifest's content.
fn manifest_hash(
    sources: &[Source],
    entity_types: &[EntityTypeManifest],
    extractors: &[Extractor],
) -> String {
    hash_json(&(sources, entity_types, extractors))
}

impl Project {
    /// Discovers the sources, transforms and entity types under `dir`.
    /// Nothing is validated beyond names; [`compile`](Self::compile) checks
    /// the files.
    pub fn read(dir: impl AsRef<Path>) -> Result<Self> {
        let root = dir.as_ref();
        let entities = root.join("entities");
        if !entities.is_dir() {
            return Err(Error::Project {
                file: entities,
                message:
                    "a project needs an `entities` directory with one directory per entity type"
                        .to_owned(),
            });
        }
        let mut project = Project::default();
        let sources = root.join("sources");
        if sources.is_dir() {
            for file in list(&sources)? {
                if file.is_file() && file.extension().is_some_and(|ext| ext == "yaml") {
                    project.sources.push(read_yaml(&file)?);
                } else {
                    project.ignored.push(file);
                }
            }
        }
        let transforms = root.join("transforms");
        if transforms.is_dir() {
            for file in list(&transforms)? {
                if file.is_file() && file.extension().is_some_and(|ext| ext == "sql") {
                    project.transforms.push(read_sql(&file)?);
                } else {
                    project.ignored.push(file);
                }
            }
        }
        for entry in list(&entities)? {
            if !entry.is_dir() {
                project.ignored.push(entry);
                continue;
            }
            let name = file_name(&entry);
            EntityType::new(name.clone()).map_err(|error| Error::Project {
                file: entry.clone(),
                message: error.to_string(),
            })?;
            let entity_path = entry.join("entity.yaml");
            if !entity_path.is_file() {
                return Err(Error::Project {
                    file: entity_path,
                    message: format!("entity type `{name}` needs an entity.yaml"),
                });
            }
            let mut source = EntityTypeSource {
                name,
                entity: read_yaml(&entity_path)?,
                block: Vec::new(),
                no_match: Vec::new(),
                match_: Vec::new(),
                strategies: Vec::new(),
                extract: None,
            };
            for child in list(&entry)? {
                match file_name(&child).as_str() {
                    "entity.yaml" => {}
                    "extract.md" if child.is_file() => {
                        source.extract =
                            Some(std::fs::read_to_string(&child).map_err(Error::io(&child))?);
                    }
                    "rules" if child.is_dir() => {
                        for group in list(&child)? {
                            let rules = match file_name(&group).as_str() {
                                "block" if group.is_dir() => &mut source.block,
                                "no_match" if group.is_dir() => &mut source.no_match,
                                "match" if group.is_dir() => &mut source.match_,
                                _ => {
                                    project.ignored.push(group);
                                    continue;
                                }
                            };
                            for file in list(&group)? {
                                if file.is_file()
                                    && file.extension().is_some_and(|ext| ext == "sql")
                                {
                                    rules.push(read_sql(&file)?);
                                } else {
                                    project.ignored.push(file);
                                }
                            }
                        }
                    }
                    "strategies" if child.is_dir() => {
                        source.strategies = user_code(root, &child, &mut project.ignored)?;
                    }
                    _ => project.ignored.push(child),
                }
            }
            project.entity_types.push(source);
        }
        let extractors = root.join("extractors");
        if extractors.is_dir() {
            project.extractors = user_code(root, &extractors, &mut project.ignored)?;
        }
        Ok(project)
    }

    /// Checks every file, the SQL by preparing it against DuckDB, and
    /// returns the manifest. A mistake in a file fails here, not during a
    /// run.
    #[expect(clippy::too_many_lines)]
    pub fn compile(&self) -> Result<Manifest> {
        // A scratch database with the engine's schema to prepare against.
        let scratch = DuckStore::open(None, Box::new(StepClock::new(0, 0)))?;
        let in_path = |path: &Path| {
            let path = path.to_owned();
            move |message: String| Error::Project {
                file: path.clone(),
                message,
            }
        };

        let mut sources: Vec<Source> = Vec::new();
        for file in &self.sources {
            let invalid = in_path(&file.path);
            if sources.iter().any(|other| other.name == file.name) {
                return Err(invalid(format!("source `{}` is declared twice", file.name)));
            }
            sources.push(source_of(file).map_err(invalid)?);
        }
        sources.sort_by(|a, b| a.name.cmp(&b.name));

        let mut transforms: BTreeMap<String, Transform> = BTreeMap::new();
        for file in &self.transforms {
            let invalid = in_path(&file.path);
            if transforms.contains_key(&file.name) {
                return Err(invalid(format!(
                    "transform `{}` is defined twice",
                    file.name
                )));
            }
            let transform = transform_of(file).map_err(&invalid)?;
            features::check_transform(&scratch.conn, &transform)
                .map_err(|error| invalid(error.to_string()))?;
            transforms.insert(file.name.clone(), transform);
        }

        let mut names = BTreeSet::new();
        let mut entity_types = Vec::new();

        for source in &self.entity_types {
            let in_file = |file: &SqlFile| in_path(&file.path);
            let in_entity = in_path(&source.entity.path);
            let name = EntityType::new(source.name.clone())
                .map_err(|error| in_entity(error.to_string()))?;
            if !names.insert(name.clone()) {
                return Err(in_entity(format!("entity type `{name}` is declared twice")));
            }

            let entity = entity_of(&source.entity, &sources, &transforms).map_err(&in_entity)?;
            let columns = features::describe(&scratch.conn, &entity.transforms, &entity.query)
                .map_err(|error| in_entity(error.to_string()))?;
            let declaration = Declaration {
                features: entity.query,
                columns,
                label: entity.label,
                transforms: entity.transforms,
            };
            features::create_table(&scratch.conn, &name, &declaration, 1)
                .map_err(|error| in_entity(error.to_string()))?;

            let table = features::table(&name);
            let compile_rules = |files: &[SqlFile]| -> Result<Vec<Rule>> {
                let mut rules = Vec::new();
                let mut seen = BTreeSet::new();
                for file in files {
                    if !valid_name(&file.name) {
                        return Err(in_file(file)(
                            "a rule's file name may contain only letters, digits, `_` and `-`"
                                .to_owned(),
                        ));
                    }
                    if !seen.insert(file.name.clone()) {
                        return Err(in_file(file)(format!(
                            "rule `{}` is defined twice",
                            file.name
                        )));
                    }
                    let predicate = strip(&file.sql);
                    if predicate.is_empty() {
                        return Err(in_file(file)("the rule is empty".to_owned()));
                    }
                    scratch
                        .conn
                        .prepare(&format!(
                            "SELECT 1 FROM {table} l, {table} r WHERE (\n{predicate}\n)"
                        ))
                        .map_err(|error| in_file(file)(error.to_string()))?;
                    rules.push(Rule {
                        name: file.name.clone(),
                        predicate,
                    });
                }
                rules.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(rules)
            };
            let rules = RuleSet {
                block: compile_rules(&source.block)?,
                no_match: compile_rules(&source.no_match)?,
                match_: compile_rules(&source.match_)?,
            };

            let mut strategies = Vec::new();
            for strategy in &source.strategies {
                let invalid = |message: String| Error::Project {
                    file: strategy.path.clone(),
                    message,
                };
                if !valid_name(&strategy.name) {
                    return Err(invalid(
                        "a strategy's file name may contain only letters, digits, `_` and `-`"
                            .to_owned(),
                    ));
                }
                if strategy.name == BUILT_IN {
                    return Err(invalid(format!(
                        "`{BUILT_IN}` is the built-in strategy that runs the SQL rules; give this file another name"
                    )));
                }
                if strategies
                    .iter()
                    .any(|other: &Strategy| other.name == strategy.name)
                {
                    return Err(invalid(format!(
                        "strategy `{}` is defined twice",
                        strategy.name
                    )));
                }
                strategies.push(Strategy {
                    name: strategy.name.clone(),
                    runtime: strategy.runtime,
                    file: strategy.file.clone(),
                    hash: strategy.hash.clone(),
                });
            }
            strategies.sort_by(|a, b| a.name.cmp(&b.name));

            entity_types.push(EntityTypeManifest {
                declaration_hash: hash_json(&declaration),
                name,
                declaration,
                sources: entity.sources,
                mapping: entity.mapping,
                rules,
                strategies,
                extract: source
                    .extract
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned),
            });
        }

        let mut extractors: Vec<Extractor> = Vec::new();
        for extractor in &self.extractors {
            let invalid = |message: String| Error::Project {
                file: extractor.path.clone(),
                message,
            };
            if !valid_name(&extractor.name) {
                return Err(invalid(
                    "an extractor's file name may contain only letters, digits, `_` and `-`"
                        .to_owned(),
                ));
            }
            if extractors.iter().any(|other| other.name == extractor.name) {
                return Err(invalid(format!(
                    "extractor `{}` is defined twice",
                    extractor.name
                )));
            }
            extractors.push(Extractor {
                name: extractor.name.clone(),
                runtime: extractor.runtime,
                file: extractor.file.clone(),
                hash: extractor.hash.clone(),
            });
        }
        extractors.sort_by(|a, b| a.name.cmp(&b.name));

        entity_types.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Manifest {
            hash: manifest_hash(&sources, &entity_types, &extractors),
            sources,
            entity_types,
            extractors,
        })
    }
}

/// A source's file, as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceYaml {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    evidence: Option<String>,
    columns: Vec<String>,
}

/// An entity type's file, as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityYaml {
    #[serde(default)]
    label: Option<String>,
    features: Vec<String>,
    /// Source, then feature, then where the feature comes from.
    mapping: BTreeMap<String, BTreeMap<String, MappedYaml>>,
}

/// Where a feature comes from in one source: a column as it is, or columns
/// given to a transform.
#[derive(Deserialize)]
#[serde(untagged)]
enum MappedYaml {
    Column(String),
    Transformed(TransformedYaml),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformedYaml {
    from: FromYaml,
    #[serde(default)]
    transform: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FromYaml {
    One(String),
    Many(Vec<String>),
}

/// What an `entity.yaml` compiles to.
struct Entity {
    query: String,
    label: Option<String>,
    /// The transforms the query calls, in name order.
    transforms: Vec<Transform>,
    /// The sources the mapping reads, in name order.
    sources: Vec<String>,
    mapping: BTreeMap<String, BTreeMap<String, Mapped>>,
}

/// A name that can stand in SQL without quotes: lowercase letters, digits
/// and `_`, not starting with a digit.
fn identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn source_of(file: &YamlFile) -> Result<Source, String> {
    if !valid_name(&file.name) {
        return Err(
            "a source's file name may contain only letters, digits, `_` and `-`".to_owned(),
        );
    }
    let written: SourceYaml =
        serde_saphyr::from_str(&file.yaml).map_err(|error| error.to_string())?;
    if written.columns.is_empty() {
        return Err("a source lists the columns its rows may have".to_owned());
    }
    let mut seen = BTreeSet::new();
    for column in &written.columns {
        if column.is_empty() || column.contains(['"', '\\']) {
            return Err(format!(
                "column `{column}`: a column needs a name without `\"` or `\\`"
            ));
        }
        if !seen.insert(column.as_str()) {
            return Err(format!("column `{column}` is listed twice"));
        }
    }
    for (what, column) in [("key", &written.key), ("evidence", &written.evidence)] {
        if let Some(column) = column
            && !seen.contains(column.as_str())
        {
            return Err(format!(
                "the {what} is `{column}`, which is not one of the columns"
            ));
        }
    }
    if written.key.is_some() && written.key == written.evidence {
        return Err("the key and the evidence are the same column".to_owned());
    }
    Ok(Source {
        name: file.name.clone(),
        key: written.key,
        evidence: written.evidence,
        columns: written.columns,
    })
}

/// A transform's file: `lambda` and the names of its arguments, a colon, and
/// an expression over them, as DuckDB writes a lambda.
fn transform_of(file: &SqlFile) -> Result<Transform, String> {
    const FORM: &str = "a transform is `lambda`, the names of its arguments, `:` and an expression, as in `lambda name: lower(trim(name))`";
    if !identifier(&file.name) {
        return Err(
            "a transform's file name may contain only lowercase letters, digits and `_`".to_owned(),
        );
    }
    let text = after_comments(&file.sql);
    let rest = text
        .strip_prefix("lambda")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .ok_or(FORM)?;
    let (header, body) = rest.split_once(':').ok_or(FORM)?;
    let mut parameters: Vec<String> = Vec::new();
    for parameter in header.split(',').map(str::trim) {
        if !identifier(parameter) {
            return Err(format!(
                "`{parameter}` cannot name an argument: use lowercase letters, digits and `_`"
            ));
        }
        if parameters.iter().any(|other| other == parameter) {
            return Err(format!("the argument `{parameter}` is named twice"));
        }
        parameters.push(parameter.to_owned());
    }
    let body = strip(body);
    if body.is_empty() {
        return Err(FORM.to_owned());
    }
    if words(&body).any(|word| word.eq_ignore_ascii_case("select")) {
        return Err(
            "a transform works on the values it is given from one record, and cannot `select` from anything"
                .to_owned(),
        );
    }
    Ok(Transform {
        name: file.name.clone(),
        parameters,
        body,
    })
}

/// SQL text from its first character that is not whitespace or a comment.
fn after_comments(sql: &str) -> &str {
    let mut text = sql.trim_start();
    loop {
        if let Some(rest) = text.strip_prefix("--") {
            text = rest.split_once('\n').map_or("", |(_, rest)| rest);
        } else if let Some(rest) = text.strip_prefix("/*") {
            text = rest.split_once("*/").map_or("", |(_, rest)| rest);
        } else {
            return text;
        }
        text = text.trim_start();
    }
}

/// The words of SQL text that are not inside a string, a quoted name or a
/// comment.
fn words(sql: &str) -> impl Iterator<Item = &str> {
    // Every byte of a character outside ASCII counts as part of a word, so
    // the scan only ever stops between characters.
    let in_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii();
    let bytes = sql.as_bytes();
    let mut at = 0;
    std::iter::from_fn(move || {
        while at < bytes.len() {
            let byte = bytes[at];
            let rest = &bytes[at..];
            if byte == b'\'' || byte == b'"' {
                at = past(bytes, at + 1, &[byte]);
            } else if rest.starts_with(b"--") {
                at = past(bytes, at, b"\n");
            } else if rest.starts_with(b"/*") {
                at = past(bytes, at + 2, b"*/");
            } else if let Some(quote) = dollar_quote(rest) {
                at = past(bytes, at + quote.len(), quote);
            } else if in_word(byte) && !byte.is_ascii_digit() {
                let start = at;
                while at < bytes.len() && in_word(bytes[at]) {
                    at += 1;
                }
                return Some(&sql[start..at]);
            } else {
                at += 1;
            }
        }
        None
    })
}

/// Where the first `needle` at or after `from` ends, or the end of the text.
fn past(bytes: &[u8], from: usize, needle: &[u8]) -> usize {
    bytes
        .get(from..)
        .and_then(|rest| {
            rest.windows(needle.len())
                .position(|window| window == needle)
        })
        .map_or(bytes.len(), |found| from + found + needle.len())
}

/// The `$$` or `$tag$` that opens a dollar-quoted string, if the text begins
/// with one.
fn dollar_quote(text: &[u8]) -> Option<&[u8]> {
    let tag = text.strip_prefix(b"$")?;
    let length = tag
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_' || !byte.is_ascii())
        .count();
    let closed = tag.get(length) == Some(&b'$');
    let numbered = tag.first().is_some_and(u8::is_ascii_digit);
    (closed && !numbered).then(|| &text[..length + 2])
}

/// Checks an `entity.yaml` against the sources and transforms it names, and
/// writes the query that computes its features: one row per record, each
/// feature from the columns its source's mapping says.
#[expect(clippy::too_many_lines)]
fn entity_of(
    file: &YamlFile,
    sources: &[Source],
    transforms: &BTreeMap<String, Transform>,
) -> Result<Entity, String> {
    let written: EntityYaml =
        serde_saphyr::from_str(&file.yaml).map_err(|error| error.to_string())?;

    if written.features.is_empty() {
        return Err("an entity type lists its features".to_owned());
    }
    let mut features = BTreeSet::new();
    for feature in &written.features {
        if !identifier(feature) {
            return Err(format!(
                "`{feature}` cannot name a feature: use lowercase letters, digits and `_`"
            ));
        }
        if ["record_id", "declaration_version", "identity_id", "is_new"].contains(&feature.as_str())
        {
            return Err(format!(
                "`{feature}` is a column the engine adds; give the feature another name"
            ));
        }
        if !features.insert(feature.as_str()) {
            return Err(format!("feature `{feature}` is listed twice"));
        }
    }
    if let Some(label) = &written.label
        && !features.contains(label.as_str())
    {
        return Err(format!(
            "the label is `{label}`, which is not one of the features"
        ));
    }
    if written.mapping.is_empty() {
        return Err("the mapping names no source, so nothing can become a record".to_owned());
    }

    // Feature, then the expression each source computes it with.
    let mut expressions: BTreeMap<&str, Vec<(&str, String)>> = BTreeMap::new();
    let mut used: BTreeMap<&str, &Transform> = BTreeMap::new();
    let mut mapping: BTreeMap<String, BTreeMap<String, Mapped>> = BTreeMap::new();
    for (source_name, mapped) in &written.mapping {
        let source = sources
            .iter()
            .find(|source| &source.name == source_name)
            .ok_or_else(|| {
                format!("the mapping reads source `{source_name}`, and there is no sources/{source_name}.yaml")
            })?;
        for (feature, mapped) in mapped {
            let at = format!("mapping of `{feature}` from `{source_name}`");
            if !features.contains(feature.as_str()) {
                return Err(format!("{at}: `{feature}` is not one of the features"));
            }
            let (from, transform) = match mapped {
                MappedYaml::Column(column) => (vec![column.as_str()], None),
                MappedYaml::Transformed(mapped) => (
                    match &mapped.from {
                        FromYaml::One(column) => vec![column.as_str()],
                        FromYaml::Many(columns) => columns.iter().map(String::as_str).collect(),
                    },
                    mapped.transform.as_deref(),
                ),
            };
            let mut arguments = Vec::new();
            for column in &from {
                if !source.columns.iter().any(|known| known == column) {
                    return Err(format!(
                        "{at}: `{column}` is not a column of sources/{source_name}.yaml"
                    ));
                }
                // The column's value as text, whatever JSON type it has.
                arguments.push(format!(
                    "json_extract_string(payload, '$.\"{}\"')",
                    column.replace('\'', "''")
                ));
            }
            let expression = match transform {
                None => match &arguments[..] {
                    [one] => one.clone(),
                    _ => {
                        return Err(format!(
                            "{at}: {} columns need a transform to become one feature",
                            arguments.len()
                        ));
                    }
                },
                Some(name) => {
                    let function = match transforms.get(name) {
                        Some(transform) => {
                            if transform.parameters.len() != arguments.len() {
                                return Err(format!(
                                    "{at}: transform `{name}` takes {} ({}), and is given {}",
                                    transform.parameters.len(),
                                    transform.parameters.join(", "),
                                    arguments.len()
                                ));
                            }
                            used.insert(name, transform);
                            features::transform_function(name)
                        }
                        // A function DuckDB has, such as `upper`.
                        None if identifier(name) => name.to_owned(),
                        None => {
                            return Err(format!(
                                "{at}: `{name}` is not a file in transforms/ and cannot name a function"
                            ));
                        }
                    };
                    format!("{function}({})", arguments.join(", "))
                }
            };
            mapping.entry(source_name.clone()).or_default().insert(
                feature.clone(),
                Mapped {
                    from: from.iter().map(|column| (*column).to_owned()).collect(),
                    transform: transform.map(str::to_owned),
                },
            );
            expressions
                .entry(feature.as_str())
                .or_default()
                .push((source_name.as_str(), expression));
        }
    }

    let mut columns = Vec::new();
    for feature in &written.features {
        let Some(by_source) = expressions.get(feature.as_str()) else {
            return Err(format!("feature `{feature}` is not mapped from any source"));
        };
        let cases: Vec<String> = by_source
            .iter()
            .map(|(source, expression)| {
                format!("WHEN '{}' THEN {expression}", source.replace('\'', "''"))
            })
            .collect();
        columns.push(format!(
            "  CASE source_id\n    {}\n  END AS {feature}",
            cases.join("\n    ")
        ));
    }

    Ok(Entity {
        query: format!("SELECT\n  record_id,\n{}\nFROM record", columns.join(",\n")),
        label: written.label,
        transforms: used.into_values().cloned().collect(),
        sources: written.mapping.into_keys().collect(),
        mapping,
    })
}

/// The user-code files of a directory: every `.py` and `.ts` file, each
/// with the directory of the same name if there is one. Anything else in the
/// directory is not used.
fn user_code(root: &Path, dir: &Path, ignored: &mut Vec<PathBuf>) -> Result<Vec<StrategySource>> {
    let files = list(dir)?;
    let mut found = Vec::new();
    for file in &files {
        let runtime = Runtime::of(file).filter(|_| file.is_file());
        let Some(runtime) = runtime else {
            let owned = file.is_dir()
                && files.iter().any(|other| {
                    other.is_file()
                        && Runtime::of(other).is_some()
                        && other.file_stem() == file.file_name()
                });
            if !owned {
                ignored.push(file.clone());
            }
            continue;
        };
        let name = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        found.push(StrategySource {
            hash: strategy_hash(file)?,
            file: relative(root, file),
            name,
            runtime,
            path: file.clone(),
        });
    }
    Ok(found)
}

/// The name of the built-in strategy, which no file may take.
const BUILT_IN: &str = "rules";

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A path below the project directory, with `/` between its parts.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// SHA-256 of a strategy: its file, and every file in the directory beside
/// it that shares its name, which is where a strategy keeps what belongs to
/// it. What tools leave behind (`__pycache__`, `node_modules`, hidden files)
/// is not part of it.
pub(crate) fn strategy_hash(file: &Path) -> Result<String> {
    fn add(hasher: &mut Sha256, name: &str, path: &Path) -> Result<()> {
        let contents = std::fs::read(path).map_err(Error::io(path))?;
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((contents.len() as u64).to_le_bytes());
        hasher.update(&contents);
        Ok(())
    }
    fn walk(hasher: &mut Sha256, root: &Path, dir: &Path) -> Result<()> {
        for entry in list(dir)? {
            let name = file_name(&entry);
            if entry.is_dir() {
                if name != "__pycache__" && name != "node_modules" {
                    walk(hasher, root, &entry)?;
                }
            } else {
                add(hasher, &relative(root, &entry), &entry)?;
            }
        }
        Ok(())
    }
    let mut hasher = Sha256::new();
    add(&mut hasher, &file_name(file), file)?;
    let owned = file.with_extension("");
    if owned.is_dir() {
        walk(&mut hasher, &owned, &owned)?;
    }
    Ok(hex::encode(hasher.finalize()))
}

/// SHA-256 of several texts, each with its length, as lowercase hex.
pub(crate) fn hash_text(texts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for text in texts {
        hasher.update((text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// SHA-256 of a value's JSON form, as lowercase hex.
pub(crate) fn hash_json(value: &impl Serialize) -> String {
    let json = serde_json::to_vec(value).expect("manifest types serialise to JSON");
    hex::encode(Sha256::digest(&json))
}

/// Trims whitespace and trailing semicolons, so a file can end the way SQL
/// files usually do.
fn strip(sql: &str) -> String {
    sql.trim().trim_end_matches(';').trim_end().to_owned()
}

fn read_yaml(path: &Path) -> Result<YamlFile> {
    Ok(YamlFile {
        name: path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_owned(),
        yaml: std::fs::read_to_string(path).map_err(Error::io(path))?,
    })
}

fn read_sql(path: &Path) -> Result<SqlFile> {
    Ok(SqlFile {
        name: path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_owned(),
        sql: std::fs::read_to_string(path).map_err(Error::io(path))?,
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The entries of a directory in name order, without hidden files.
fn list(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(Error::io(dir))? {
        let path = entry.map_err(Error::io(dir))?.path();
        if !file_name(&path).starts_with('.') {
            entries.push(path);
        }
    }
    entries.sort();
    Ok(entries)
}
