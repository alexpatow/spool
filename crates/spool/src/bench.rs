//! Benchmark scenarios.
//!
//! A scenario is an ordered list of steps with the truth about every
//! observation. Each step ingests one or more files and resolves, through the
//! same calls any caller uses, and committed state is then scored against the
//! truth. Nothing is special-cased for the benchmark.
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
//! ```

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use duckdb::arrow::array::{Array, AsArray};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spool_core::Seq;

use crate::engine::{Engine, ResolveOptions, ResolveReport};
use crate::error::{Error, Result};
use crate::ingest::IngestOptions;
use crate::metrics::{IdStability, Metrics};
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
    ingest: Vec<IngestSpec>,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepReport {
    pub name: String,
    pub ingest: Vec<IngestSummary>,
    pub resolve: ResolveReport,
    /// The log position after the step.
    pub head: Seq,
    /// Committed state against the truth for everything ingested so far.
    pub metrics: Metrics,
    /// Of the observations that had an identity before the step, how many
    /// kept it.
    pub id_stability: IdStability,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestSummary {
    pub source: String,
    pub entity_type: String,
    pub received: u64,
    pub inserted: u64,
    pub duplicates: u64,
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
        for step in &spec.steps {
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

        let mut steps = Vec::new();
        for step in &self.spec.steps {
            let before = engine.head()?;
            let mut ingested = Vec::new();
            for ingest in &step.ingest {
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
                ingested.push(IngestSummary {
                    source: ingest.source.clone(),
                    entity_type: ingest.entity_type.clone(),
                    received: report.received,
                    inserted: report.inserted,
                    duplicates: report.duplicates,
                });
            }

            let resolve = engine.resolve(&ResolveOptions::default())?;
            let head = engine.head()?;
            let report = StepReport {
                name: step.name.clone(),
                ingest: ingested,
                resolve,
                head,
                metrics: engine.score(truth_relation(engine, &truth)?)?,
                id_stability: engine.id_stability(before, head)?,
            };
            after_step(&report)?;
            steps.push(report);
        }

        engine.verify()?;
        Ok(BenchReport {
            scenario: self.name.clone(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            manifest_hash: manifest.hash,
            steps,
        })
    }
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
