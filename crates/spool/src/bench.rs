//! Benchmark scenarios.
//!
//! A scenario is an ordered list of steps with the truth about every
//! record. Each step ingests files, applies the corrections a reviewer
//! would make, and resolves, through the same calls any caller uses.
//! Committed state is then scored against the truth. Nothing is special-cased
//! for the benchmark.
//!
//! A scenario is a `scenario.toml`:
//!
//! ```toml
//! project = "."                 # the project directory, relative to this file
//!
//! [truth]
//! file = "truth.csv"            # columns: source, source_key, true_entity
//! # column = "cluster"          # or: a column of every ingested file
//!
//! [[step]]
//! name = "first batch"
//!
//! [[step.ingest]]
//! file = "batch-1.csv"          # or: dataset = "<name in datasets.toml>"
//! source = "crm"
//! entity_type = "company"
//! key = "id"
//!
//! [[step]]
//! name = "review"               # a step may have corrections and no files
//!
//! [[step.correct]]
//! name = "wrong merge"          # optional: lets a later `undo` name it
//! merge = ["crm:7", "crm:9"]    # the identity of the first into that of the second
//!
//! [[step]]
//! name = "a better model"
//! strategy = "splink"           # the strategy this step resolves with
//! rematch = true                # and it judges every record again
//! ```
//!
//! A step can also store documents and run extractors over them, before it
//! resolves:
//!
//! ```toml
//! [truth]
//! mentions = "mentions.csv"     # columns: source, document, start, end, entity_type, true_entity
//!
//! [[step]]
//! name = "the news"
//!
//! [[step.documents]]
//! dir = "documents"             # every file is a document, and its name is its key
//! source = "news"
//!
//! [[step.extract]]
//! extractor = "names"           # one of the project's extractors
//! ```
//!
//! A labelled mention says that a document mentions an entity at a place:
//! `start` and `end` count Unicode code points, and `end` is the position
//! after the last one. A `text` column, if the file has one, must hold the
//! document's text at that place, which catches a label that is off by
//! one. The labels are the truth twice over. Extraction is scored against
//! them on its own: a mention the extractor found is right when a label has
//! the same document, entity type, start and end. Resolution is scored as it
//! is for rows, over the records that have a true entity, which for an
//! extracted mention is the label at the same place. A mention the
//! extractor made up has no true entity and is not scored there, so the two
//! scores do not leak into each other.
//!
//! A step resolves with its own `strategy`, or else with the scenario's
//! top-level `strategy`, or else with the built-in `rules`. A step with
//! `rematch = true` matches everything again, which is what switching
//! strategy on the same data means, and reports upgrade stability.
//!
//! A correction names records as `source:key`, and is one of:
//!
//! | Field | What it does |
//! | --- | --- |
//! | `merge = [a, b]` | merges the identity of `a` into the identity of `b` |
//! | `assign = [a, b]` | puts `a` into the identity of `b` |
//! | `split = [a, ...]` | splits the records from the rest of their identity, and keeps the sides apart |
//! | `must_link = [a, b]` | records that the two share an identity |
//! | `cannot_link = [a, b]` | records that the two never share an identity |
//! | `undo = "name"` | undoes the named correction |
//!
//! Corrections are applied after the step's files are ingested and before
//! the step resolves, so a step of corrections alone shows what the next run
//! makes of them.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use duckdb::arrow::array::{Array, AsArray};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spool_core::{Actor, Changeset, Constraint, ConstraintKind, IdentityId, Op, Seq};

use spool_core::DocumentId;

use crate::documents::{ExtractOptions, NewDocument, locator};
use crate::engine::{
    ConstraintFilter, Engine, Options, ResolveOptions, ResolveReport, StrategyRef, Suggestions,
};
use crate::error::{Error, Result};
use crate::ingest::IngestOptions;
use crate::lookup::{RecordEntry, RecordFilter};
use crate::metrics::{
    Churn, ExtractionMetrics, IdStability, Metrics, OrderSensitivity, OverridePersistence,
    SplitRecovery, UpgradeStability,
};
use crate::project::Manifest;
use crate::rows::Rows;
use crate::usercode::UserCode;

#[derive(Clone, Debug)]
pub struct Scenario {
    /// The name of the directory that holds `scenario.toml`, or of the file
    /// if it is called something else.
    pub name: String,
    dir: PathBuf,
    spec: Spec,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    #[serde(default = "here")]
    project: PathBuf,
    /// The strategy of every step that does not name its own.
    strategy: Option<String>,
    truth: TruthSpec,
    #[serde(rename = "step")]
    steps: Vec<StepSpec>,
}

fn here() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TruthSpec {
    file: Option<PathBuf>,
    column: Option<String>,
    /// Labelled mentions: the truth about what the documents mention.
    mentions: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepSpec {
    name: String,
    strategy: Option<String>,
    #[serde(default)]
    rematch: bool,
    #[serde(default)]
    ingest: Vec<IngestSpec>,
    #[serde(default)]
    documents: Vec<DocumentsSpec>,
    #[serde(default)]
    extract: Vec<ExtractSpec>,
    #[serde(default, rename = "correct")]
    corrections: Vec<CorrectionSpec>,
}

impl StepSpec {
    /// Whether the step brings anything new into the store.
    fn feeds(&self) -> bool {
        !self.ingest.is_empty() || !self.documents.is_empty() || !self.extract.is_empty()
    }
}

/// A directory of documents: every file in it is one, keyed by its name.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentsSpec {
    dir: PathBuf,
    source: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtractSpec {
    extractor: String,
    source: Option<String>,
    #[serde(default)]
    again: bool,
}

/// One thing a reviewer does. Exactly one of the fields after `reason` is
/// set, which is checked when the scenario is loaded.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorrectionSpec {
    name: Option<String>,
    reason: Option<String>,
    merge: Option<[String; 2]>,
    assign: Option<[String; 2]>,
    split: Option<Vec<String>>,
    must_link: Option<[String; 2]>,
    cannot_link: Option<[String; 2]>,
    undo: Option<String>,
}

enum Correction<'a> {
    Merge(&'a [String; 2]),
    Assign(&'a [String; 2]),
    Split(&'a [String]),
    Constrain(ConstraintKind, &'a [String; 2]),
    Undo(&'a str),
}

impl CorrectionSpec {
    fn kind(&self) -> std::result::Result<Correction<'_>, String> {
        let mut kinds = Vec::new();
        if let Some(pair) = &self.merge {
            kinds.push(Correction::Merge(pair));
        }
        if let Some(pair) = &self.assign {
            kinds.push(Correction::Assign(pair));
        }
        if let Some(records) = &self.split {
            kinds.push(Correction::Split(records));
        }
        if let Some(pair) = &self.must_link {
            kinds.push(Correction::Constrain(ConstraintKind::MustLink, pair));
        }
        if let Some(pair) = &self.cannot_link {
            kinds.push(Correction::Constrain(ConstraintKind::CannotLink, pair));
        }
        if let Some(name) = &self.undo {
            kinds.push(Correction::Undo(name));
        }
        let kind = kinds.pop();
        match kind {
            Some(kind) if kinds.is_empty() => Ok(kind),
            _ => Err(
                "a correction needs exactly one of `merge`, `assign`, `split`, `must_link`, \
                 `cannot_link` and `undo`"
                    .to_owned(),
            ),
        }
    }
}

impl Correction<'_> {
    fn name(&self) -> &'static str {
        match self {
            Correction::Merge(_) => "merge",
            Correction::Assign(_) => "assign",
            Correction::Split(_) => "split",
            Correction::Constrain(ConstraintKind::MustLink, _) => "must_link",
            Correction::Constrain(ConstraintKind::CannotLink, _) => "cannot_link",
            Correction::Undo(_) => "undo",
        }
    }

    fn records(&self) -> &[String] {
        match self {
            Correction::Merge(pair) | Correction::Assign(pair) | Correction::Constrain(_, pair) => {
                &pair[..]
            }
            Correction::Split(records) => records,
            Correction::Undo(_) => &[],
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IngestSpec {
    file: Option<PathBuf>,
    dataset: Option<String>,
    source: String,
    entity_type: String,
    key: Option<String>,
}

/// An entry of `datasets.toml`: a public dataset that is downloaded and
/// checked when a scenario needs it, and never committed.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dataset {
    name: String,
    url: String,
    sha256: String,
    #[allow(dead_code)]
    licence: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Datasets {
    dataset: Vec<Dataset>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchReport {
    pub scenario: String,
    pub engine_version: String,
    pub manifest_hash: String,
    pub steps: Vec<StepReport>,
    /// `None` when fewer than two steps ingest anything, so there is no
    /// other order to try.
    pub order_sensitivity: Option<OrderSensitivity>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepReport {
    pub name: String,
    /// The strategy the step resolved with.
    pub strategy: String,
    pub ingest: Vec<IngestSummary>,
    /// The documents the step stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<DocumentsSummary>,
    /// The extractions the step ran, after its documents were stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract: Vec<ExtractSummary>,
    /// The corrections the step applied, in order, before it resolved.
    pub corrections: Vec<CorrectionSummary>,
    pub resolve: ResolveReport,
    /// The log position after the step.
    pub head: Seq,
    /// Committed state against the truth for everything ingested so far.
    pub metrics: Metrics,
    /// The mentions extracted so far against the labelled ones. Only a
    /// scenario with labelled mentions has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<ExtractionMetrics>,
    /// Of the records that had an identity before the step, how many
    /// kept it.
    pub id_stability: IdStability,
    pub churn: Churn,
    /// Of the splits the scenario has made so far, how many still hold.
    pub split_recovery: SplitRecovery,
    /// Of the constraints the scenario has recorded so far, how many are
    /// still honoured.
    pub override_persistence: OverridePersistence,
    /// What matching everything again did to existing identities. Only a
    /// step with `rematch = true` has it.
    pub upgrade_stability: Option<UpgradeStability>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestSummary {
    pub source: String,
    pub entity_type: String,
    pub received: u64,
    pub inserted: u64,
    pub duplicates: u64,
    /// Rows that replaced an earlier version of the same key.
    pub superseded: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentsSummary {
    pub source: String,
    pub received: u64,
    pub inserted: u64,
    pub duplicates: u64,
    /// Documents that replaced an earlier version of the same key.
    pub superseded: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractSummary {
    pub extractor: String,
    /// Documents the extractor read.
    pub documents: u64,
    pub mentions: u64,
    /// Records created.
    pub inserted: u64,
    pub duplicates: u64,
    pub superseded: u64,
}

/// What a scenario knows to be true about what it has fed the engine.
#[derive(Default)]
struct Truth {
    /// `(source, key)` to true entity, for the records there may be so
    /// far.
    entities: HashMap<(String, String), String>,
    /// The labelled mentions of each document, by `(source, document key)`.
    labels: HashMap<(String, String), Vec<Label>>,
    /// Whether the scenario has labelled mentions at all.
    labelled: bool,
    /// `(source, locator)` of the labelled mentions in the documents stored
    /// so far.
    gold: HashSet<(String, String)>,
}

/// One labelled mention.
struct Label {
    entity_type: String,
    start: u64,
    end: u64,
    /// The document's text at that place, if the label says.
    text: Option<String>,
    entity: String,
}

impl Truth {
    /// Takes note of a document that was stored: its labelled mentions are
    /// now mentions an extractor should find, each at a known key.
    fn add_document(&mut self, source: &str, key: &str, text: &str) -> Result<()> {
        let Some(labels) = self.labels.get(&(source.to_owned(), key.to_owned())) else {
            return Ok(());
        };
        let id = DocumentId::derive(source, key, text);
        let characters: Vec<char> = text.chars().collect();
        for label in labels {
            let invalid = |message: String| {
                Error::Scenario(format!(
                    "the labelled mention of `{}` at {}-{} in document `{key}` of `{source}`: {message}",
                    label.entity, label.start, label.end
                ))
            };
            let (start, end) = (label.start as usize, label.end as usize);
            if start >= end || end > characters.len() {
                return Err(invalid(format!(
                    "the document is {} long",
                    characters.len()
                )));
            }
            let there: String = characters[start..end].iter().collect();
            if let Some(expected) = &label.text
                && *expected != there
            {
                return Err(invalid(format!(
                    "the label says the text there is `{expected}`, and it is `{there}`"
                )));
            }
            let key = locator(id, &label.entity_type, label.start, label.end);
            self.gold.insert((source.to_owned(), key.clone()));
            self.entities
                .insert((source.to_owned(), key), label.entity.clone());
        }
        Ok(())
    }
}

/// What feeding the engine one step's files, documents and extractions
/// reported.
struct Fed {
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectionSummary {
    pub name: Option<String>,
    /// `merge`, `assign`, `split`, `must_link`, `cannot_link` or `undo`.
    pub kind: String,
    /// The log position of the changeset it committed.
    pub seq: Seq,
}

/// What the corrections so far have done that later steps are measured
/// against.
#[derive(Default)]
struct Tracked {
    /// Named corrections: where each was committed, and the split it made.
    named: HashMap<String, (Seq, Option<usize>)>,
    /// The two sides of each split, as record IDs. `None` once undone.
    splits: Vec<Option<[Vec<String>; 2]>>,
    constraints: BTreeSet<Constraint>,
}

impl Tracked {
    /// Follows the constraints a committed changeset recorded and removed.
    fn follow(&mut self, changeset: &Changeset) {
        for op in &changeset.ops {
            match op {
                Op::Constrain { kind, a, b } => {
                    self.constraints.insert(Constraint::new(*kind, *a, *b));
                }
                Op::Unconstrain { kind, a, b } => {
                    self.constraints.remove(&Constraint::new(*kind, *a, *b));
                }
                _ => {}
            }
        }
    }
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
            for ingest in &step.ingest {
                if ingest.file.is_some() == ingest.dataset.is_some() {
                    return Err(invalid(format!(
                        "step `{}`: an ingest needs exactly one of `file` and `dataset`",
                        step.name
                    )));
                }
                if ingest.key.is_none() {
                    return Err(invalid(format!(
                        "step `{}`: an ingest needs a `key` column, which is how truth is joined to records",
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
    fn strategy<'a>(&'a self, step: &'a StepSpec) -> &'a str {
        step.strategy
            .as_deref()
            .or(self.spec.strategy.as_deref())
            .unwrap_or(StrategyRef::RULES)
    }

    fn resolve(&self, engine: &Engine, step: &StepSpec) -> Result<ResolveReport> {
        engine.resolve(&ResolveOptions {
            strategy: Some(self.strategy(step).to_owned()),
            rematch: step.rematch,
            ..ResolveOptions::default()
        })
    }

    /// The truth the scenario's files hold before anything is ingested.
    fn truth(&self) -> Result<Truth> {
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
    /// then its extractions.
    fn feed(&self, engine: &Engine, step: &StepSpec, truth: &mut Truth) -> Result<Fed> {
        let mut fed = Fed {
            ingest: Vec::new(),
            documents: Vec::new(),
            extract: Vec::new(),
        };
        for ingest in &step.ingest {
            fed.ingest.push(self.ingest(engine, ingest, truth)?);
        }
        for documents in &step.documents {
            let read = read_documents(&self.dir.join(&documents.dir))?;
            for document in &read {
                truth.add_document(&documents.source, &document.key, &document.text)?;
            }
            let report = engine.add_documents(&documents.source, &read)?;
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
        let manifest = Manifest::load(self.project_dir())?;
        engine.apply(&manifest)?;
        // The project's strategies and extractors are beside the manifest's
        // sources.
        if engine.user_code().is_none() {
            engine.set_user_code(UserCode::new(self.project_dir()));
        }

        let mut truth = self.truth()?;
        let mut tracked = Tracked::default();
        let mut steps = Vec::new();
        for step in &self.spec.steps {
            let before = engine.head()?;
            let fed = self.feed(engine, step, &mut truth)?;
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
                corrections,
                resolve,
                head,
                metrics: engine.score(truth_relation(engine, &truth.entities)?)?,
                extraction: extraction(engine, &truth)?,
                id_stability,
                churn,
                split_recovery,
                override_persistence,
                upgrade_stability,
            };
            after_step(&report)?;
            steps.push(report);
        }

        engine.verify()?;
        Ok(BenchReport {
            scenario: self.name.clone(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            manifest_hash: manifest.hash.clone(),
            steps,
            order_sensitivity: self.order_sensitivity(engine, &manifest)?,
        })
    }

    /// Ingests one file of a step, and adds the truth it carries.
    fn ingest(
        &self,
        engine: &Engine,
        ingest: &IngestSpec,
        truth: &mut Truth,
    ) -> Result<IngestSummary> {
        let path = match (&ingest.file, &ingest.dataset) {
            (Some(file), _) => self.dir.join(file),
            (None, Some(dataset)) => fetch_dataset(&self.dir, dataset)?,
            (None, None) => unreachable!("checked when the scenario was loaded"),
        };
        let key = ingest.key.as_deref().unwrap_or_default();
        let mut rows = Rows::from_csv(&path)?;
        if let Some(column) = &self.spec.truth.column {
            let (rest, pairs) = rows.take_column(column, key)?;
            rows = rest;
            for (key, entity) in pairs {
                truth.entities.insert((ingest.source.clone(), key), entity);
            }
        }
        let report = engine.ingest(
            &ingest.source,
            &ingest.entity_type,
            rows,
            &IngestOptions::keyed_by(key),
        )?;
        Ok(IngestSummary {
            source: ingest.source.clone(),
            entity_type: ingest.entity_type.clone(),
            received: report.received,
            inserted: report.inserted,
            duplicates: report.duplicates,
            superseded: report.superseded,
        })
    }

    /// Runs the steps that ingest anything in reverse, without corrections,
    /// and compares the grouping that leaves with the one the written order
    /// leaves. `engine` holds the scenario as it was just run.
    fn order_sensitivity(
        &self,
        engine: &Engine,
        manifest: &Manifest,
    ) -> Result<Option<OrderSensitivity>> {
        let batches: Vec<&StepSpec> = self.spec.steps.iter().filter(|step| step.feeds()).collect();
        if batches.len() < 2 {
            return Ok(None);
        }
        let replay = |steps: &mut dyn Iterator<Item = &&StepSpec>| -> Result<_> {
            let replayed = Engine::open_in_memory(Options::seeded(0))?;
            replayed.apply(manifest)?;
            if let Some(user_code) = engine.user_code() {
                replayed.set_user_code(user_code.as_ref().clone());
            }
            let mut truth = self.truth()?;
            for step in steps {
                self.feed(&replayed, step, &mut truth)?;
                // The batch with the strategy it was written with, and no
                // rematch: this measures the order of arrival alone.
                replayed.resolve(&ResolveOptions::with_strategy(self.strategy(step)))?;
            }
            grouping(&replayed)
        };
        // The engine holds the written order as it is unless the scenario
        // did more than ingest and resolve.
        let corrected = self
            .spec
            .steps
            .iter()
            .any(|step| !step.corrections.is_empty() || step.rematch);
        let written = if corrected {
            replay(&mut batches.iter())?
        } else {
            grouping(engine)?
        };
        let reversed = replay(&mut batches.iter().rev())?;
        Ok(Some(OrderSensitivity::between(&written, &reversed)))
    }
}

/// Every record with the group it is in: its identity, or itself when
/// it has none.
fn grouping(engine: &Engine) -> Result<Vec<(String, String)>> {
    let batches = engine.query(
        "SELECT o.record_id::VARCHAR,
                coalesce(m.identity_id::VARCHAR, 'unassigned:' || o.record_id::VARCHAR)
         FROM spool.record o
         LEFT JOIN spool.membership m ON m.record_id = o.record_id AND m.to_seq IS NULL
         ORDER BY o.record_id",
    )?;
    let mut groups = Vec::new();
    for batch in &batches {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (records, identities) = (column(0), column(1));
        for row in 0..batch.num_rows() {
            groups.push((
                records.value(row).to_owned(),
                identities.value(row).to_owned(),
            ));
        }
    }
    Ok(groups)
}

/// The version of a row the source holds now, named as `source:key`.
fn record(engine: &Engine, reference: &str) -> Result<RecordEntry> {
    let (source, key) = reference
        .split_once(':')
        .expect("checked when the scenario was loaded");
    engine
        .records(&RecordFilter::keyed(source, key))?
        .into_iter()
        .find(|version| version.superseded_by.is_none())
        .ok_or_else(|| Error::Scenario(format!("no record is `{reference}`")))
}

fn identity_of(engine: &Engine, reference: &str) -> Result<IdentityId> {
    record(engine, reference)?
        .identity
        .ok_or_else(|| Error::Scenario(format!("`{reference}` has no identity")))
}

fn members(engine: &Engine, identity: IdentityId) -> Result<Vec<String>> {
    Ok(engine
        .identity(identity)?
        .members
        .iter()
        .map(|member| member.record.to_string())
        .collect())
}

/// Applies one correction as a reviewer would: through the calls any caller
/// uses, as the user `benchmark`.
fn correct(
    engine: &Engine,
    step: &str,
    spec: &CorrectionSpec,
    tracked: &mut Tracked,
) -> Result<CorrectionSummary> {
    let actor = Actor::User("benchmark".to_owned());
    let reason = spec
        .reason
        .clone()
        .or_else(|| spec.name.clone())
        .or_else(|| Some(step.to_owned()));
    let kind = spec.kind().expect("checked when the scenario was loaded");
    let author = |ops: Vec<Op>| engine.author(actor.clone(), reason.clone(), ops);

    let mut split = None;
    let (changeset, seq) = match &kind {
        Correction::Merge([from, into]) => {
            let authored = author(vec![Op::Merge {
                from: identity_of(engine, from)?,
                into: identity_of(engine, into)?,
            }])?;
            (authored.changeset, authored.committed.map(|c| c.seq))
        }
        Correction::Assign([moved, target]) => {
            let moved = record(engine, moved)?;
            let authored = author(vec![Op::Assign {
                record: moved.id,
                from: moved.identity,
                to: Some(identity_of(engine, target)?),
            }])?;
            (authored.changeset, authored.committed.map(|c| c.seq))
        }
        Correction::Split(records) => {
            let identity = identity_of(engine, &records[0])?;
            let records = records
                .iter()
                .map(|reference| record(engine, reference).map(|found| found.id))
                .collect::<Result<Vec<_>>>()?;
            let report = engine.split(
                identity,
                &records,
                true,
                None,
                actor.clone(),
                reason.clone(),
            )?;
            tracked.splits.push(Some([
                members(engine, report.kept)?,
                members(engine, report.identity)?,
            ]));
            split = Some(tracked.splits.len() - 1);
            (report.changeset, Some(report.committed.seq))
        }
        Correction::Constrain(kind, [a, b]) => {
            let authored = author(vec![Op::Constrain {
                kind: *kind,
                a: record(engine, a)?.id,
                b: record(engine, b)?.id,
            }])?;
            (authored.changeset, authored.committed.map(|c| c.seq))
        }
        Correction::Undo(name) => {
            let (seq, split) = tracked.named[*name];
            let authored = engine.undo(seq, actor.clone(), reason.clone(), true)?;
            if let Some(split) = split {
                tracked.splits[split] = None;
            }
            (authored.changeset, authored.committed.map(|c| c.seq))
        }
    };
    let seq = seq.expect("the changeset was committed");
    tracked.follow(&changeset);
    if let Some(name) = &spec.name {
        tracked.named.insert(name.clone(), (seq, split));
    }
    Ok(CorrectionSummary {
        name: spec.name.clone(),
        kind: kind.name().to_owned(),
        seq,
    })
}

/// Measures what the scenario's corrections asked for against current state.
fn held(engine: &Engine, tracked: &Tracked) -> Result<(SplitRecovery, OverridePersistence)> {
    let splits = tracked.splits.iter().flatten().count() as u64;
    let constraints = tracked.constraints.len() as u64;
    if splits == 0 && constraints == 0 {
        return Ok((SplitRecovery::new(0, 0), OverridePersistence::new(0, 0)));
    }
    let groups: HashMap<String, String> = grouping(engine)?.into_iter().collect();
    let group = |record: &String| groups.get(record);

    let kept_apart = tracked
        .splits
        .iter()
        .flatten()
        .filter(|[kept, moved]| {
            let kept: HashSet<_> = kept.iter().filter_map(group).collect();
            !moved
                .iter()
                .filter_map(group)
                .any(|group| kept.contains(group))
        })
        .count() as u64;

    let in_force: HashSet<Constraint> = engine
        .constraints(&ConstraintFilter::default())?
        .into_iter()
        .map(|record| record.constraint)
        .collect();
    let honoured = tracked
        .constraints
        .iter()
        .filter(|constraint| {
            let together = group(&constraint.a.to_string()) == group(&constraint.b.to_string());
            in_force.contains(constraint)
                && match constraint.kind {
                    ConstraintKind::MustLink => together,
                    ConstraintKind::CannotLink => !together,
                }
        })
        .count() as u64;

    Ok((
        SplitRecovery::new(splits, kept_apart),
        OverridePersistence::new(constraints, honoured),
    ))
}

/// The truth relation `(record_id, true_entity)` for the records
/// the store holds, joined to the truth by source and key.
fn truth_relation(engine: &Engine, truth: &HashMap<(String, String), String>) -> Result<Rows> {
    let records = engine.query(
        "SELECT record_id::VARCHAR, source_id, source_key
         FROM spool.record WHERE source_key IS NOT NULL",
    )?;
    let mut rows: Vec<(String, &str)> = Vec::new();
    for batch in &records {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (ids, sources, keys) = (column(0), column(1), column(2));
        for row in 0..batch.num_rows() {
            let key = (sources.value(row).to_owned(), keys.value(row).to_owned());
            if let Some(entity) = truth.get(&key) {
                rows.push((ids.value(row).to_owned(), entity));
            }
        }
    }
    let rows: Vec<Vec<Option<&str>>> = rows
        .iter()
        .map(|(id, entity)| vec![Some(id.as_str()), Some(*entity)])
        .collect();
    Rows::from_text(&["record_id", "true_entity"], &rows)
}

/// How the mentions extracted so far compare with the labelled mentions of
/// the documents stored so far. A mention is one place in one document,
/// however many versions of the record there are.
fn extraction(engine: &Engine, truth: &Truth) -> Result<Option<ExtractionMetrics>> {
    if !truth.labelled {
        return Ok(None);
    }
    let found = engine.query(
        "SELECT DISTINCT source_id, source_key FROM spool.record
         WHERE json_extract_string(evidence, '$.document') IS NOT NULL",
    )?;
    let (mut extracted, mut correct) = (0, 0);
    for batch in &found {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (sources, keys) = (column(0), column(1));
        for row in 0..batch.num_rows() {
            extracted += 1;
            let key = (sources.value(row).to_owned(), keys.value(row).to_owned());
            if truth.gold.contains(&key) {
                correct += 1;
            }
        }
    }
    Ok(Some(ExtractionMetrics::new(
        truth.gold.len() as u64,
        extracted,
        correct,
    )))
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

/// Reads labelled mentions: each with the `(source, document key)` it is in.
fn read_labels(path: &Path) -> Result<Vec<((String, String), Label)>> {
    const COLUMNS: [&str; 6] = [
        "source",
        "document",
        "start",
        "end",
        "entity_type",
        "true_entity",
    ];
    let invalid = |message: String| Error::Scenario(format!("{}: {message}", path.display()));
    let rows = Rows::from_csv(path)?;
    let schema = duckdb::arrow::record_batch::RecordBatchReader::schema(&rows);
    let mut at = Vec::new();
    for name in COLUMNS {
        at.push(schema.index_of(name).map_err(|_| {
            invalid(
                "a file of labelled mentions needs the columns `source`, `document`, `start`, \
                 `end`, `entity_type` and `true_entity`"
                    .to_owned(),
            )
        })?);
    }
    let text_at = schema.index_of("text").ok();
    let mut labels = Vec::new();
    let mut line = 1;
    for batch in rows {
        let batch = batch?;
        let column = |index: usize| batch.column(index).as_string::<i32>();
        for row in 0..batch.num_rows() {
            line += 1;
            let field = |index: usize| -> Result<String> {
                let values = column(at[index]);
                if values.is_null(row) || values.value(row).is_empty() {
                    return Err(invalid(format!("line {line} has no `{}`", COLUMNS[index])));
                }
                Ok(values.value(row).to_owned())
            };
            let position = |index: usize| -> Result<u64> {
                field(index)?.parse().map_err(|_| {
                    invalid(format!(
                        "line {line}: `{}` is not a position",
                        COLUMNS[index]
                    ))
                })
            };
            let text = text_at
                .map(column)
                .filter(|values| !values.is_null(row))
                .map(|values| values.value(row).to_owned());
            labels.push((
                (field(0)?, field(1)?),
                Label {
                    start: position(2)?,
                    end: position(3)?,
                    entity_type: field(4)?,
                    entity: field(5)?,
                    text,
                },
            ));
        }
    }
    Ok(labels)
}

fn read_truth_file(path: &Path) -> Result<Vec<(String, String, String)>> {
    let invalid = |message: &str| Error::Scenario(format!("{}: {message}", path.display()));
    let rows = Rows::from_csv(path)?;
    let schema = duckdb::arrow::record_batch::RecordBatchReader::schema(&rows);
    let mut at = Vec::new();
    for name in ["source", "source_key", "true_entity"] {
        at.push(schema.index_of(name).map_err(|_| {
            invalid("a truth file needs the columns `source`, `source_key` and `true_entity`")
        })?);
    }
    let mut truth = Vec::new();
    for batch in rows {
        let batch = batch?;
        let column = |index: usize| batch.column(at[index]).as_string::<i32>();
        let (sources, keys, entities) = (column(0), column(1), column(2));
        for row in 0..batch.num_rows() {
            if sources.is_null(row) || keys.is_null(row) || entities.is_null(row) {
                return Err(invalid("a row has an empty field"));
            }
            truth.push((
                sources.value(row).to_owned(),
                keys.value(row).to_owned(),
                entities.value(row).to_owned(),
            ));
        }
    }
    Ok(truth)
}

/// Returns the local copy of a dataset named in the nearest `datasets.toml`
/// above the scenario, downloading it if the copy is missing or does not
/// match its checksum. Copies are kept in `.cache` beside `datasets.toml`.
fn fetch_dataset(scenario_dir: &Path, name: &str) -> Result<PathBuf> {
    let start = std::fs::canonicalize(scenario_dir).map_err(Error::io(scenario_dir))?;
    let index = start
        .ancestors()
        .map(|dir| dir.join("datasets.toml"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            Error::Scenario(format!(
                "dataset `{name}`: no datasets.toml in {} or above it",
                start.display()
            ))
        })?;
    let text = std::fs::read_to_string(&index).map_err(Error::io(&index))?;
    let datasets: Datasets = toml::from_str(&text)
        .map_err(|error| Error::Scenario(format!("{}: {error}", index.display())))?;
    let dataset = datasets
        .dataset
        .into_iter()
        .find(|dataset| dataset.name == name)
        .ok_or_else(|| {
            Error::Scenario(format!("{}: no dataset named `{name}`", index.display()))
        })?;

    let cache = index.with_file_name(".cache");
    let path = cache.join(format!("{name}.csv"));
    if path.is_file() && sha256_of(&path)? == dataset.sha256 {
        return Ok(path);
    }

    std::fs::create_dir_all(&cache).map_err(Error::io(&cache))?;
    let partial = cache.join(format!("{name}.csv.part"));
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(&partial)
        .arg(&dataset.url)
        .status()
        .map_err(|error| {
            Error::Scenario(format!(
                "dataset `{name}`: curl is needed to download it and could not be run: {error}"
            ))
        })?;
    if !status.success() {
        return Err(Error::Scenario(format!(
            "dataset `{name}`: downloading {} failed",
            dataset.url
        )));
    }
    let found = sha256_of(&partial)?;
    if found != dataset.sha256 {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::Scenario(format!(
            "dataset `{name}`: the download has SHA-256 {found}, and datasets.toml expects {}",
            dataset.sha256
        )));
    }
    std::fs::rename(&partial, &path).map_err(Error::io(&path))?;
    Ok(path)
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(Error::io(path))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(Error::io(path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
