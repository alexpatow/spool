//! Discovering a project: which files a directory holds, and what each
//! is by its path.

use std::path::{Path, PathBuf};

use spool_core::EntityType;

use super::hash::strategy_hash;
use super::{
    EntityTypeSource, Project, Runtime, SqlFile, StrategySource, YamlFile, file_name, list,
    relative,
};
use crate::error::{Error, Result};

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
