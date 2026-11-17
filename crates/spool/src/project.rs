//! Project discovery: a directory of files in, a manifest out.
//!
//! The path is the name. `entities/company/` declares the entity type
//! `company`, and `entities/company/rules/match/same_domain.sql` is its match
//! rule `same_domain`. The engine never reads the directory, only the
//! [`Manifest`] compiled from it.
//!
//! ```text
//! my-project/
//! ├── entities/
//! │   └── company/
//! │       ├── features.sql        one query; its columns are the feature table
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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spool_core::{EntityType, StepClock};

use crate::error::{Error, Result};
use crate::features;
use crate::store::DuckStore;

/// A project as read from disk, before any SQL has been checked.
#[derive(Clone, Debug, Default)]
pub struct Project {
    pub entity_types: Vec<EntityTypeSource>,
    /// The files under `extractors/`.
    pub extractors: Vec<StrategySource>,
    /// Files under `entities/` and `extractors/` that this version does not
    /// use.
    pub ignored: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct EntityTypeSource {
    pub name: String,
    pub features: SqlFile,
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

/// What the engine runs on: every entity type with its checked features query
/// and rules. The hash covers all of it and is recorded with each match run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
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

/// What runs a strategy file. The file's extension decides.
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

/// The features query and the columns DuckDB infers from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    pub features: String,
    pub columns: Vec<FeatureColumn>,
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
        if manifest_hash(&manifest.entity_types, &manifest.extractors) != manifest.hash {
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

    pub fn extractor(&self, name: &str) -> Option<&Extractor> {
        self.extractors
            .iter()
            .find(|extractor| extractor.name == name)
    }
}

/// The hash of a manifest's content. A project without extractors has the
/// hash it had before there were any.
fn manifest_hash(entity_types: &[EntityTypeManifest], extractors: &[Extractor]) -> String {
    if extractors.is_empty() {
        hash_json(&entity_types)
    } else {
        hash_json(&(entity_types, extractors))
    }
}

impl Project {
    /// Discovers the entity types under `dir/entities`. Nothing is validated
    /// beyond names; [`compile`](Self::compile) checks the SQL.
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
            let features_path = entry.join("features.sql");
            if !features_path.is_file() {
                return Err(Error::Project {
                    file: features_path,
                    message: format!("entity type `{name}` needs a features.sql"),
                });
            }
            let mut source = EntityTypeSource {
                name,
                features: read_sql(&features_path)?,
                block: Vec::new(),
                no_match: Vec::new(),
                match_: Vec::new(),
                strategies: Vec::new(),
                extract: None,
            };
            for child in list(&entry)? {
                match file_name(&child).as_str() {
                    "features.sql" => {}
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

    /// Checks every SQL file by preparing it against DuckDB and returns the
    /// manifest. A mistake in a file fails here, not during a run.
    pub fn compile(&self) -> Result<Manifest> {
        // A scratch database with the engine's schema to prepare against.
        let scratch = DuckStore::open(None, Box::new(StepClock::new(0, 0)))?;
        let mut names = BTreeSet::new();
        let mut entity_types = Vec::new();

        for source in &self.entity_types {
            let in_file = |file: &SqlFile| {
                let path = file.path.clone();
                move |message: String| Error::Project {
                    file: path,
                    message,
                }
            };
            let name = EntityType::new(source.name.clone())
                .map_err(|error| in_file(&source.features)(error.to_string()))?;
            if !names.insert(name.clone()) {
                return Err(in_file(&source.features)(format!(
                    "entity type `{name}` is declared twice"
                )));
            }

            let query = strip(&source.features.sql);
            let columns = features::describe(&scratch.conn, &query)
                .map_err(|error| in_file(&source.features)(error.to_string()))?;
            features::check_columns(&columns).map_err(in_file(&source.features))?;
            let declaration = Declaration {
                features: query,
                columns,
            };
            features::create_table(&scratch.conn, &name, &declaration, 1)
                .map_err(|error| in_file(&source.features)(error.to_string()))?;

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
            hash: manifest_hash(&entity_types, &extractors),
            entity_types,
            extractors,
        })
    }
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
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// SHA-256 of several texts, each with its length, as lowercase hex.
pub(crate) fn hash_text(texts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for text in texts {
        hasher.update((text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// SHA-256 of a value's JSON form, as lowercase hex.
pub(crate) fn hash_json(value: &impl Serialize) -> String {
    let json = serde_json::to_vec(value).expect("manifest types serialise to JSON");
    Sha256::digest(&json)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Trims whitespace and trailing semicolons, so a file can end the way SQL
/// files usually do.
fn strip(sql: &str) -> String {
    sql.trim().trim_end_matches(';').trim_end().to_owned()
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
