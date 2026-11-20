//! Compiling a project: checking every file it holds, and writing the
//! manifest.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use spool_core::{EntityType, StepClock};

use super::entity::entity_of;
use super::hash::hash_json;
use super::manifest::manifest_hash;
use super::source::source_of;
use super::transform::transform_of;
use super::{
    BUILT_IN, Declaration, EntityTypeManifest, Extractor, Manifest, Project, Rule, RuleSet, Source,
    SqlFile, Strategy, Transform, strip, valid_name,
};
use crate::error::{Error, Result};
use crate::features;
use crate::store::DuckStore;

impl Project {
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
