//! Project discovery: a directory of files in, a manifest out.
//!
//! The path is the name. `entities/company/` declares the entity type
//! `company`, and `entities/company/rules/match/same_domain.sql` is its match
//! rule `same_domain`. The engine never reads the directory, only the
//! [`Manifest`] compiled from it.
//!
//! ```text
//! my-project/
//! └── entities/
//!     └── company/
//!         ├── features.sql        one query; its columns are the feature table
//!         └── rules/
//!             ├── block/*.sql     predicates over rows `l` and `r` that generate candidate pairs
//!             ├── no_match/*.sql  predicates that judge a candidate `no_match`
//!             └── match/*.sql     predicates that judge a candidate `match`
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
    /// Files under `entities/` that this version does not use.
    pub ignored: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct EntityTypeSource {
    pub name: String,
    pub features: SqlFile,
    pub block: Vec<SqlFile>,
    pub no_match: Vec<SqlFile>,
    pub match_: Vec<SqlFile>,
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
/// entity type's feature table. `l` is the row with the smaller observation
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
        if hash_json(&manifest.entity_types) != manifest.hash {
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
}

impl Project {
    /// Discovers the entity types under `dir/entities`. Nothing is validated
    /// beyond names; [`compile`](Self::compile) checks the SQL.
    pub fn read(dir: impl AsRef<Path>) -> Result<Self> {
        let entities = dir.as_ref().join("entities");
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
            };
            for child in list(&entry)? {
                match file_name(&child).as_str() {
                    "features.sql" => {}
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
                    _ => project.ignored.push(child),
                }
            }
            project.entity_types.push(source);
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
                    let valid_name = !file.name.is_empty()
                        && file
                            .name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                    if !valid_name {
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

            entity_types.push(EntityTypeManifest {
                declaration_hash: hash_json(&declaration),
                name,
                declaration,
                rules,
            });
        }

        entity_types.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Manifest {
            hash: hash_json(&entity_types),
            entity_types,
        })
    }
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
