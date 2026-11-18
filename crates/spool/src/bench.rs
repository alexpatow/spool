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
//! # member = "dir/part-1.csv"   # the file to read, when the dataset is an archive
//! # where = "region = 'north'"  # only the rows this holds for: SQL over the file's columns
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
//! Every column of a file is text, so a `where` that compares numbers casts
//! them: `id::BIGINT % 10 = 0` takes a tenth of the rows, and the same tenth
//! of every file that shares the IDs. The truth column may be the column the
//! source declares as its key, when the key is what says who a row is; any
//! other truth column is taken out of the rows before they are ingested.
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
//! A step can take rows back, as a source does that no longer holds them:
//!
//! ```toml
//! [[step]]
//! name = "struck off"
//! rebuild = true                # compare what is left with resolving it afresh
//!
//! [[step.retract]]
//! source = "registry"
//! entity_type = "company"
//! keys = ["X0000017"]           # the keys of the rows
//!
//! [[step.retract]]
//! source = "crm"
//! entity_type = "company"
//! file = "closed-accounts.csv"  # or: the rows of a file or a `dataset`,
//! where = "closed = 'yes'"      # read as an ingest reads them, for their keys
//! ```
//!
//! Rows are taken back after the step's files are ingested and before its
//! corrections. From then on the scenario is scored on the rows the sources
//! still hold. A step with `rebuild = true` also runs the steps so far in a
//! store of its own, without the rows that are taken back as things stand
//! and without the corrections, and reports how its identities differ from
//! those of the store that held the rows and lost them.
//!
//! The store is verified after every step, and the scenario fails at the
//! first step that leaves it inconsistent.
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
use std::sync::atomic::{AtomicU64, Ordering};

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
use crate::lookup::{RecordEntry, RecordFilter};
use crate::metrics::{
    Churn, ExtractionMetrics, IdStability, Metrics, OrderSensitivity, OverridePersistence, Rebuild,
    SplitRecovery, UpgradeStability,
};
use crate::project::Manifest;
use crate::retract::NOT_RETRACTED;
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
    #[serde(default)]
    retract: Vec<RetractSpec>,
    #[serde(default, rename = "correct")]
    corrections: Vec<CorrectionSpec>,
    /// Whether what the step leaves is compared with resolving the rows
    /// the sources still hold in a store of their own.
    #[serde(default)]
    rebuild: bool,
}

impl StepSpec {
    /// Whether the step brings anything new into the store.
    fn feeds(&self) -> bool {
        !self.ingest.is_empty() || !self.documents.is_empty() || !self.extract.is_empty()
    }
}

/// Rows a source takes back: their keys, or the rows of a file, which are
/// read as an ingest reads them and name the rows by the source's key
/// column.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetractSpec {
    keys: Option<Vec<String>>,
    file: Option<PathBuf>,
    dataset: Option<String>,
    /// The file to read within a dataset that is an archive.
    member: Option<String>,
    /// A SQL condition on the file's columns. Only the rows it holds for
    /// are taken back.
    #[serde(rename = "where")]
    only: Option<String>,
    source: String,
    entity_type: String,
}

impl RetractSpec {
    fn check(&self) -> std::result::Result<(), String> {
        let named = [
            self.keys.is_some(),
            self.file.is_some(),
            self.dataset.is_some(),
        ];
        if named.iter().filter(|named| **named).count() != 1 {
            return Err(
                "a retraction needs exactly one of `keys`, `file` and `dataset`".to_owned(),
            );
        }
        if self.member.is_some() && self.dataset.is_none() {
            return Err(
                "`member` names a file within a `dataset`, and the retraction has none".to_owned(),
            );
        }
        if self.only.is_some() && self.keys.is_some() {
            return Err(
                "`where` chooses among the rows of a `file` or a `dataset`, and the retraction \
                 names `keys`"
                    .to_owned(),
            );
        }
        Ok(())
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
    /// The file to read within a dataset that is an archive.
    member: Option<String>,
    /// A SQL condition on the file's columns. Only the rows it holds for
    /// are ingested.
    #[serde(rename = "where")]
    only: Option<String>,
    source: String,
    entity_type: String,
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
    /// `tar` when the download is a tar archive, compressed or not, of
    /// which an ingest names one file. Without it the download is the file.
    archive: Option<Archive>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Archive {
    Tar,
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
    /// The rows the step took back, after its files were ingested.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retract: Vec<RetractSummary>,
    /// The corrections the step applied, in order, before it resolved.
    pub corrections: Vec<CorrectionSummary>,
    pub resolve: ResolveReport,
    /// The log position after the step.
    pub head: Seq,
    /// Committed state against the truth for everything ingested so far
    /// that its source has not taken back.
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
    /// What the step left against the rows the sources still hold resolved
    /// in a store of their own. Only a step with `rebuild = true` has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<Rebuild>,
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
    /// Records that came back: their source had taken them back, and sent
    /// a row under their key again.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub restored: u64,
    /// Those of them that went back to the identity they left.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub returned: u64,
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// Rows one source took back in a step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetractSummary {
    pub source: String,
    pub entity_type: String,
    /// Keys that were named.
    pub received: u64,
    /// Those the source had no row under.
    pub unknown: u64,
    /// Records taken back: every version of each row.
    pub retracted: u64,
    /// Those that had an identity, and left it.
    pub detached: u64,
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
    /// then its extractions. The rows in `without` are left out of the
    /// files.
    fn feed(
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
                id_stability,
                churn,
                split_recovery,
                override_persistence,
                upgrade_stability,
                rebuild: match step.rebuild {
                    true => Some(self.rebuild(engine, &manifest, &self.spec.steps[..=at])?),
                    false => None,
                },
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
            None => Rows::from_csv(&path),
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
            let (rest, pairs) = rows.take_column(column, key)?;
            rows = rest;
            for (key, entity) in pairs {
                truth.entities.insert((ingest.source.clone(), key), entity);
            }
        }
        if let Some(keys) = without.get(&(ingest.entity_type.clone(), ingest.source.clone())) {
            rows = rows.without(key, keys)?;
        }
        let report = engine.ingest(&ingest.source, &ingest.entity_type, rows)?;
        Ok(IngestSummary {
            source: ingest.source.clone(),
            entity_type: ingest.entity_type.clone(),
            received: report.received,
            inserted: report.inserted,
            duplicates: report.duplicates,
            superseded: report.superseded,
            restored: report.restored,
            returned: report.returned,
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
                let (_, pairs) = rows.take_column(&key, &key)?;
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
        })
    }

    /// Runs steps in a store of their own, without the rows in `without`
    /// and without corrections: each batch with the strategy it was written
    /// with, and no rematch. The scenario's own engine says how user code
    /// is run.
    fn replay<'a>(
        &self,
        engine: &Engine,
        manifest: &Manifest,
        steps: impl Iterator<Item = &'a StepSpec>,
        without: &Gone,
    ) -> Result<(Engine, Truth)> {
        let replayed = Engine::open_in_memory(Options::seeded(0))?;
        replayed.apply(manifest)?;
        if let Some(user_code) = engine.user_code() {
            replayed.set_user_code(user_code.as_ref().clone());
        }
        let mut truth = self.truth()?;
        for step in steps {
            self.feed(&replayed, step, &mut truth, without)?;
            replayed.resolve(&ResolveOptions::with_strategy(self.strategy(step)))?;
        }
        Ok((replayed, truth))
    }

    /// Compares what `engine` holds after `steps` with the same steps run
    /// in a store that is never sent the rows the sources have taken back.
    fn rebuild(&self, engine: &Engine, manifest: &Manifest, steps: &[StepSpec]) -> Result<Rebuild> {
        let gone = gone(engine)?;
        let batches = steps.iter().filter(|step| step.feeds());
        let (rebuilt, truth) = self.replay(engine, manifest, batches, &gone)?;
        let (kept, afresh) = (grouping(engine)?, grouping(&rebuilt)?);
        // Both are in the order of their record IDs, which say what a
        // record holds: the two stores must hold the same rows.
        let same = kept.len() == afresh.len()
            && kept
                .iter()
                .zip(&afresh)
                .all(|((one, _), (other, _))| one == other);
        if !same {
            return Err(Error::Scenario(format!(
                "the store holds {} records its sources have not taken back, and the same steps \
                 without the rows that were taken back give {} records, or others",
                kept.len(),
                afresh.len()
            )));
        }
        let metrics = rebuilt.score(truth_relation(&rebuilt, &truth.entities)?)?;
        Ok(Rebuild::between(&kept, &afresh, metrics))
    }

    /// Runs the steps that ingest anything in reverse, without corrections
    /// and without the rows that are taken back at the end, and compares
    /// the grouping that leaves with the one the written order leaves.
    /// `engine` holds the scenario as it was just run.
    fn order_sensitivity(
        &self,
        engine: &Engine,
        manifest: &Manifest,
    ) -> Result<Option<OrderSensitivity>> {
        let batches: Vec<&StepSpec> = self.spec.steps.iter().filter(|step| step.feeds()).collect();
        if batches.len() < 2 {
            return Ok(None);
        }
        let gone = gone(engine)?;
        // This measures the order of arrival alone.
        let replay = |steps: &mut dyn Iterator<Item = &StepSpec>| -> Result<_> {
            let (replayed, _) = self.replay(engine, manifest, steps, &gone)?;
            grouping(&replayed)
        };
        // The engine holds the written order as it is unless the scenario
        // did more than ingest and resolve.
        let corrected =
            self.spec.steps.iter().any(|step| {
                !step.corrections.is_empty() || step.rematch || !step.retract.is_empty()
            });
        let written = if corrected {
            replay(&mut batches.iter().copied())?
        } else {
            grouping(engine)?
        };
        let reversed = replay(&mut batches.iter().rev().copied())?;
        Ok(Some(OrderSensitivity::between(&written, &reversed)))
    }
}

/// The rows the sources have taken back as things stand: their keys, by
/// entity type and source.
type Gone = HashMap<(String, String), HashSet<String>>;

fn gone(engine: &Engine) -> Result<Gone> {
    let batches = engine.query(&format!(
        "SELECT DISTINCT entity_type, source_id, source_key FROM spool.record
         WHERE source_key IS NOT NULL AND NOT ({NOT_RETRACTED})"
    ))?;
    let mut gone = Gone::new();
    for batch in &batches {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (types, sources, keys) = (column(0), column(1), column(2));
        for row in 0..batch.num_rows() {
            gone.entry((types.value(row).to_owned(), sources.value(row).to_owned()))
                .or_default()
                .insert(keys.value(row).to_owned());
        }
    }
    Ok(gone)
}

/// Every record its source has not taken back, with the group it is in:
/// its identity, or itself when it has none.
fn grouping(engine: &Engine) -> Result<Vec<(String, String)>> {
    let batches = engine.query(&format!(
        "SELECT o.record_id::VARCHAR,
                coalesce(m.identity_id::VARCHAR, 'unassigned:' || o.record_id::VARCHAR)
         FROM (SELECT record_id FROM spool.record WHERE {NOT_RETRACTED}) o
         LEFT JOIN spool.membership m ON m.record_id = o.record_id AND m.to_seq IS NULL
         ORDER BY o.record_id"
    ))?;
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
    // A record its source took back is no longer something to be right
    // or wrong about.
    let records = engine.query(&format!(
        "SELECT record_id::VARCHAR, source_id, source_key
         FROM spool.record WHERE source_key IS NOT NULL AND {NOT_RETRACTED}"
    ))?;
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
///
/// A dataset that is an archive is unpacked beside its download, once, and
/// `member` names the file in it that is wanted.
fn fetch_dataset(scenario_dir: &Path, name: &str, member: Option<&str>) -> Result<PathBuf> {
    let (index, dataset) = find_dataset(scenario_dir, name, member)?;
    let cache = index.with_file_name(".cache");
    let Some(member) = member else {
        let path = cache.join(format!("{name}.csv"));
        if !(path.is_file() && sha256_of(&path)? == dataset.sha256) {
            download(&cache, name, &dataset, &path)?;
        }
        return Ok(path);
    };
    unpacked(&cache, name, &dataset, member)
}

/// Finds a dataset in the `datasets.toml` nearest above the scenario, and
/// that file. An ingest names a `member` of an archive, and of nothing else.
fn find_dataset(
    scenario_dir: &Path,
    name: &str,
    member: Option<&str>,
) -> Result<(PathBuf, Dataset)> {
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
    match (dataset.archive, member) {
        (None, None) | (Some(Archive::Tar), Some(_)) => Ok((index, dataset)),
        (None, Some(_)) => Err(Error::Scenario(format!(
            "dataset `{name}` is one file, and the ingest names a `member` of it"
        ))),
        (Some(Archive::Tar), None) => Err(Error::Scenario(format!(
            "dataset `{name}` is an archive: the ingest names the file to read with `member`"
        ))),
    }
}

/// A path in the cache that no other run uses: two runs that fetch the same
/// dataset at once each write their own file, and the one that is whole
/// takes the dataset's name.
fn scratch(cache: &Path, name: &str, kind: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    cache.join(format!(
        "{name}.{}-{}.{kind}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Downloads a dataset to `path`, which is kept only if it has the checksum
/// `datasets.toml` names.
fn download(cache: &Path, name: &str, dataset: &Dataset, path: &Path) -> Result<()> {
    std::fs::create_dir_all(cache).map_err(Error::io(cache))?;
    let partial = scratch(cache, name, "part");
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
        let _ = std::fs::remove_file(&partial);
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
    std::fs::rename(&partial, path).map_err(Error::io(path))
}

/// The file `member` of a dataset that is an archive. The archive is
/// downloaded, checked and unpacked into a directory named for the dataset,
/// and only the directory is kept. It records the checksum of the archive it
/// came from, so a dataset whose checksum changed is fetched again.
fn unpacked(cache: &Path, name: &str, dataset: &Dataset, member: &str) -> Result<PathBuf> {
    let inside = Path::new(member);
    let plain = inside
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)));
    if !plain || member.is_empty() {
        return Err(Error::Scenario(format!(
            "dataset `{name}`: `{member}` does not name a file within the archive"
        )));
    }
    let dir = cache.join(name);
    let current = |dir: &Path| {
        std::fs::read_to_string(dir.join(".sha256")).is_ok_and(|kept| kept == dataset.sha256)
    };
    if !current(&dir) {
        let archive = scratch(cache, name, "tar");
        download(cache, name, dataset, &archive)?;
        // Unpacked beside where it goes and then given its name, so that
        // nobody reads a directory that is half there.
        let unpacking = scratch(cache, name, "unpacking");
        let unpack = || -> Result<()> {
            std::fs::create_dir_all(&unpacking).map_err(Error::io(&unpacking))?;
            let status = Command::new("tar")
                .arg("-xf")
                .arg(&archive)
                .arg("-C")
                .arg(&unpacking)
                .status()
                .map_err(|error| {
                    Error::Scenario(format!(
                        "dataset `{name}`: tar is needed to unpack it and could not be run: {error}"
                    ))
                })?;
            if !status.success() {
                return Err(Error::Scenario(format!(
                    "dataset `{name}`: the download could not be unpacked with tar"
                )));
            }
            let stamp = unpacking.join(".sha256");
            std::fs::write(&stamp, &dataset.sha256).map_err(Error::io(&stamp))?;
            if dir.exists() && !current(&dir) {
                std::fs::remove_dir_all(&dir).map_err(Error::io(&dir))?;
            }
            match std::fs::rename(&unpacking, &dir) {
                // Another run put the same files there first.
                Err(_) if current(&dir) => Ok(()),
                renamed => renamed.map_err(Error::io(&dir)),
            }
        };
        let unpacked = unpack();
        let _ = std::fs::remove_file(&archive);
        if unpacking.exists() {
            let _ = std::fs::remove_dir_all(&unpacking);
        }
        unpacked?;
    }
    let file = dir.join(inside);
    if !file.is_file() {
        return Err(Error::Scenario(format!(
            "dataset `{name}`: the archive has no file `{member}`"
        )));
    }
    Ok(file)
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
