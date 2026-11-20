//! A scenario: loading one, and checking it against its project.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use spool::{Manifest, StrategyRef};

use crate::datasets::find_dataset;
use crate::error::{Error, Result};
use crate::spec::{Correction, Spec, StepSpec};

#[derive(Clone, Debug)]
pub struct Scenario {
    /// The name of the directory that holds `scenario.toml`, or of the file
    /// if it is called something else.
    pub name: String,
    pub(crate) dir: PathBuf,
    pub(crate) spec: Spec,
}

impl Scenario {
    /// Loads a scenario from its `scenario.toml`, or from the directory that
    /// holds one.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = if path.is_dir() {
            path.join("scenario.toml")
        } else {
            path.to_owned()
        };
        let text = std::fs::read_to_string(&file).map_err(Error::io(&file))?;
        let invalid = |message: String| Error::Scenario(format!("{}: {message}", file.display()));
        let spec: Spec = toml::from_str(&text).map_err(|error| invalid(error.to_string()))?;

        let truth = &spec.truth;
        if truth.file.is_some() && truth.column.is_some() {
            return Err(invalid(
                "[truth] has `file` or `column`, not both".to_owned(),
            ));
        }
        if truth.file.is_none() && truth.column.is_none() && truth.mentions.is_none() {
            return Err(invalid(
                "[truth] needs `file`, `column` or `mentions`".to_owned(),
            ));
        }
        let rows_have_truth = truth.file.is_some() || truth.column.is_some();
        let mut named = HashSet::new();
        for step in &spec.steps {
            for (index, correction) in step.corrections.iter().enumerate() {
                let invalid = |message: String| {
                    invalid(format!(
                        "step `{}`, correction {}: {message}",
                        step.name,
                        index + 1
                    ))
                };
                let kind = correction.kind().map_err(invalid)?;
                if let Correction::Split([]) = kind {
                    return Err(invalid("a split needs records".to_owned()));
                }
                for record in kind.records() {
                    if record.split_once(':').is_none() {
                        return Err(invalid(format!(
                            "`{record}` does not name a record as `source:key`"
                        )));
                    }
                }
                if let Correction::Undo(name) = kind
                    && !named.contains(name)
                {
                    return Err(invalid(format!("no earlier correction is named `{name}`")));
                }
                if let Some(name) = &correction.name
                    && !named.insert(name.as_str())
                {
                    return Err(invalid(format!("two corrections are named `{name}`")));
                }
            }
            for (index, retract) in step.retract.iter().enumerate() {
                retract.check().map_err(|message| {
                    invalid(format!(
                        "step `{}`, retraction {}: {message}",
                        step.name,
                        index + 1
                    ))
                })?;
            }
            for ingest in &step.ingest {
                if ingest.file.is_some() == ingest.dataset.is_some() {
                    return Err(invalid(format!(
                        "step `{}`: an ingest needs exactly one of `file` and `dataset`",
                        step.name
                    )));
                }
                if ingest.member.is_some() && ingest.dataset.is_none() {
                    return Err(invalid(format!(
                        "step `{}`: `member` names a file within a `dataset`, and the ingest reads a `file`",
                        step.name
                    )));
                }
                if !rows_have_truth {
                    return Err(invalid(format!(
                        "step `{}` ingests rows, and [truth] has no `file` or `column` to say what they are",
                        step.name
                    )));
                }
            }
        }

        let dir = file
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_owned();
        // The path is the name: the directory for a `scenario.toml`, and the
        // file otherwise, so one project can hold several scenarios.
        let stem = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned());
        let name = match stem {
            Some(stem) if stem != "scenario" => stem,
            _ => std::fs::canonicalize(&dir)
                .ok()
                .and_then(|dir| {
                    dir.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "scenario".to_owned()),
        };
        Ok(Self { name, dir, spec })
    }

    /// The project directory the scenario runs against.
    pub fn project_dir(&self) -> PathBuf {
        self.dir.join(&self.spec.project).components().collect()
    }

    /// The same scenario with another strategy for every step that does not
    /// name its own.
    pub fn with_strategy(mut self, strategy: impl Into<String>) -> Self {
        self.spec.strategy = Some(strategy.into());
        self
    }

    /// The strategy a step resolves with.
    pub(crate) fn strategy<'a>(&'a self, step: &'a StepSpec) -> &'a str {
        step.strategy
            .as_deref()
            .or(self.spec.strategy.as_deref())
            .unwrap_or(StrategyRef::RULES)
    }

    /// Checks the scenario against its project and against `datasets.toml`,
    /// and returns the project's manifest: every ingest names a source the
    /// entity type maps, and a file that is there or a dataset that is
    /// listed. Nothing is downloaded, so a scenario that cannot run says so
    /// before it has fetched anything.
    pub fn check(&self) -> Result<Manifest> {
        let manifest = Manifest::load(self.project_dir())?;
        // The rows a step reads: what they are read for, and where from.
        struct Read<'a> {
            what: &'a str,
            source: &'a str,
            entity_type: &'a str,
            file: Option<&'a PathBuf>,
            dataset: Option<&'a String>,
            member: Option<&'a str>,
        }
        for step in &self.spec.steps {
            let ingests = step.ingest.iter().map(|ingest| Read {
                what: "ingest",
                source: &ingest.source,
                entity_type: &ingest.entity_type,
                file: ingest.file.as_ref(),
                dataset: ingest.dataset.as_ref(),
                member: ingest.member.as_deref(),
            });
            let retractions = step.retract.iter().map(|retract| Read {
                what: "retraction",
                source: &retract.source,
                entity_type: &retract.entity_type,
                file: retract.file.as_ref(),
                dataset: retract.dataset.as_ref(),
                member: retract.member.as_deref(),
            });
            for read in ingests.chain(retractions) {
                let invalid = |message: String| {
                    Error::Scenario(format!(
                        "step `{}`, the {} from `{}`: {message}",
                        step.name, read.what, read.source
                    ))
                };
                let Some(entity_type) = manifest
                    .entity_types
                    .iter()
                    .find(|entity_type| entity_type.name.as_str() == read.entity_type)
                else {
                    return Err(invalid(format!(
                        "the project has no entity type `{}`",
                        read.entity_type
                    )));
                };
                if !entity_type
                    .sources
                    .iter()
                    .any(|source| source == read.source)
                {
                    return Err(invalid(format!(
                        "the mapping of `{}` has no block for the source",
                        read.entity_type
                    )));
                }
                match (read.file, read.dataset) {
                    (Some(file), _) => {
                        let path = self.dir.join(file);
                        if !path.is_file() {
                            return Err(invalid(format!("there is no file {}", path.display())));
                        }
                    }
                    (None, Some(dataset)) => {
                        find_dataset(&self.dir, dataset, read.member)
                            .map_err(|error| invalid(error.to_string()))?;
                    }
                    (None, None) => {}
                }
            }
        }
        Ok(manifest)
    }
}
