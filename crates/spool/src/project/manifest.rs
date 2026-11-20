//! The manifest: what a project compiles to, and what the engine runs on.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use spool_core::EntityType;

use super::Project;
use super::hash::hash_json;
use crate::error::{Error, Result};

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
    pub(super) fn of(path: &Path) -> Option<Self> {
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
pub(super) fn manifest_hash(
    sources: &[Source],
    entity_types: &[EntityTypeManifest],
    extractors: &[Extractor],
) -> String {
    hash_json(&(sources, entity_types, extractors))
}
