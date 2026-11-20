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

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

mod compile;
mod discover;
mod entity;
mod hash;
mod manifest;
mod source;
mod transform;

pub(crate) use hash::{hash_json, hash_text, strategy_hash};
pub use manifest::{
    Declaration, EntityTypeManifest, Extractor, FeatureColumn, Manifest, Mapped, Rule, RuleSet,
    Runtime, Source, Strategy, Transform,
};

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

/// Whether a name can stand in SQL without quotes: lowercase letters, digits
/// and `_`, not starting with a digit.
fn is_sql_identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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
fn format_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Trims whitespace and trailing semicolons, so a file can end the way SQL
/// files usually do.
fn trim_sql(sql: &str) -> String {
    sql.trim().trim_end_matches(';').trim_end().to_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The entries of a directory in name order, without hidden files.
fn list_dir_entries(dir: &Path) -> Result<Vec<PathBuf>> {
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
