//! The engine: one type that owns the store and is the whole API.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use duckdb::arrow::record_batch::{RecordBatch, RecordBatchReader};
use duckdb::params;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use serde::{Deserialize, Serialize};
use serde_json::json;
use spool_core::{
    Changeset, Clock, Committed, Draft, EntityType, EvidenceSummary, IdGenerator, IdentityId,
    IdentityLink, IdentityState, IdentityStatus, IdentityStore, LogEntry, MemoryState,
    NewObservation, NewPair, ObservationId, Policy, RandomIds, ReviewItem, RunId, SeededIds, Seq,
    Snapshot, StepClock, SystemClock, Verdict, reconcile, validate,
};

use crate::error::{Error, Result};
use crate::features;
use crate::ingest::{self, IngestOptions, IngestReport};
use crate::metrics::{self, IdStability, Metrics};
use crate::project::{EntityTypeManifest, Manifest, hash_json};
use crate::review;
use crate::rules;
use crate::schema;
use crate::store::DuckStore;

/// Where an engine gets new IDs and the time from. The default mints UUIDv7
/// values and reads the system clock.
pub struct Options {
    pub ids: Box<dyn IdGenerator + Send>,
    pub clock: Box<dyn Clock + Send>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            ids: Box::new(RandomIds),
            clock: Box::new(SystemClock),
        }
    }
}

impl Options {
    /// Reproducible IDs and a clock that starts at 2026-01-01T00:00:00Z and
    /// advances one second per reading. For tests and benchmarks: a seed must
    /// not be used twice against one store.
    pub fn seeded(seed: u64) -> Self {
        Self {
            ids: Box::new(SeededIds::new(seed)),
            clock: Box::new(StepClock::new(1_767_225_600_000_000, 1_000_000)),
        }
    }
}

/// Names the strategy that produces a match run's judgements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRef {
    pub name: String,
    pub version: Option<String>,
}

impl StrategyRef {
    /// The built-in runner of the project's SQL rules.
    pub const RULES: &'static str = "rules";

    pub fn rules() -> Self {
        Self {
            name: Self::RULES.to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }
    }

    /// A strategy outside the engine that reads a run's candidates and
    /// submits judgements.
    pub fn external(name: impl Into<String>, version: Option<String>) -> Self {
        Self {
            name: name.into(),
            version,
        }
    }
}

/// Which observations a match run treats as new: those of one entity type
/// ingested since the last match run of that type that succeeded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub entity_type: EntityType,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolveOptions {
    /// Resolve only this entity type. `None` resolves every declared type.
    pub entity_type: Option<EntityType>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyReport {
    /// Entity types declared by this call.
    pub declared: Vec<EntityType>,
    /// Entity types the store already held with the same declaration.
    pub unchanged: Vec<EntityType>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveReport {
    /// One entry per entity type that had new observations.
    pub resolved: Vec<TypeResolution>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeResolution {
    pub entity_type: EntityType,
    pub match_run: RunId,
    pub reconcile_run: RunId,
    /// Observations the run examined: those ingested since the last run, and
    /// those an earlier run left for review.
    pub new_observations: u64,
    /// How many of them an earlier run had left for review.
    pub reexamined: u64,
    /// Distinct candidate pairs.
    pub candidates: u64,
    pub judgements: JudgementCounts,
    pub identities_created: u64,
    pub observations_assigned: u64,
    /// Examined observations left without an identity. Each is named in
    /// `review`.
    pub observations_unassigned: u64,
    /// What the run would not decide. The store keeps these as review items.
    pub review: Vec<ReviewItem>,
    /// `None` when there was nothing to commit.
    pub committed: Option<Committed>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JudgementCounts {
    #[serde(rename = "match")]
    pub match_: u64,
    pub no_match: u64,
    pub unsure: u64,
}

/// The engine. It owns the database file, and every change to identity state
/// goes through [`commit`](Engine::commit).
///
/// Calls are serialised: the engine holds one connection behind a lock.
pub struct Engine {
    inner: Mutex<Inner>,
}

struct Inner {
    store: DuckStore,
    ids: Box<dyn IdGenerator + Send>,
    manifest: Option<Manifest>,
}

/// What a match run recorded about itself when it began.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct MatchConfig {
    entity_type: EntityType,
    declaration_version: u32,
    manifest_hash: String,
    /// New observations are those ingested after this run.
    after_run: RunId,
    /// The review items that were open when the run began. Their observations
    /// are examined again.
    #[serde(default)]
    review_items: Vec<u64>,
    rules: crate::project::RuleSet,
}

struct RunRow {
    kind: String,
    status: String,
    config: Option<String>,
}

impl Engine {
    /// Opens the store at `path`, creating it if the file does not exist.
    pub fn open(path: impl AsRef<Path>, options: Options) -> Result<Self> {
        Self::with_store(
            DuckStore::open(Some(path.as_ref()), options.clock)?,
            options.ids,
        )
    }

    /// Opens a store that lives in memory and is gone when the engine is
    /// dropped.
    pub fn open_in_memory(options: Options) -> Result<Self> {
        Self::with_store(DuckStore::open(None, options.clock)?, options.ids)
    }

    fn with_store(store: DuckStore, ids: Box<dyn IdGenerator + Send>) -> Result<Self> {
        Ok(Self {
            inner: Mutex::new(Inner {
                store,
                ids,
                manifest: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Declares the manifest's entity types in the store, generating a
    /// feature table for each new one, and makes the manifest's rules the
    /// ones this engine runs. Applying the same manifest again changes
    /// nothing in the store.
    ///
    /// A type whose declaration differs from the one in the store is refused:
    /// changing a declaration after the fact is not supported yet.
    pub fn apply(&self, manifest: &Manifest) -> Result<ApplyReport> {
        self.lock().apply(manifest)
    }

    /// Stores rows from one source as observations of one entity type and
    /// computes their feature rows. A row that is already known, by its
    /// source, key and content, creates nothing.
    pub fn ingest(
        &self,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
        options: &IngestOptions,
    ) -> Result<IngestReport> {
        self.lock().ingest(source, entity_type, rows, options)
    }

    /// Starts a match run and generates its candidate pairs with the
    /// project's block rules. The run examines the observations ingested
    /// since the last match run of the type that succeeded, and again those
    /// of open review items.
    pub fn begin_run(&self, strategy: &StrategyRef, scope: &Scope) -> Result<RunId> {
        self.lock().begin_run(strategy, scope)
    }

    /// A run's candidate pairs with both sides' features: `left_id`,
    /// `right_id`, then every feature column prefixed with `left_` and with
    /// `right_`.
    pub fn candidates(&self, run: RunId) -> Result<Vec<RecordBatch>> {
        self.lock().candidates(run)
    }

    /// Adds judgements to a running match run and returns how many were
    /// stored. The rows need `left_id`, `right_id` and `verdict`, and may
    /// have `score` and `detail`.
    ///
    /// The whole submission is refused if a row names an observation the
    /// store does not hold as this run's entity type, or a pair in which
    /// neither observation is new to the run.
    pub fn submit_judgements(&self, run: RunId, rows: impl RecordBatchReader) -> Result<u64> {
        self.lock().submit_judgements(run, rows)
    }

    /// Closes a match run and turns its judgements into a draft changeset.
    /// Nothing is committed. What the draft leaves out is stored as review
    /// items, which stay open until their observations have an identity.
    pub fn reconcile(&self, run: RunId, policy: &Policy) -> Result<Draft> {
        self.lock().reconcile(run, policy)
    }

    /// Validates a changeset against current state and commits it. This is
    /// the only way identity state changes, whoever wrote the changeset.
    pub fn commit(&self, changeset: Changeset) -> Result<Committed> {
        self.lock().commit(changeset)
    }

    /// Matches new observations with the built-in rule runner, reconciles
    /// with the default policy, and commits. A type is resolved when it has
    /// newly ingested observations or open review items.
    ///
    /// The match run is recorded as succeeded only after the commit. A
    /// resolve that is cut short leaves the run unfinished, and the next
    /// resolve examines the same observations again.
    pub fn resolve(&self, options: &ResolveOptions) -> Result<ResolveReport> {
        self.lock().resolve(options)
    }

    pub fn identity_of(&self, observation: ObservationId) -> Result<Option<IdentityId>> {
        Ok(self.lock().store.assignments(&[observation])?[0])
    }

    /// The active identity an ID stands for: itself, or the identity it was
    /// merged into.
    pub fn current_id(&self, id: IdentityId) -> Result<IdentityId> {
        match self.lock().store.identities(&[id])?.remove(0) {
            None => Err(Error::UnknownIdentity(id)),
            Some(record) => Ok(match record.status {
                IdentityStatus::Active => id,
                IdentityStatus::MergedInto(survivor) => survivor,
            }),
        }
    }

    /// Runs one `SELECT`. The stable things to read are the views
    /// `spool.identity_current`, `spool.membership_current` and
    /// `spool.entity_<type>`. UUID columns come back as strings.
    ///
    /// The statement is run as a subquery, so anything that is not a query
    /// is a syntax error. It can still read files the process can read, so
    /// this is not for SQL from untrusted callers.
    pub fn query(&self, sql: &str) -> Result<Vec<RecordBatch>> {
        let sql = sql.trim().trim_end_matches(';');
        if sql.is_empty() {
            return Err(Error::Query("the query is empty".to_owned()));
        }
        let inner = self.lock();
        let mut statement = inner
            .store
            .conn
            .prepare(&format!("SELECT * FROM (\n{sql}\n)"))
            .map_err(|error| Error::Query(error.to_string()))?;
        Ok(statement.query_arrow([])?.collect())
    }

    /// The position of the last committed changeset.
    pub fn head(&self) -> Result<Seq> {
        Ok(self.lock().store.head()?)
    }

    /// Every committed changeset, in log order.
    pub fn log(&self) -> Result<Vec<LogEntry>> {
        Ok(self.lock().store.log()?)
    }

    /// All of identity state, read from the projection tables.
    pub fn snapshot(&self) -> Result<Snapshot> {
        Ok(self.lock().store.snapshot()?)
    }

    /// Checks that the projection tables equal a replay of the log, and that
    /// the tables hold what the validator guarantees.
    pub fn verify(&self) -> Result<()> {
        self.lock().verify()
    }

    /// Replaces the projection tables with a replay of the log.
    pub fn rebuild_projection(&self) -> Result<()> {
        let mut inner = self.lock();
        let replayed = MemoryState::replay(&inner.store.log()?)?;
        inner.store.write_projection(&replayed.snapshot()?)
    }

    /// Scores current state against a truth relation with the columns
    /// `observation_id` and `true_entity`.
    pub fn score(&self, truth: impl RecordBatchReader) -> Result<Metrics> {
        let inner = self.lock();
        let conn = &inner.store.conn;
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {} (observation_id UUID, true_entity VARCHAR)",
            metrics::TRUTH
        ))?;
        for batch in truth {
            conn.execute(
                &format!(
                    "INSERT INTO {} SELECT observation_id::UUID, true_entity::VARCHAR FROM arrow(?)",
                    metrics::TRUTH
                ),
                [&ArrowBatchRegistration::new(batch?)],
            )?;
        }
        metrics::score(conn)
    }

    /// Compares each observation's identity at two log positions.
    pub fn id_stability(&self, before: Seq, after: Seq) -> Result<IdStability> {
        metrics::id_stability(&self.lock().store.conn, before, after)
    }
}

/// An ID read from the store could not be parsed, which means the store is
/// corrupt.
fn id_error(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::State(spool_core::StateError::new(error))
}

impl Inner {
    fn apply(&mut self, manifest: &Manifest) -> Result<ApplyReport> {
        let now = self.store.clock.now_micros();
        let mut report = ApplyReport {
            declared: Vec::new(),
            unchanged: Vec::new(),
        };
        let tx = self.store.conn.transaction()?;
        for entity_type in &manifest.entity_types {
            let name = &entity_type.name;
            let stored: Option<String> = {
                let mut statement = tx.prepare(
                    "SELECT declaration_hash FROM spool.entity_type
                     WHERE entity_type = ? ORDER BY version DESC LIMIT 1",
                )?;
                let mut rows = statement.query([name.as_str()])?;
                match rows.next()? {
                    Some(row) => Some(row.get(0)?),
                    None => None,
                }
            };
            match stored {
                Some(hash) if hash == entity_type.declaration_hash => {
                    report.unchanged.push(name.clone());
                }
                Some(_) => return Err(Error::DeclarationChanged(name.to_string())),
                None => {
                    tx.execute(
                        "INSERT INTO spool.entity_type VALUES (?, 1, ?, ?, make_timestamptz(?))",
                        params![
                            name.as_str(),
                            json!(entity_type.declaration).to_string(),
                            entity_type.declaration_hash,
                            now
                        ],
                    )?;
                    features::create_table(&tx, name, &entity_type.declaration, 1)?;
                    report.declared.push(name.clone());
                }
            }
        }
        schema::set_meta(&tx, "manifest_hash", &manifest.hash)?;
        tx.commit()?;
        self.manifest = Some(manifest.clone());
        Ok(report)
    }

    /// The applied manifest's entry for a type, with the version the store
    /// holds for it.
    fn declared(&self, entity_type: &str) -> Result<(&Manifest, &EntityTypeManifest, u32)> {
        let manifest = self.manifest.as_ref().ok_or(Error::NoManifest)?;
        let entry = manifest
            .entity_type(entity_type)
            .ok_or_else(|| Error::UndeclaredType(entity_type.to_owned()))?;
        let version: Option<i32> = self.store.conn.query_row(
            "SELECT max(version) FROM spool.entity_type WHERE entity_type = ?",
            [entity_type],
            |row| row.get(0),
        )?;
        let version = version.ok_or_else(|| Error::UndeclaredType(entity_type.to_owned()))?;
        Ok((manifest, entry, version as u32))
    }

    fn start_run(
        &mut self,
        kind: &str,
        strategy: Option<&StrategyRef>,
        config: &serde_json::Value,
    ) -> Result<RunId> {
        let now = self.store.clock.now_micros();
        let conn = &self.store.conn;
        let run: i64 = conn.query_row(
            "SELECT coalesce(max(run_id), 0) + 1 FROM spool.run",
            [],
            |row| row.get(0),
        )?;
        conn.execute(
            "INSERT INTO spool.run
             VALUES (?, ?, 'running', ?, ?, ?, ?, ?, make_timestamptz(?), NULL)",
            params![
                run,
                kind,
                strategy.map(|strategy| strategy.name.as_str()),
                strategy.and_then(|strategy| strategy.version.as_deref()),
                config.to_string(),
                hash_json(config),
                env!("CARGO_PKG_VERSION"),
                now
            ],
        )?;
        Ok(RunId(run as u64))
    }

    fn finish_run(&mut self, run: RunId, status: &str) -> Result<()> {
        let now = self.store.clock.now_micros();
        self.store.conn.execute(
            "UPDATE spool.run SET status = ?, finished_at = make_timestamptz(?) WHERE run_id = ?",
            params![status, now, run.0 as i64],
        )?;
        Ok(())
    }

    /// Runs the work of a run, and records the run as failed if it fails.
    fn in_run<T>(&mut self, run: RunId, work: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let result = work(self);
        if result.is_err() {
            // The first error is the one worth reporting.
            let _ = self.finish_run(run, "failed");
        }
        result
    }

    fn run_row(&self, run: RunId) -> Result<RunRow> {
        let mut statement = self
            .store
            .conn
            .prepare("SELECT kind, status, config::VARCHAR FROM spool.run WHERE run_id = ?")?;
        let mut rows = statement.query([run.0 as i64])?;
        match rows.next()? {
            Some(row) => Ok(RunRow {
                kind: row.get(0)?,
                status: row.get(1)?,
                config: row.get(2)?,
            }),
            None => Err(Error::Run {
                run,
                message: "there is no such run".to_owned(),
            }),
        }
    }

    fn match_run(&self, run: RunId) -> Result<(RunRow, MatchConfig)> {
        let row = self.run_row(run)?;
        let fail = |message: &str| Error::Run {
            run,
            message: message.to_owned(),
        };
        if row.kind != "match" {
            return Err(fail("it is not a match run"));
        }
        let config = row
            .config
            .as_deref()
            .and_then(|config| serde_json::from_str(config).ok())
            .ok_or_else(|| fail("its recorded configuration cannot be read"))?;
        Ok((row, config))
    }

    fn ingest(
        &mut self,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
        options: &IngestOptions,
    ) -> Result<IngestReport> {
        if source.is_empty() {
            return Err(Error::Ingest("the source needs a name".to_owned()));
        }
        let (_, entry, version) = self.declared(entity_type)?;
        let name = entry.name.clone();
        let declaration = entry.declaration.clone();

        let run = self.start_run(
            "ingest",
            None,
            &json!({ "source": source, "entity_type": name }),
        )?;
        self.in_run(run, |inner| {
            let now = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            let mut seen = HashSet::new();
            let (mut received, mut inserted) = (0u64, 0u64);

            for incoming in rows {
                let incoming = incoming?;
                let staged = ingest::stage(source, &incoming, options, &mut seen, received as usize)?;
                received += incoming.num_rows() as u64;
                let Some(staged) = staged else { continue };

                // The ID does not cover the entity type, so the same row sent
                // as another type would otherwise be dropped without a word.
                let clash: Option<String> = {
                    let mut statement = tx.prepare(
                        "SELECT o.entity_type FROM arrow(?) s
                         JOIN spool.observation o ON o.observation_id = s.observation_id::UUID
                         WHERE o.entity_type <> ? LIMIT 1",
                    )?;
                    let mut found = statement.query(params![
                        &ArrowBatchRegistration::new(staged.clone()),
                        name.as_str()
                    ])?;
                    match found.next()? {
                        Some(row) => Some(row.get(0)?),
                        None => None,
                    }
                };
                if let Some(other) = clash {
                    return Err(Error::Ingest(format!(
                        "a row with the same source, key and content is already stored as entity type `{other}`"
                    )));
                }

                inserted += tx.execute(
                    "INSERT INTO spool.observation
                     SELECT s.observation_id::UUID, ?, ?, s.source_key, NULL,
                            s.payload::JSON, s.evidence::JSON, NULL, make_timestamptz(?), ?
                     FROM arrow(?) s
                     WHERE NOT EXISTS (
                       SELECT 1 FROM spool.observation o
                       WHERE o.observation_id = s.observation_id::UUID)",
                    params![
                        name.as_str(),
                        source,
                        now,
                        run.0 as i64,
                        &ArrowBatchRegistration::new(staged)
                    ],
                )? as u64;
            }

            features::fill(&tx, &name, &declaration, version, run)?;
            let finished = inner.store.clock.now_micros();
            tx.execute(
                "UPDATE spool.run SET status = 'succeeded', finished_at = make_timestamptz(?)
                 WHERE run_id = ?",
                params![finished, run.0 as i64],
            )?;
            tx.commit()?;
            Ok(IngestReport {
                run,
                received,
                inserted,
                duplicates: received - inserted,
            })
        })
    }

    fn begin_run(&mut self, strategy: &StrategyRef, scope: &Scope) -> Result<RunId> {
        let (manifest, entry, version) = self.declared(scope.entity_type.as_str())?;
        let entity_type = entry.name.clone();
        let after_run = self.last_match_run(&entity_type)?;
        let review_items = review::open_ids(&self.store.conn, &entity_type)?;
        let config = MatchConfig {
            entity_type: entity_type.clone(),
            declaration_version: version,
            manifest_hash: manifest.hash.clone(),
            after_run,
            review_items,
            rules: entry.rules.clone(),
        };

        let run = self.start_run("match", Some(strategy), &json!(config))?;
        self.in_run(run, |inner| {
            let tx = inner.store.conn.transaction()?;
            rules::create_scope(
                &tx,
                &entity_type,
                config.after_run,
                run,
                &config.review_items,
            )?;
            rules::generate_candidates(&tx, &entity_type, run, &config.rules.block)?;
            tx.commit()?;
            Ok(run)
        })
    }

    /// The last match run of an entity type that succeeded, or run 0. What
    /// was ingested after it is new.
    fn last_match_run(&self, entity_type: &EntityType) -> Result<RunId> {
        let run: i64 = self.store.conn.query_row(
            "SELECT coalesce(max(run_id), 0) FROM spool.run
             WHERE kind = 'match' AND status = 'succeeded'
               AND json_extract_string(config, '$.entity_type') = ?",
            [entity_type.as_str()],
            |row| row.get(0),
        )?;
        Ok(RunId(run as u64))
    }

    /// Recreates the scope table of a match run that is still open.
    fn open_match_run(&self, run: RunId) -> Result<MatchConfig> {
        let (row, config) = self.match_run(run)?;
        if row.status != "running" {
            return Err(Error::Run {
                run,
                message: format!("it is {} and takes no more judgements", row.status),
            });
        }
        rules::create_scope(
            &self.store.conn,
            &config.entity_type,
            config.after_run,
            run,
            &config.review_items,
        )?;
        Ok(config)
    }

    fn candidates(&mut self, run: RunId) -> Result<Vec<RecordBatch>> {
        let (_, config) = self.match_run(run)?;
        let table = features::table(&config.entity_type);
        let exclude = "EXCLUDE (observation_id, declaration_version)";
        let has_features: bool = self.store.conn.query_row(
            &format!("SELECT count(*) > 2 FROM (DESCRIBE {table})"),
            [],
            |row| row.get(0),
        )?;
        let features = if has_features {
            format!(
                ", COLUMNS(l.* {exclude}) AS \"left_\\0\", COLUMNS(r.* {exclude}) AS \"right_\\0\""
            )
        } else {
            String::new()
        };
        let mut statement = self.store.conn.prepare(&format!(
            "SELECT c.left_id, c.right_id{features}
             FROM (SELECT DISTINCT left_id, right_id FROM spool.candidate WHERE run_id = ?) c
             JOIN {table} l ON l.observation_id = c.left_id
             JOIN {table} r ON r.observation_id = c.right_id
             ORDER BY c.left_id, c.right_id"
        ))?;
        Ok(statement.query_arrow([run.0 as i64])?.collect())
    }

    fn submit_judgements(&mut self, run: RunId, rows: impl RecordBatchReader) -> Result<u64> {
        let config = self.open_match_run(run)?;
        let entity_type = &config.entity_type;
        let schema = rows.schema();
        for required in ["left_id", "right_id", "verdict"] {
            if schema.index_of(required).is_err() {
                return Err(Error::Judgements(format!(
                    "the rows have no `{required}` column"
                )));
            }
        }
        let score = match schema.index_of("score") {
            Ok(_) => "j.score::DOUBLE",
            Err(_) => "NULL::DOUBLE",
        };
        let detail = match schema.index_of("detail") {
            Ok(_) => "j.detail::VARCHAR",
            Err(_) => "NULL::VARCHAR",
        };

        let tx = self.store.conn.transaction()?;
        let mut stored = 0;
        for incoming in rows {
            let incoming = incoming?;
            let problem: Option<(Option<String>, Option<String>, String)> = {
                let mut statement = tx.prepare(&format!(
                    "SELECT left_id, right_id, problem FROM (
                       SELECT j.left_id::VARCHAR AS left_id, j.right_id::VARCHAR AS right_id,
                         CASE
                           WHEN try_cast(j.left_id AS UUID) IS NULL OR try_cast(j.right_id AS UUID) IS NULL
                             THEN 'an ID is missing or is not a UUID'
                           WHEN try_cast(j.left_id AS UUID) = try_cast(j.right_id AS UUID)
                             THEN 'both sides are the same observation'
                           WHEN j.verdict IS NULL OR j.verdict NOT IN ('match', 'no_match', 'unsure')
                             THEN 'the verdict must be match, no_match or unsure'
                           WHEN {detail} IS NOT NULL AND NOT json_valid({detail})
                             THEN 'the detail is not valid JSON'
                           WHEN l.observation_id IS NULL OR r.observation_id IS NULL
                             THEN 'it names an observation that is not a known {entity_type}'
                           WHEN ls.observation_id IS NULL AND rs.observation_id IS NULL
                             THEN 'neither observation is new to this run'
                         END AS problem
                       FROM arrow(?) j
                       LEFT JOIN spool.observation l
                         ON l.observation_id = try_cast(j.left_id AS UUID) AND l.entity_type = '{entity_type}'
                       LEFT JOIN spool.observation r
                         ON r.observation_id = try_cast(j.right_id AS UUID) AND r.entity_type = '{entity_type}'
                       LEFT JOIN {scope} ls ON ls.observation_id = l.observation_id
                       LEFT JOIN {scope} rs ON rs.observation_id = r.observation_id
                     ) WHERE problem IS NOT NULL LIMIT 1",
                    scope = rules::SCOPE,
                ))?;
                let mut found =
                    statement.query([&ArrowBatchRegistration::new(incoming.clone())])?;
                match found.next()? {
                    Some(row) => Some((row.get(0)?, row.get(1)?, row.get(2)?)),
                    None => None,
                }
            };
            if let Some((left, right, problem)) = problem {
                return Err(Error::Judgements(format!(
                    "the judgement between {} and {}: {problem}",
                    left.as_deref().unwrap_or("nothing"),
                    right.as_deref().unwrap_or("nothing"),
                )));
            }
            stored += tx.execute(
                &format!(
                    "INSERT INTO spool.judgement
                     SELECT ?, least(j.left_id::UUID, j.right_id::UUID),
                            greatest(j.left_id::UUID, j.right_id::UUID),
                            j.verdict, {score}, ({detail})::JSON
                     FROM arrow(?) j"
                ),
                params![run.0 as i64, &ArrowBatchRegistration::new(incoming)],
            )? as u64;
        }
        tx.commit()?;
        Ok(stored)
    }

    /// The built-in strategy: fills a match run's judgements from the
    /// project's `no_match` and `match` rules.
    fn judge_with_rules(&mut self, run: RunId) -> Result<()> {
        let config = self.open_match_run(run)?;
        rules::judge_candidates(
            &self.store.conn,
            &config.entity_type,
            run,
            &config.rules.no_match,
            &config.rules.match_,
        )
    }

    fn reconcile(&mut self, run: RunId, policy: &Policy) -> Result<Draft> {
        let (row, _) = self.match_run(run)?;
        if row.status == "running" {
            self.finish_run(run, "succeeded")?;
        }
        self.draft(run, policy)
    }

    /// Turns a match run's judgements into a draft and stores its review
    /// items. The match run is left as it is: whether it has succeeded is
    /// for the caller to say.
    fn draft(&mut self, run: RunId, policy: &Policy) -> Result<Draft> {
        let (row, config) = self.match_run(run)?;
        if !matches!(row.status.as_str(), "running" | "succeeded") {
            return Err(Error::Run {
                run,
                message: format!("it {} and cannot be reconciled", row.status),
            });
        }

        let reconcile_run = self.start_run(
            "reconcile",
            None,
            &json!({ "match_run": run, "policy": policy }),
        )?;
        self.in_run(reconcile_run, |inner| {
            let evidence = inner.evidence(run, &config)?;
            let mut draft = reconcile(&evidence, &inner.store, policy, inner.ids.as_mut())?;
            draft.changeset.run = Some(reconcile_run);

            let finished = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            review::record(&tx, &config.entity_type, reconcile_run, &draft.review)?;
            tx.execute(
                "UPDATE spool.run SET status = 'succeeded', finished_at = make_timestamptz(?)
                 WHERE run_id = ?",
                params![finished, reconcile_run.0 as i64],
            )?;
            tx.commit()?;
            Ok(draft)
        })
    }

    /// Joins a match run's judgements against current membership. This is
    /// the heavy part, and DuckDB does it; the reconciler sees a summary.
    fn evidence(&self, run: RunId, config: &MatchConfig) -> Result<EvidenceSummary> {
        let conn = &self.store.conn;
        let scope = rules::SCOPE;
        rules::create_scope(
            conn,
            &config.entity_type,
            config.after_run,
            run,
            &config.review_items,
        )?;
        // An observation that got an identity since the run began is no longer new.
        conn.execute_batch(&format!(
            "DELETE FROM {scope} WHERE observation_id IN (
               SELECT observation_id FROM spool.membership WHERE to_seq IS NULL)"
        ))?;
        let mut evidence = EvidenceSummary::default();

        let mut statement = conn.prepare(&format!(
            "SELECT observation_id::VARCHAR FROM {scope} ORDER BY observation_id"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            evidence.observations.push(NewObservation {
                observation: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                entity_type: config.entity_type.clone(),
            });
        }

        let mut statement = conn.prepare(&format!(
            "SELECT left_id::VARCHAR, right_id::VARCHAR, verdict
             FROM spool.judgement
             WHERE run_id = ?
               AND left_id IN (SELECT observation_id FROM {scope})
               AND right_id IN (SELECT observation_id FROM {scope})
             ORDER BY left_id, right_id, verdict"
        ))?;
        let mut rows = statement.query([run.0 as i64])?;
        while let Some(row) = rows.next()? {
            evidence.pairs.push(NewPair {
                a: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                b: row.get::<_, String>(1)?.parse().map_err(id_error)?,
                verdict: row
                    .get::<_, String>(2)?
                    .parse::<Verdict>()
                    .map_err(|message| Error::Run { run, message })?,
            });
        }

        let mut statement = conn.prepare(&format!(
            "SELECT j.new_id::VARCHAR, m.identity_id::VARCHAR,
                    count(*) FILTER (WHERE j.verdict = 'match'),
                    count(*) FILTER (WHERE j.verdict = 'no_match'),
                    max(j.score) FILTER (WHERE j.verdict = 'match')
             FROM (
               SELECT left_id AS new_id, right_id AS other_id, verdict, score
               FROM spool.judgement
               WHERE run_id = $1 AND left_id IN (SELECT observation_id FROM {scope})
               UNION ALL
               SELECT right_id, left_id, verdict, score
               FROM spool.judgement
               WHERE run_id = $1 AND right_id IN (SELECT observation_id FROM {scope})
             ) j
             JOIN spool.membership m ON m.observation_id = j.other_id AND m.to_seq IS NULL
             GROUP BY j.new_id, m.identity_id
             ORDER BY j.new_id, m.identity_id"
        ))?;
        let mut rows = statement.query([run.0 as i64])?;
        while let Some(row) = rows.next()? {
            evidence.links.push(IdentityLink {
                observation: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                identity: row.get::<_, String>(1)?.parse().map_err(id_error)?,
                matches: row.get::<_, i64>(2)? as u64,
                no_matches: row.get::<_, i64>(3)? as u64,
                best_score: row.get(4)?,
            });
        }
        Ok(evidence)
    }

    fn commit(&mut self, changeset: Changeset) -> Result<Committed> {
        let validated = validate(&self.store, changeset)?;
        Ok(self.store.apply(validated)?)
    }

    fn resolve(&mut self, options: &ResolveOptions) -> Result<ResolveReport> {
        let manifest = self.manifest.as_ref().ok_or(Error::NoManifest)?;
        let entity_types: Vec<EntityType> = match &options.entity_type {
            Some(entity_type) => vec![self.declared(entity_type.as_str())?.1.name.clone()],
            None => manifest
                .entity_types
                .iter()
                .map(|entry| entry.name.clone())
                .collect(),
        };

        let mut report = ResolveReport::default();
        for entity_type in entity_types {
            let after_run = self.last_match_run(&entity_type)?;
            let ingested = self.count(&format!(
                "SELECT count(*) FROM spool.observation
                 WHERE entity_type = '{entity_type}' AND ingest_run > {after_run}"
            ))?;
            if ingested == 0 && review::open_ids(&self.store.conn, &entity_type)?.is_empty() {
                continue;
            }
            let scope = Scope {
                entity_type: entity_type.clone(),
            };
            let run = self.begin_run(&StrategyRef::rules(), &scope)?;
            // Observations in scope that already have an identity are not
            // examined. There are some when an earlier resolve committed and
            // was cut short before it recorded that.
            let unplaced = format!(
                "FROM {} s
                 JOIN spool.observation o ON o.observation_id = s.observation_id
                 WHERE NOT EXISTS (
                   SELECT 1 FROM spool.membership m
                   WHERE m.observation_id = s.observation_id AND m.to_seq IS NULL)",
                rules::SCOPE
            );
            let new_observations = self.count(&format!("SELECT count(*) {unplaced}"))?;
            let reexamined = self.count(&format!(
                "SELECT count(*) {unplaced} AND o.ingest_run <= {after_run}"
            ))?;

            let (draft, committed) = self.in_run(run, |inner| {
                inner.judge_with_rules(run)?;
                let draft = inner.draft(run, &Policy::default())?;
                let committed = if draft.changeset.ops.is_empty() {
                    None
                } else {
                    Some(inner.commit(draft.changeset.clone())?)
                };
                // The run has succeeded once its draft is committed, and not
                // before: what is new to the next run is counted from the
                // last run that succeeded. If the process stops before this
                // line, the next resolve examines the same observations
                // again, and those the commit placed are no longer new.
                inner.finish_run(run, "succeeded")?;
                Ok((draft, committed))
            })?;

            let run_filter = format!("WHERE run_id = {run}");
            let judged = |inner: &Self, verdict: &str| {
                inner.count(&format!(
                    "SELECT count(*) FROM spool.judgement {run_filter} AND verdict = '{verdict}'"
                ))
            };
            let (mut identities_created, mut observations_assigned) = (0, 0);
            for op in &draft.changeset.ops {
                match op {
                    spool_core::Op::CreateIdentity { .. } => identities_created += 1,
                    spool_core::Op::Assign { .. } => observations_assigned += 1,
                    _ => {}
                }
            }
            report.resolved.push(TypeResolution {
                entity_type,
                match_run: run,
                reconcile_run: draft.changeset.run.unwrap_or(run),
                new_observations,
                reexamined,
                candidates: self.count(&format!(
                    "SELECT count(*) FROM (SELECT DISTINCT left_id, right_id FROM spool.candidate {run_filter})"
                ))?,
                judgements: JudgementCounts {
                    match_: judged(self, "match")?,
                    no_match: judged(self, "no_match")?,
                    unsure: judged(self, "unsure")?,
                },
                identities_created,
                observations_assigned,
                observations_unassigned: new_observations - observations_assigned,
                review: draft.review,
                committed,
            });
        }
        Ok(report)
    }

    fn count(&self, sql: &str) -> Result<u64> {
        let count: i64 = self.store.conn.query_row(sql, [], |row| row.get(0))?;
        Ok(count as u64)
    }

    fn verify(&mut self) -> Result<()> {
        let mut problems = Vec::new();

        let replayed = MemoryState::replay(&self.store.log()?)?;
        if replayed.snapshot()? != self.store.snapshot()? {
            problems.push("the projection tables differ from a replay of the log".to_owned());
        }

        let mut checks = vec![
            (
                "observation IDs that occur more than once".to_owned(),
                "SELECT count(*) FROM (
                   SELECT observation_id FROM spool.observation GROUP BY 1 HAVING count(*) > 1)"
                    .to_owned(),
            ),
            (
                "changeset IDs that occur more than once".to_owned(),
                "SELECT count(*) FROM (
                   SELECT changeset_id FROM spool.changeset GROUP BY 1 HAVING count(*) > 1)"
                    .to_owned(),
            ),
            (
                "observations that belong to more than one identity".to_owned(),
                "SELECT count(*) FROM (
                   SELECT observation_id FROM spool.membership WHERE to_seq IS NULL
                   GROUP BY 1 HAVING count(*) > 1)"
                    .to_owned(),
            ),
            (
                "memberships of observations the store does not hold".to_owned(),
                "SELECT count(*) FROM spool.membership m
                 WHERE NOT EXISTS (
                   SELECT 1 FROM spool.observation o WHERE o.observation_id = m.observation_id)"
                    .to_owned(),
            ),
            (
                "current memberships in an identity that is missing, merged, or of another entity type"
                    .to_owned(),
                "SELECT count(*) FROM spool.membership m
                 JOIN spool.observation o ON o.observation_id = m.observation_id
                 LEFT JOIN spool.identity i ON i.identity_id = m.identity_id
                 WHERE m.to_seq IS NULL
                   AND (i.identity_id IS NULL OR i.merged_into IS NOT NULL
                        OR i.entity_type <> o.entity_type)"
                    .to_owned(),
            ),
            (
                "open review items whose observations all have an identity".to_owned(),
                "SELECT count(*) FROM spool.review_item i
                 WHERE i.status = 'open' AND NOT EXISTS (
                   SELECT 1 FROM (SELECT unnest(i.observation_ids) AS observation_id) o
                   WHERE NOT EXISTS (
                     SELECT 1 FROM spool.membership m
                     WHERE m.observation_id = o.observation_id AND m.to_seq IS NULL))"
                    .to_owned(),
            ),
        ];
        let mut statement = self
            .store
            .conn
            .prepare("SELECT DISTINCT entity_type FROM spool.entity_type ORDER BY 1")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let entity_type: String = row.get(0)?;
            checks.push((
                format!("`{entity_type}` observations with more than one feature row"),
                format!(
                    "SELECT count(*) FROM (
                       SELECT observation_id FROM spool.features_{entity_type}
                       GROUP BY 1 HAVING count(*) > 1)"
                ),
            ));
        }
        drop(rows);
        drop(statement);

        for (what, sql) in checks {
            let found = self.count(&sql)?;
            if found > 0 {
                problems.push(format!("{found} {what}"));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Verify(problems))
        }
    }
}
