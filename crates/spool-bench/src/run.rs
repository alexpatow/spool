//! Running a scenario: each step feeds the engine, takes rows back,
//! applies its corrections, resolves and is scored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use spool::core::{Actor, DocumentId};
use spool::{
    DocumentFilter, Engine, ExtractOptions, NewDocument, ResolveOptions, ResolveReport, Rows,
    Suggestions, UserCode,
};

use crate::corrections::{Tracked, correct, held};
use crate::datasets::fetch_dataset;
use crate::error::{Error, Result};
use crate::metrics::UpgradeStability;
use crate::replay::Gone;
use crate::report::{
    BenchReport, DocumentsSummary, ExtractSummary, IngestSummary, RetractSummary, StepReport,
};
use crate::rows::{take_column, without_keys};
use crate::scenario::Scenario;
use crate::score::{end_to_end, extraction, possible_splits, truth_relation};
use crate::spec::{IngestSpec, RetractSpec, StepSpec};
use crate::truth::{Truth, read_labels, read_truth_file};

/// What feeding the engine one step's files, documents and extractions
/// reported.
pub(crate) struct Fed {
    ingest: Vec<IngestSummary>,
    documents: Vec<DocumentsSummary>,
    extract: Vec<ExtractSummary>,
}

impl Fed {
    fn new_records(&self) -> u64 {
        self.ingest
            .iter()
            .map(|ingest| ingest.inserted)
            .chain(self.extract.iter().map(|extract| extract.inserted))
            .sum()
    }
}

impl Scenario {
    fn resolve(&self, engine: &Engine, step: &StepSpec) -> Result<ResolveReport> {
        Ok(engine.resolve(&ResolveOptions {
            strategy: Some(self.strategy(step).to_owned()),
            rematch: step.rematch,
            ..ResolveOptions::default()
        })?)
    }

    /// The truth the scenario's files hold before anything is ingested.
    pub(crate) fn truth(&self) -> Result<Truth> {
        let mut truth = Truth::default();
        if let Some(file) = &self.spec.truth.file {
            let path = self.dir.join(file);
            for (source, key, entity) in read_truth_file(&path)? {
                truth.entities.insert((source, key), entity);
            }
        }
        if let Some(file) = &self.spec.truth.mentions {
            truth.labelled = true;
            for (document, label) in read_labels(&self.dir.join(file))? {
                truth.labels.entry(document).or_default().push(label);
            }
        }
        Ok(truth)
    }

    /// Feeds the engine what a step brings: its files, then its documents,
    /// then its extractions. The rows in `without` are left out of the
    /// files.
    pub(crate) fn feed(
        &self,
        engine: &Engine,
        step: &StepSpec,
        truth: &mut Truth,
        without: &Gone,
    ) -> Result<Fed> {
        let mut fed = Fed {
            ingest: Vec::new(),
            documents: Vec::new(),
            extract: Vec::new(),
        };
        for ingest in &step.ingest {
            fed.ingest
                .push(self.ingest(engine, ingest, truth, without)?);
        }
        for documents in &step.documents {
            let read = read_documents(&self.dir.join(&documents.dir))?;
            let report = engine.add_documents(&documents.source, &read)?;
            // The engine names each version, and a text a document held
            // before is not named by the text alone.
            let stored: HashMap<String, DocumentId> = if truth.labelled {
                engine
                    .documents(&DocumentFilter {
                        source: Some(documents.source.clone()),
                        latest: true,
                        limit: u64::MAX,
                        ..DocumentFilter::default()
                    })?
                    .into_iter()
                    .map(|document| (document.key, document.id))
                    .collect()
            } else {
                HashMap::new()
            };
            for document in &read {
                if let Some(id) = stored.get(&document.key) {
                    truth.add_document(&documents.source, *id, &document.key, &document.text)?;
                }
            }
            fed.documents.push(DocumentsSummary {
                source: documents.source.clone(),
                received: report.received,
                inserted: report.inserted,
                duplicates: report.duplicates,
                superseded: report.superseded,
            });
        }
        for extract in &step.extract {
            let report = engine.extract(&ExtractOptions {
                extractor: extract.extractor.clone(),
                source: extract.source.clone(),
                again: extract.again,
                since: None,
                limit: None,
            })?;
            fed.extract.push(ExtractSummary {
                extractor: extract.extractor.clone(),
                documents: report.documents,
                mentions: report.mentions,
                inserted: report.inserted,
                duplicates: report.duplicates,
                superseded: report.superseded,
            });
        }
        Ok(fed)
    }

    /// Runs every step against an engine with an empty store. `after_step`
    /// is called once a step is committed and scored, while the engine still
    /// holds the state the step left.
    pub fn run(
        &self,
        engine: &Engine,
        mut after_step: impl FnMut(&StepReport) -> Result<()>,
    ) -> Result<BenchReport> {
        let manifest = self.check()?;
        engine.apply(&manifest)?;
        // The project's strategies and extractors are beside the manifest's
        // sources.
        if engine.user_code().is_none() {
            engine.set_user_code(UserCode::new(self.project_dir()));
        }

        let mut truth = self.truth()?;
        let mut tracked = Tracked::default();
        let mut steps = Vec::new();
        for (at, step) in self.spec.steps.iter().enumerate() {
            let before = engine.head()?;
            let fed = self.feed(engine, step, &mut truth, &Gone::new())?;
            let mut retract = Vec::new();
            for (index, retraction) in step.retract.iter().enumerate() {
                let summary = self.take_back(engine, retraction).map_err(|error| {
                    Error::Scenario(format!(
                        "step `{}`, retraction {}: {error}",
                        step.name,
                        index + 1
                    ))
                })?;
                retract.push(summary);
            }
            let mut corrections = Vec::new();
            for (index, correction) in step.corrections.iter().enumerate() {
                let summary =
                    correct(engine, &step.name, correction, &mut tracked).map_err(|error| {
                        Error::Scenario(format!(
                            "step `{}`, correction {}: {error}",
                            step.name,
                            index + 1
                        ))
                    })?;
                corrections.push(summary);
            }

            let resolve = self.resolve(engine, step)?;
            let head = engine.head()?;
            let new_records = fed.new_records();
            let (split_recovery, override_persistence) = held(engine, &tracked)?;
            let id_stability = engine.id_stability(before, head)?;
            let churn = engine.churn(before, head, new_records)?;
            let upgrade_stability = step.rematch.then(|| {
                let suggested = |count: fn(&Suggestions) -> usize| {
                    resolve
                        .resolved
                        .iter()
                        .filter_map(|resolved| resolved.suggestions.as_ref())
                        .map(count)
                        .sum::<usize>() as u64
                };
                UpgradeStability::new(
                    self.strategy(step),
                    &id_stability,
                    &churn,
                    suggested(|suggestions| suggestions.merges.len()),
                    suggested(|suggestions| suggestions.splits.len()),
                )
            });
            let report = StepReport {
                name: step.name.clone(),
                strategy: self.strategy(step).to_owned(),
                ingest: fed.ingest,
                documents: fed.documents,
                extract: fed.extract,
                retract,
                corrections,
                resolve,
                head,
                metrics: engine.score(truth_relation(engine, &truth.entities)?)?,
                extraction: extraction(engine, &truth)?,
                end_to_end: end_to_end(engine, &truth)?,
                id_stability,
                churn,
                split_recovery,
                override_persistence,
                upgrade_stability,
                rebuild: match step.rebuild {
                    true => Some(self.rebuild(engine, &manifest, &self.spec.steps[..=at])?),
                    false => None,
                },
                possible_splits: possible_splits(engine, &truth.entities)?,
            };
            engine.verify().map_err(|error| {
                Error::Scenario(format!(
                    "step `{}` left a store that does not verify: {error}",
                    step.name
                ))
            })?;
            after_step(&report)?;
            steps.push(report);
        }

        Ok(BenchReport {
            scenario: self.name.clone(),
            // The engine's: the crates of the workspace share one version.
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            manifest_hash: manifest.hash.clone(),
            steps,
            order_sensitivity: self.order_sensitivity(engine, &manifest)?,
        })
    }

    /// The column a source declares as its key, which is how truth is
    /// joined to records and how a retraction names rows.
    fn key_of(engine: &Engine, source: &str) -> Result<String> {
        engine
            .manifest()
            .and_then(|manifest| manifest.source(source)?.key.clone())
            .ok_or_else(|| {
                Error::Scenario(format!(
                    "source `{source}` needs a sources/{source}.yaml with a `key`, which is how truth is joined to records"
                ))
            })
    }

    /// The rows of a file of the scenario or of a dataset, for which `only`
    /// holds.
    fn rows(
        &self,
        source: &str,
        file: Option<&PathBuf>,
        dataset: Option<&String>,
        member: Option<&str>,
        only: Option<&str>,
    ) -> Result<Rows> {
        let path = match (file, dataset) {
            (Some(file), _) => self.dir.join(file),
            (None, Some(dataset)) => fetch_dataset(&self.dir, dataset, member)?,
            (None, None) => unreachable!("checked when the scenario was loaded"),
        };
        match only {
            None => Ok(Rows::from_csv(&path)?),
            Some(condition) => Rows::from_csv_where(&path, condition).map_err(|error| {
                Error::Scenario(format!(
                    "the rows of `{source}` where `{condition}`: {error}"
                ))
            }),
        }
    }

    /// Ingests one file of a step, and adds the truth it carries. The rows
    /// in `without` are left out.
    fn ingest(
        &self,
        engine: &Engine,
        ingest: &IngestSpec,
        truth: &mut Truth,
        without: &Gone,
    ) -> Result<IngestSummary> {
        let key = Self::key_of(engine, &ingest.source)?;
        let key = key.as_str();
        let mut rows = self.rows(
            &ingest.source,
            ingest.file.as_ref(),
            ingest.dataset.as_ref(),
            ingest.member.as_deref(),
            ingest.only.as_deref(),
        )?;
        if let Some(column) = &self.spec.truth.column {
            let (rest, pairs) = take_column(rows, column, key)?;
            rows = rest;
            for (key, entity) in pairs {
                truth.entities.insert((ingest.source.clone(), key), entity);
            }
        }
        if let Some(keys) = without.get(&(ingest.entity_type.clone(), ingest.source.clone())) {
            rows = without_keys(rows, key, keys)?;
        }
        let report = engine.load(&ingest.source, &ingest.entity_type, rows)?;
        Ok(IngestSummary {
            source: ingest.source.clone(),
            entity_type: ingest.entity_type.clone(),
            received: report.received,
            inserted: report.inserted,
            duplicates: report.duplicates,
            superseded: report.superseded,
            restored: report.restored,
            returned: report.returned,
            possible_splits: report.possible_splits,
        })
    }

    /// Takes back the rows one retraction of a step names.
    fn take_back(&self, engine: &Engine, retract: &RetractSpec) -> Result<RetractSummary> {
        let keys = match &retract.keys {
            Some(keys) => keys.clone(),
            None => {
                let key = Self::key_of(engine, &retract.source)?;
                let rows = self.rows(
                    &retract.source,
                    retract.file.as_ref(),
                    retract.dataset.as_ref(),
                    retract.member.as_deref(),
                    retract.only.as_deref(),
                )?;
                let (_, pairs) = take_column(rows, &key, &key)?;
                pairs.into_iter().map(|(key, _)| key).collect()
            }
        };
        if keys.is_empty() {
            return Err(Error::Scenario(format!(
                "the retraction from `{}` names no rows",
                retract.source
            )));
        }
        let report = engine.retract(
            &retract.source,
            &retract.entity_type,
            &keys,
            Actor::Agent("benchmark".to_owned()),
            None,
        )?;
        Ok(RetractSummary {
            source: retract.source.clone(),
            entity_type: retract.entity_type.clone(),
            received: report.received,
            unknown: report.unknown,
            retracted: report.retracted,
            detached: report.detached,
            possible_splits: report.possible_splits,
        })
    }
}

/// Every file of a directory as a document keyed by its name, in the order
/// of the names.
fn read_documents(dir: &Path) -> Result<Vec<NewDocument>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(Error::io(dir))? {
        let path = entry.map_err(Error::io(dir))?.path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.is_file() && !name.starts_with('.') {
            files.push((name, path));
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|(key, path)| {
            let text = std::fs::read_to_string(&path).map_err(Error::io(&path))?;
            Ok(NewDocument {
                key,
                uri: None,
                text,
            })
        })
        .collect()
}
