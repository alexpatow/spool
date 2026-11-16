//! Benchmark scenarios.
//!
//! A scenario is an ordered list of steps with the truth about every
//! observation. Each step ingests files, applies the corrections a reviewer
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
//! ```
//!
//! A correction names observations as `source:key`, and is one of:
//!
//! | Field | What it does |
//! | --- | --- |
//! | `merge = [a, b]` | merges the identity of `a` into the identity of `b` |
//! | `assign = [a, b]` | puts `a` into the identity of `b` |
//! | `split = [a, ...]` | splits the observations from the rest of their identity, and keeps the sides apart |
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

use crate::engine::{ConstraintFilter, Engine, Options, ResolveOptions, ResolveReport};
use crate::error::{Error, Result};
use crate::ingest::IngestOptions;
use crate::lookup::{ObservationFilter, ObservationRecord};
use crate::metrics::{
    Churn, IdStability, Metrics, OrderSensitivity, OverridePersistence, SplitRecovery,
};
use crate::project::Manifest;
use crate::rows::Rows;

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
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepSpec {
    name: String,
    #[serde(default)]
    ingest: Vec<IngestSpec>,
    #[serde(default, rename = "correct")]
    corrections: Vec<CorrectionSpec>,
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
        if let Some(observations) = &self.split {
            kinds.push(Correction::Split(observations));
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

    fn observations(&self) -> &[String] {
        match self {
            Correction::Merge(pair) | Correction::Assign(pair) | Correction::Constrain(_, pair) => {
                &pair[..]
            }
            Correction::Split(observations) => observations,
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
    pub ingest: Vec<IngestSummary>,
    /// The corrections the step applied, in order, before it resolved.
    pub corrections: Vec<CorrectionSummary>,
    pub resolve: ResolveReport,
    /// The log position after the step.
    pub head: Seq,
    /// Committed state against the truth for everything ingested so far.
    pub metrics: Metrics,
    /// Of the observations that had an identity before the step, how many
    /// kept it.
    pub id_stability: IdStability,
    pub churn: Churn,
    /// Of the splits the scenario has made so far, how many still hold.
    pub split_recovery: SplitRecovery,
    /// Of the constraints the scenario has recorded so far, how many are
    /// still honoured.
    pub override_persistence: OverridePersistence,
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
    /// The two sides of each split, as observation IDs. `None` once undone.
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

        if spec.truth.file.is_some() == spec.truth.column.is_some() {
            return Err(invalid(
                "[truth] needs exactly one of `file` and `column`".to_owned(),
            ));
        }
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
                    return Err(invalid("a split needs observations".to_owned()));
                }
                for observation in kind.observations() {
                    if observation.split_once(':').is_none() {
                        return Err(invalid(format!(
                            "`{observation}` does not name an observation as `source:key`"
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
                        "step `{}`: an ingest needs a `key` column, which is how truth is joined to observations",
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

        // (source, key) to true entity, for everything ingested so far.
        let mut truth: HashMap<(String, String), String> = HashMap::new();
        if let Some(file) = &self.spec.truth.file {
            let path = self.dir.join(file);
            for (source, key, entity) in read_truth_file(&path)? {
                truth.insert((source, key), entity);
            }
        }

        let mut tracked = Tracked::default();
        let mut steps = Vec::new();
        for step in &self.spec.steps {
            let before = engine.head()?;
            let mut ingested = Vec::new();
            for ingest in &step.ingest {
                ingested.push(self.ingest(engine, ingest, &mut truth)?);
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

            let resolve = engine.resolve(&ResolveOptions::default())?;
            let head = engine.head()?;
            let new_observations = ingested.iter().map(|ingest| ingest.inserted).sum();
            let (split_recovery, override_persistence) = held(engine, &tracked)?;
            let report = StepReport {
                name: step.name.clone(),
                ingest: ingested,
                corrections,
                resolve,
                head,
                metrics: engine.score(truth_relation(engine, &truth)?)?,
                id_stability: engine.id_stability(before, head)?,
                churn: engine.churn(before, head, new_observations)?,
                split_recovery,
                override_persistence,
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
        truth: &mut HashMap<(String, String), String>,
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
                truth.insert((ingest.source.clone(), key), entity);
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
        let batches: Vec<&StepSpec> = self
            .spec
            .steps
            .iter()
            .filter(|step| !step.ingest.is_empty())
            .collect();
        if batches.len() < 2 {
            return Ok(None);
        }
        let replay = |steps: &mut dyn Iterator<Item = &&StepSpec>| -> Result<_> {
            let engine = Engine::open_in_memory(Options::seeded(0))?;
            engine.apply(manifest)?;
            let mut truth = HashMap::new();
            for step in steps {
                for ingest in &step.ingest {
                    self.ingest(&engine, ingest, &mut truth)?;
                }
                engine.resolve(&ResolveOptions::default())?;
            }
            grouping(&engine)
        };
        let corrected = self
            .spec
            .steps
            .iter()
            .any(|step| !step.corrections.is_empty());
        let written = if corrected {
            replay(&mut batches.iter())?
        } else {
            grouping(engine)?
        };
        let reversed = replay(&mut batches.iter().rev())?;
        Ok(Some(OrderSensitivity::between(&written, &reversed)))
    }
}

/// Every observation with the group it is in: its identity, or itself when
/// it has none.
fn grouping(engine: &Engine) -> Result<Vec<(String, String)>> {
    let batches = engine.query(
        "SELECT o.observation_id::VARCHAR,
                coalesce(m.identity_id::VARCHAR, 'unassigned:' || o.observation_id::VARCHAR)
         FROM spool.observation o
         LEFT JOIN spool.membership m ON m.observation_id = o.observation_id AND m.to_seq IS NULL
         ORDER BY o.observation_id",
    )?;
    let mut groups = Vec::new();
    for batch in &batches {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (observations, identities) = (column(0), column(1));
        for row in 0..batch.num_rows() {
            groups.push((
                observations.value(row).to_owned(),
                identities.value(row).to_owned(),
            ));
        }
    }
    Ok(groups)
}

/// The version of a row the source holds now, named as `source:key`.
fn observation(engine: &Engine, reference: &str) -> Result<ObservationRecord> {
    let (source, key) = reference
        .split_once(':')
        .expect("checked when the scenario was loaded");
    engine
        .observations(&ObservationFilter::keyed(source, key))?
        .into_iter()
        .find(|version| version.superseded_by.is_none())
        .ok_or_else(|| Error::Scenario(format!("no observation is `{reference}`")))
}

fn identity_of(engine: &Engine, reference: &str) -> Result<IdentityId> {
    observation(engine, reference)?
        .identity
        .ok_or_else(|| Error::Scenario(format!("`{reference}` has no identity")))
}

fn members(engine: &Engine, identity: IdentityId) -> Result<Vec<String>> {
    Ok(engine
        .identity(identity)?
        .members
        .iter()
        .map(|member| member.observation.to_string())
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
            let moved = observation(engine, moved)?;
            let authored = author(vec![Op::Assign {
                observation: moved.id,
                from: moved.identity,
                to: Some(identity_of(engine, target)?),
            }])?;
            (authored.changeset, authored.committed.map(|c| c.seq))
        }
        Correction::Split(observations) => {
            let identity = identity_of(engine, &observations[0])?;
            let observations = observations
                .iter()
                .map(|reference| observation(engine, reference).map(|found| found.id))
                .collect::<Result<Vec<_>>>()?;
            let report =
                engine.split(identity, &observations, true, actor.clone(), reason.clone())?;
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
                a: observation(engine, a)?.id,
                b: observation(engine, b)?.id,
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
    let group = |observation: &String| groups.get(observation);

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

/// The truth relation `(observation_id, true_entity)` for the observations
/// the store holds, joined to the truth by source and key.
fn truth_relation(engine: &Engine, truth: &HashMap<(String, String), String>) -> Result<Rows> {
    let observations = engine.query(
        "SELECT observation_id::VARCHAR, source_id, source_key
         FROM spool.observation WHERE source_key IS NOT NULL",
    )?;
    let mut rows: Vec<(String, &str)> = Vec::new();
    for batch in &observations {
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
    Rows::from_text(&["observation_id", "true_entity"], &rows)
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
