//! The engine: one type that owns the store and is the whole API.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use duckdb::arrow::record_batch::{RecordBatch, RecordBatchReader};
use duckdb::vtab::arrow::ArrowBatchRegistration;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use spool_core::{
    Changeset, ChangesetId, Clock, Committed, Draft, EntityType, EvidenceSummary, IdGenerator,
    IdentityId, IdentityLink, IdentityStatus, IdentityStore, LogEntry, MemoryState, NewObservation,
    NewPair, ObservationId, Policy, RandomIds, ReviewItem, RunId, SeededIds, Seq, Snapshot,
    StepClock, SystemClock, Verdict, reconcile, validate,
};

use crate::drafts::{self, DraftFilter, DraftRecord, DraftStatus, DraftSummary};
use crate::error::{Error, Result};
use crate::features;
use crate::ingest::{self, IngestOptions, IngestReport};
use crate::lookup::{
    self, HistoryEntry, IdentityFilter, IdentitySummary, IdentityView, ObservationFilter,
    ObservationRecord, StoreInfo,
};
use crate::metrics::{self, IdStability, Metrics};
use crate::project::{EntityTypeManifest, Manifest, hash_json};
use crate::review::{self, ReviewFilter, ReviewRecord};
use crate::rows::Rows;
use crate::rules;
use crate::runs::{self, RunFilter, RunRecord, RunStatus};
use crate::schema;
use crate::store::{self, DuckStore};

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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// Store each type's draft for review instead of committing it.
    pub leave_for_review: bool,
}

/// What becomes of the draft a reconcile run writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Commit it at once.
    Commit,
    /// Store it. It waits until it is approved or rejected, or until a later
    /// run of the entity type drafts again.
    LeaveForReview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApplyReport {
    /// Entity types declared by this call.
    pub declared: Vec<EntityType>,
    /// Entity types the store already held with the same declaration.
    pub unchanged: Vec<EntityType>,
    /// Entity types whose declaration changed. Their feature tables were
    /// computed again from the stored observations.
    #[serde(default)]
    pub rebuilt: Vec<EntityType>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ResolveReport {
    /// One entry per entity type that had new observations.
    pub resolved: Vec<TypeResolution>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// The changeset, once it is committed. `None` when there was nothing to
    /// commit, and while a draft waits.
    pub committed: Option<Committed>,
    /// The draft that was stored for review, if the run was asked to leave
    /// one. It is named by its changeset ID.
    #[serde(default)]
    pub draft: Option<ChangesetId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JudgementCounts {
    #[serde(rename = "match")]
    pub match_: u64,
    pub no_match: u64,
    pub unsure: u64,
}

/// IDs minted for a caller that writes its own changesets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MintedIds {
    pub identities: Vec<IdentityId>,
    pub changesets: Vec<ChangesetId>,
}

type Ids = Arc<Mutex<Box<dyn IdGenerator + Send>>>;

/// The engine. It owns the database file, and every change to identity state
/// goes through [`commit`](Engine::commit).
///
/// Calls that write are serialised: one connection does all the writing,
/// behind a lock. Calls that only read use connections of their own and see
/// what has been committed, so they do not wait for a run in progress.
pub struct Engine {
    inner: Mutex<Inner>,
    readers: Readers,
    ids: Ids,
    manifest: RwLock<Option<Arc<Manifest>>>,
}

struct Inner {
    store: DuckStore,
    ids: Ids,
    manifest: Option<Arc<Manifest>>,
}

/// Connections for reading, made as they are needed and kept for the next
/// read.
struct Readers {
    attached: bool,
    source: Mutex<Connection>,
    idle: Mutex<Vec<Connection>>,
}

impl Readers {
    fn with<T>(&self, read: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let idle = lock(&self.idle).pop();
        let conn = match idle {
            Some(conn) => conn,
            None => store::reader(&lock(&self.source), self.attached)?,
        };
        let result = read(&conn);
        lock(&self.idle).push(conn);
        result
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

type RunWatcher = Box<dyn FnMut(RunId)>;

thread_local! {
    static RUN_WATCHER: RefCell<Option<RunWatcher>> = const { RefCell::new(None) };
}

/// Calls `started` with the ID of each run the engine starts on this thread
/// while `work` runs, as soon as the run is recorded and before its work is
/// done. A server uses this to hand a caller the ID of a run that is still in
/// progress.
pub fn watch_runs<T>(started: impl FnMut(RunId) + 'static, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<RunWatcher>);
    impl Drop for Restore {
        fn drop(&mut self) {
            RUN_WATCHER.with(|watcher| *watcher.borrow_mut() = self.0.take());
        }
    }
    let previous = RUN_WATCHER.with(|watcher| watcher.borrow_mut().replace(Box::new(started)));
    let _restore = Restore(previous);
    work()
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
    status: RunStatus,
    config: Option<String>,
}

/// What is recorded as the error of a run that was `running` when the store
/// was opened.
const INTERRUPTED: &str = "interrupted: the engine stopped while the run was in progress";

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

    fn with_store(mut store: DuckStore, ids: Box<dyn IdGenerator + Send>) -> Result<Self> {
        // Nothing else has this store open, so a run that is still marked as
        // running was cut short. The clock is read only if there is one.
        let interrupted: i64 = store.conn.query_row(
            "SELECT count(*) FROM spool.run WHERE status = 'running'",
            [],
            |row| row.get(0),
        )?;
        if interrupted > 0 {
            let now = store.clock.now_micros();
            store.conn.execute(
                "UPDATE spool.run
                 SET status = 'failed', finished_at = make_timestamptz(?), error = ?
                 WHERE status = 'running'",
                params![now, INTERRUPTED],
            )?;
        }
        let readers = Readers {
            attached: store.attached(),
            source: Mutex::new(store.reader()?),
            idle: Mutex::new(Vec::new()),
        };
        let ids: Ids = Arc::new(Mutex::new(ids));
        Ok(Self {
            inner: Mutex::new(Inner {
                store,
                ids: Arc::clone(&ids),
                manifest: None,
            }),
            readers,
            ids,
            manifest: RwLock::new(None),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }

    fn read<T>(&self, read: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        self.readers.with(read)
    }

    fn set_manifest(&self, manifest: Option<Arc<Manifest>>) {
        *self
            .manifest
            .write()
            .unwrap_or_else(PoisonError::into_inner) = manifest;
    }

    /// Declares the manifest's entity types in the store, generating a
    /// feature table for each new one, and makes the manifest's rules the
    /// ones this engine runs. Applying the same manifest again changes
    /// nothing in the store.
    ///
    /// A type whose declaration differs from the one in the store gets a new
    /// declaration version, and its feature table is computed again from the
    /// stored observations. Identities and the log are not touched. Match
    /// runs of the type that were waiting for judgements are failed, because
    /// their candidates were generated from the old features.
    pub fn apply(&self, manifest: &Manifest) -> Result<ApplyReport> {
        let mut inner = self.lock();
        let report = inner.apply(manifest)?;
        self.set_manifest(inner.manifest.clone());
        Ok(report)
    }

    /// Makes `manifest` the one this engine runs on without changing the
    /// store. Refused unless it is the manifest the store was last applied
    /// with: a server must not run rules against feature tables built for
    /// another declaration.
    pub fn adopt(&self, manifest: &Manifest) -> Result<()> {
        let mut inner = self.lock();
        let applied = schema::meta(&inner.store.conn, "manifest_hash")?;
        if applied.as_deref() != Some(manifest.hash.as_str()) {
            return Err(Error::ManifestMismatch {
                applied,
                manifest: manifest.hash.clone(),
            });
        }
        inner.manifest = Some(Arc::new(manifest.clone()));
        self.set_manifest(inner.manifest.clone());
        Ok(())
    }

    /// The manifest this engine runs on, if one was applied or adopted.
    pub fn manifest(&self) -> Option<Arc<Manifest>> {
        self.manifest
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The hash of the manifest the store was last applied with.
    pub fn applied_manifest_hash(&self) -> Result<Option<String>> {
        self.read(|conn| schema::meta(conn, "manifest_hash"))
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
    ///
    /// The run is `open`: it takes judgements until it is reconciled, and it
    /// stays open across a restart.
    pub fn begin_run(&self, strategy: &StrategyRef, scope: &Scope) -> Result<RunId> {
        self.lock().begin_match(strategy, scope, RunStatus::Open)
    }

    /// A run's candidate pairs with both sides' features: `left_id`,
    /// `right_id`, then every feature column prefixed with `left_` and with
    /// `right_`.
    pub fn candidates(&self, run: RunId) -> Result<Vec<RecordBatch>> {
        Ok(self.candidate_rows(run)?.into_batches())
    }

    /// The same as [`candidates`](Self::candidates), with the schema, which
    /// is known even when there are no pairs.
    pub fn candidate_rows(&self, run: RunId) -> Result<Rows> {
        self.read(|conn| candidates(conn, run))
    }

    /// Adds judgements to an open match run and returns how many were
    /// stored. The rows need `left_id`, `right_id` and `verdict`, and may
    /// have `score` and `detail`.
    ///
    /// The whole submission is refused if a row names an observation the
    /// store does not hold as this run's entity type, or a pair in which
    /// neither observation is new to the run.
    pub fn submit_judgements(&self, run: RunId, rows: impl RecordBatchReader) -> Result<u64> {
        self.lock().submit_judgements(run, rows)
    }

    /// Turns a match run's judgements into a draft changeset and leaves it
    /// for review. Nothing is committed. What the draft leaves out is stored
    /// as review items, which stay open until their observations have an
    /// identity.
    ///
    /// The same as [`reconcile_with`](Self::reconcile_with) and
    /// [`Disposition::LeaveForReview`], returning the draft itself.
    pub fn reconcile(&self, run: RunId, policy: &Policy) -> Result<Draft> {
        let (draft, _) = self
            .lock()
            .reconcile(run, policy, Disposition::LeaveForReview)?;
        Ok(draft)
    }

    /// Turns a match run's judgements into a draft changeset, and commits the
    /// draft or stores it for review.
    ///
    /// A stored draft is named by its changeset ID. The run is `drafted`
    /// until the draft is committed, and only then has it succeeded: what is
    /// new to the next run is counted from the last run that succeeded, so
    /// the observations of a draft nobody commits are examined again. A run
    /// whose draft is empty has succeeded at once.
    ///
    /// Drafting an entity type closes its earlier open drafts as superseded,
    /// so a type has at most one draft waiting. A drafted run can be
    /// reconciled again, for example under another policy.
    pub fn reconcile_with(
        &self,
        run: RunId,
        policy: &Policy,
        then: Disposition,
    ) -> Result<TypeResolution> {
        let (_, report) = self.lock().reconcile(run, policy, then)?;
        Ok(report)
    }

    /// Gives up an open match run. It is recorded as failed, and its
    /// observations are still new to the next run.
    pub fn cancel_run(&self, run: RunId) -> Result<()> {
        self.lock().cancel_run(run)
    }

    /// Validates a changeset against current state and commits it. This is
    /// the only way identity state changes, whoever wrote the changeset.
    ///
    /// Committing the changeset of a stored draft approves the draft.
    pub fn commit(&self, changeset: Changeset) -> Result<Committed> {
        self.lock().commit(changeset)
    }

    /// Commits a stored draft. It is validated like any changeset: if state
    /// has moved under it, the rejections name the operations that no longer
    /// hold, and the draft stays open.
    pub fn approve_draft(&self, draft: ChangesetId) -> Result<Committed> {
        self.lock().approve_draft(draft)
    }

    /// Closes a stored draft without committing it. Its match run has not
    /// succeeded, so the next run examines the same observations again.
    pub fn reject_draft(&self, draft: ChangesetId, reason: Option<&str>) -> Result<DraftSummary> {
        self.lock().reject_draft(draft, reason)
    }

    /// A stored draft with its changeset.
    pub fn draft(&self, draft: ChangesetId) -> Result<DraftRecord> {
        self.read(|conn| drafts::get(conn, draft))?
            .ok_or(Error::UnknownDraft(draft))
    }

    /// Stored drafts without their operations, newest first.
    pub fn drafts(&self, filter: &DraftFilter) -> Result<Vec<DraftSummary>> {
        self.read(|conn| drafts::list(conn, filter))
    }

    /// Matches new observations with the built-in rule runner, reconciles
    /// with the default policy, and commits or leaves a draft for review. A
    /// type is resolved when it has newly ingested observations or open
    /// review items.
    ///
    /// The match run is recorded as succeeded only after the commit. A
    /// resolve that is cut short leaves the run unfinished, and the next
    /// resolve examines the same observations again.
    pub fn resolve(&self, options: &ResolveOptions) -> Result<ResolveReport> {
        self.lock().resolve(options)
    }

    /// A run as the store records it: the task record of whatever started
    /// it.
    pub fn run(&self, run: RunId) -> Result<RunRecord> {
        self.read(|conn| runs::get(conn, run))?
            .ok_or(Error::UnknownRun(run))
    }

    /// Runs, newest first.
    pub fn runs(&self, filter: &RunFilter) -> Result<Vec<RunRecord>> {
        self.read(|conn| runs::list(conn, filter))
    }

    /// Review items, oldest first.
    pub fn review_items(&self, filter: &ReviewFilter) -> Result<Vec<ReviewRecord>> {
        self.read(|conn| review::list(conn, filter))
    }

    pub fn identity_of(&self, observation: ObservationId) -> Result<Option<IdentityId>> {
        Ok(self.identities_of(&[observation])?[0])
    }

    /// The identity of each observation, in the order asked. `None` for an
    /// observation that has no identity or that the store does not hold.
    pub fn identities_of(&self, observations: &[ObservationId]) -> Result<Vec<Option<IdentityId>>> {
        self.read(|conn| store::read_assignments(conn, observations))
    }

    /// The active identity an ID stands for: itself, or the identity it was
    /// merged into.
    pub fn current_id(&self, id: IdentityId) -> Result<IdentityId> {
        match self
            .read(|conn| store::read_identities(conn, &[id]))?
            .remove(0)
        {
            None => Err(Error::UnknownIdentity(id)),
            Some(record) => Ok(match record.status {
                IdentityStatus::Active => id,
                IdentityStatus::MergedInto(survivor) => survivor,
            }),
        }
    }

    /// An identity with the observations that belong to it now.
    pub fn identity(&self, id: IdentityId) -> Result<IdentityView> {
        self.read(|conn| lookup::identity(conn, id))?
            .ok_or(Error::UnknownIdentity(id))
    }

    /// Active identities, ordered by ID.
    pub fn identities(&self, filter: &IdentityFilter) -> Result<Vec<IdentitySummary>> {
        self.read(|conn| lookup::identities(conn, filter))
    }

    /// The changesets that name an identity, oldest first.
    pub fn history(&self, id: IdentityId) -> Result<Vec<HistoryEntry>> {
        self.read(|conn| {
            if store::read_identities(conn, &[id])?.remove(0).is_none() {
                return Err(Error::UnknownIdentity(id));
            }
            lookup::history(conn, id)
        })
    }

    /// An observation as it was ingested, or `None` if the store does not
    /// hold it.
    pub fn observation(&self, id: ObservationId) -> Result<Option<ObservationRecord>> {
        self.read(|conn| lookup::observation(conn, id))
    }

    /// Observations, ordered by ID. A caller that knows a row by its source
    /// and key finds its observation IDs here: only the engine computes them.
    pub fn observations(&self, filter: &ObservationFilter) -> Result<Vec<ObservationRecord>> {
        self.read(|conn| lookup::observations(conn, filter))
    }

    /// Mints IDs for a caller that writes its own changesets.
    pub fn mint_ids(&self, identities: usize, changesets: usize) -> MintedIds {
        let mut ids = lock(&self.ids);
        MintedIds {
            identities: (0..identities).map(|_| ids.identity_id()).collect(),
            changesets: (0..changesets).map(|_| ids.changeset_id()).collect(),
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
        self.read(|conn| {
            let mut statement = conn
                .prepare(&format!("SELECT * FROM (\n{sql}\n)"))
                .map_err(|error| Error::Query(error.to_string()))?;
            Ok(statement.query_arrow([])?.collect())
        })
    }

    /// The position of the last committed changeset.
    pub fn head(&self) -> Result<Seq> {
        self.read(store::read_head)
    }

    /// Every committed changeset, in log order.
    pub fn log(&self) -> Result<Vec<LogEntry>> {
        self.read(store::read_log)
    }

    /// At most `limit` committed changesets after a log position, in log
    /// order.
    pub fn log_after(&self, after: Seq, limit: u64) -> Result<Vec<LogEntry>> {
        self.read(|conn| store::read_log_after(conn, after, limit))
    }

    /// All of identity state, read from the projection tables.
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.read(store::read_snapshot)
    }

    /// What the store holds, per entity type.
    pub fn info(&self) -> Result<StoreInfo> {
        let manifest = self.manifest();
        self.read(|conn| lookup::info(conn, manifest.as_deref()))
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
        self.read(|conn| metrics::id_stability(conn, before, after))
    }
}

/// The candidate pairs of a run with both sides' features.
fn candidates(conn: &Connection, run: RunId) -> Result<Rows> {
    let (_, config) = match_run(conn, run)?;
    let table = features::table(&config.entity_type);
    let exclude = "EXCLUDE (observation_id, declaration_version)";
    let has_features: bool = conn.query_row(
        &format!("SELECT count(*) > 2 FROM (DESCRIBE {table})"),
        [],
        |row| row.get(0),
    )?;
    let features = if has_features {
        format!(", COLUMNS(l.* {exclude}) AS \"left_\\0\", COLUMNS(r.* {exclude}) AS \"right_\\0\"")
    } else {
        String::new()
    };
    let mut statement = conn.prepare(&format!(
        "SELECT c.left_id, c.right_id{features}
         FROM (SELECT DISTINCT left_id, right_id FROM spool.candidate WHERE run_id = ?) c
         JOIN {table} l ON l.observation_id = c.left_id
         JOIN {table} r ON r.observation_id = c.right_id
         ORDER BY c.left_id, c.right_id"
    ))?;
    let arrow = statement.query_arrow([run.0 as i64])?;
    let schema = arrow.get_schema();
    Ok(Rows::new(schema, arrow.collect()))
}

fn run_row(conn: &Connection, run: RunId) -> Result<RunRow> {
    let mut statement =
        conn.prepare("SELECT kind, status, config::VARCHAR FROM spool.run WHERE run_id = ?")?;
    let mut rows = statement.query([run.0 as i64])?;
    match rows.next()? {
        Some(row) => Ok(RunRow {
            kind: row.get(0)?,
            status: RunStatus::from_stored(&row.get::<_, String>(1)?)?,
            config: row.get(2)?,
        }),
        None => Err(Error::UnknownRun(run)),
    }
}

fn match_run(conn: &Connection, run: RunId) -> Result<(RunRow, MatchConfig)> {
    let row = run_row(conn, run)?;
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
            rebuilt: Vec::new(),
        };
        let tx = self.store.conn.transaction()?;
        for entity_type in &manifest.entity_types {
            let name = &entity_type.name;
            let stored: Option<(i32, String)> = {
                let mut statement = tx.prepare(
                    "SELECT version, declaration_hash FROM spool.entity_type
                     WHERE entity_type = ? ORDER BY version DESC LIMIT 1",
                )?;
                let mut rows = statement.query([name.as_str()])?;
                match rows.next()? {
                    Some(row) => Some((row.get(0)?, row.get(1)?)),
                    None => None,
                }
            };
            let version = match stored {
                Some((_, hash)) if hash == entity_type.declaration_hash => {
                    report.unchanged.push(name.clone());
                    continue;
                }
                Some((version, _)) => version as u32 + 1,
                None => 1,
            };
            tx.execute(
                "INSERT INTO spool.entity_type VALUES (?, ?, ?, ?, make_timestamptz(?))",
                params![
                    name.as_str(),
                    version as i32,
                    json!(entity_type.declaration).to_string(),
                    entity_type.declaration_hash,
                    now
                ],
            )?;
            if version == 1 {
                features::create_table(&tx, name, &entity_type.declaration, version)?;
                report.declared.push(name.clone());
                continue;
            }

            // The declaration changed. The payloads are all still there, so
            // the feature table is computed again from them.
            features::drop_table(&tx, name)?;
            features::create_table(&tx, name, &entity_type.declaration, version)?;
            let problem = features::fill(
                &tx,
                name,
                &entity_type.declaration,
                version,
                &features::of_type(name),
            )?;
            if let Some(problem) = problem {
                return Err(Error::Apply(problem));
            }
            // Candidates of a run that waits for judgements were generated
            // from the old features, and its judgements would be about them.
            tx.execute(
                "UPDATE spool.run
                 SET status = 'failed', finished_at = make_timestamptz(?), error = ?
                 WHERE kind = 'match' AND status = 'open'
                   AND json_extract_string(config, '$.entity_type') = ?",
                params![
                    now,
                    format!("the declaration of `{name}` changed while the run was open"),
                    name.as_str()
                ],
            )?;
            report.rebuilt.push(name.clone());
        }
        schema::set_meta(&tx, "manifest_hash", &manifest.hash)?;
        tx.commit()?;
        self.manifest = Some(Arc::new(manifest.clone()));
        Ok(report)
    }

    /// The applied manifest's entry for a type, with the version the store
    /// holds for it.
    fn declared(&self, entity_type: &str) -> Result<(&Manifest, &EntityTypeManifest, u32)> {
        let manifest = self.manifest.as_deref().ok_or(Error::NoManifest)?;
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
             VALUES (?, ?, 'running', ?, ?, ?, ?, ?, make_timestamptz(?), NULL, NULL, NULL)",
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
        let run = RunId(run as u64);
        // The run is recorded, so whoever asked for it can be told its ID
        // while the work goes on.
        RUN_WATCHER.with(|watcher| {
            if let Some(started) = watcher.borrow_mut().as_mut() {
                started(run);
            }
        });
        Ok(run)
    }

    /// Records a run as finished, with what it reported.
    fn finish_run(
        &mut self,
        run: RunId,
        status: RunStatus,
        result: Option<&serde_json::Value>,
    ) -> Result<()> {
        let now = self.store.clock.now_micros();
        self.store.conn.execute(
            "UPDATE spool.run
             SET status = ?, finished_at = make_timestamptz(?), result = ?::JSON
             WHERE run_id = ?",
            params![
                status.as_str(),
                now,
                result.map(ToString::to_string),
                run.0 as i64
            ],
        )?;
        Ok(())
    }

    fn fail_run(&mut self, run: RunId, error: &str) -> Result<()> {
        let now = self.store.clock.now_micros();
        self.store.conn.execute(
            "UPDATE spool.run SET status = 'failed', finished_at = make_timestamptz(?), error = ?
             WHERE run_id = ?",
            params![now, error, run.0 as i64],
        )?;
        Ok(())
    }

    /// Runs the work of a run, and records the run as failed if it fails.
    fn in_run<T>(&mut self, run: RunId, work: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let result = work(self);
        if let Err(error) = &result {
            // The first error is the one worth reporting.
            let _ = self.fail_run(run, &error.to_string());
        }
        result
    }

    fn match_run(&self, run: RunId) -> Result<(RunRow, MatchConfig)> {
        match_run(&self.store.conn, run)
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

            let problem = features::fill(
                &tx,
                &name,
                &declaration,
                version,
                &features::of_run(&name, run),
            )?;
            if let Some(problem) = problem {
                return Err(Error::Ingest(problem));
            }
            let report = IngestReport {
                run,
                received,
                inserted,
                duplicates: received - inserted,
            };
            let finished = inner.store.clock.now_micros();
            tx.execute(
                "UPDATE spool.run
                 SET status = 'succeeded', finished_at = make_timestamptz(?), result = ?::JSON
                 WHERE run_id = ?",
                params![finished, json!(report).to_string(), run.0 as i64],
            )?;
            tx.commit()?;
            Ok(report)
        })
    }

    /// Starts a match run and generates its candidates. A run that waits for
    /// someone else's judgements becomes `open` together with its candidates;
    /// a run the engine drives stays `running` until it is concluded.
    fn begin_match(
        &mut self,
        strategy: &StrategyRef,
        scope: &Scope,
        then: RunStatus,
    ) -> Result<RunId> {
        let (manifest, entry, version) = self.declared(scope.entity_type.as_str())?;
        let entity_type = entry.name.clone();
        let manifest_hash = manifest.hash.clone();
        let rules = entry.rules.clone();
        let after_run = self.last_match_run(&entity_type)?;
        let review_items = review::open_ids(&self.store.conn, &entity_type)?;
        let config = MatchConfig {
            entity_type: entity_type.clone(),
            declaration_version: version,
            manifest_hash,
            after_run,
            review_items,
            rules,
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
            if then != RunStatus::Running {
                tx.execute(
                    "UPDATE spool.run SET status = ? WHERE run_id = ?",
                    params![then.as_str(), run.0 as i64],
                )?;
            }
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

    /// Recreates the scope table of a match run that still takes judgements:
    /// from callers while it is `open`, from the engine's own strategy while
    /// it is `running`.
    fn judged_run(&self, run: RunId, expected: RunStatus) -> Result<MatchConfig> {
        let (row, config) = self.match_run(run)?;
        if row.status != expected {
            return Err(Error::Run {
                run,
                message: format!("it is {} and takes no judgements", row.status),
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

    fn submit_judgements(&mut self, run: RunId, rows: impl RecordBatchReader) -> Result<u64> {
        let config = self.judged_run(run, RunStatus::Open)?;
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
        let config = self.judged_run(run, RunStatus::Running)?;
        rules::judge_candidates(
            &self.store.conn,
            &config.entity_type,
            run,
            &config.rules.no_match,
            &config.rules.match_,
        )
    }

    fn reconcile(
        &mut self,
        run: RunId,
        policy: &Policy,
        then: Disposition,
    ) -> Result<(Draft, TypeResolution)> {
        let (row, _) = self.match_run(run)?;
        if !matches!(row.status, RunStatus::Open | RunStatus::Drafted) {
            return Err(Error::Run {
                run,
                message: format!("it is {} and cannot be reconciled", row.status),
            });
        }
        self.conclude(run, policy, then)
    }

    /// Turns a match run's judgements into a draft, stores its review items,
    /// and commits the draft or stores it.
    ///
    /// The match run has succeeded once its draft is committed, or when there
    /// is nothing to commit, and not before: what is new to the next run is
    /// counted from the last run that succeeded. If the process stops between
    /// the commit and the line that records it, the next run examines the
    /// same observations again, and those the commit placed are no longer
    /// new.
    fn conclude(
        &mut self,
        run: RunId,
        policy: &Policy,
        then: Disposition,
    ) -> Result<(Draft, TypeResolution)> {
        let (_, config) = self.match_run(run)?;
        let entity_type = config.entity_type.clone();

        let reconcile_run = self.start_run(
            "reconcile",
            None,
            &json!({ "match_run": run, "policy": policy }),
        )?;
        let (draft, mut report) = self.in_run(reconcile_run, |inner| {
            let evidence = inner.evidence(run, &config)?;
            let new_observations = evidence.observations.len() as u64;
            let reexamined = inner.count(&format!(
                "SELECT count(*) FROM {} s
                 JOIN spool.observation o ON o.observation_id = s.observation_id
                 WHERE o.ingest_run <= {}",
                rules::SCOPE,
                config.after_run
            ))?;
            let mut draft = {
                let mut ids = lock(&inner.ids);
                reconcile(&evidence, &inner.store, policy, ids.as_mut())?
            };
            draft.changeset.run = Some(reconcile_run);

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
            let held = then == Disposition::LeaveForReview && !draft.changeset.ops.is_empty();
            let report = TypeResolution {
                entity_type: entity_type.clone(),
                match_run: run,
                reconcile_run,
                new_observations,
                reexamined,
                candidates: inner.count(&format!(
                    "SELECT count(*) FROM (SELECT DISTINCT left_id, right_id FROM spool.candidate {run_filter})"
                ))?,
                judgements: JudgementCounts {
                    match_: judged(inner, "match")?,
                    no_match: judged(inner, "no_match")?,
                    unsure: judged(inner, "unsure")?,
                },
                identities_created,
                observations_assigned,
                observations_unassigned: new_observations - observations_assigned,
                review: draft.review.clone(),
                committed: None,
                draft: held.then_some(draft.changeset.id),
            };

            let finished = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            review::record(&tx, &entity_type, reconcile_run, &draft.review)?;
            // Whatever this run drafts, an earlier draft of the type was
            // written against a state this one may change.
            drafts::supersede(&tx, &entity_type, reconcile_run)?;
            if then == Disposition::LeaveForReview {
                let result = json!(report).to_string();
                if held {
                    drafts::store(&tx, &entity_type, run, &draft.changeset)?;
                    tx.execute(
                        "UPDATE spool.run
                         SET status = 'drafted', finished_at = NULL, result = ?::JSON
                         WHERE run_id = ?",
                        params![result, run.0 as i64],
                    )?;
                } else {
                    tx.execute(
                        "UPDATE spool.run
                         SET status = 'succeeded', finished_at = make_timestamptz(?),
                             result = ?::JSON
                         WHERE run_id = ?",
                        params![finished, result, run.0 as i64],
                    )?;
                }
            }
            tx.execute(
                "UPDATE spool.run SET status = 'succeeded', finished_at = make_timestamptz(?)
                 WHERE run_id = ?",
                params![finished, reconcile_run.0 as i64],
            )?;
            tx.commit()?;
            Ok((draft, report))
        })?;

        if then == Disposition::Commit {
            if !draft.changeset.ops.is_empty() {
                let validated = validate(&self.store, draft.changeset.clone())?;
                report.committed = Some(self.store.apply(validated)?);
            }
            self.finish_run(run, RunStatus::Succeeded, Some(&json!(report)))?;
        }
        Ok((draft, report))
    }

    fn cancel_run(&mut self, run: RunId) -> Result<()> {
        let (row, _) = self.match_run(run)?;
        if row.status != RunStatus::Open {
            return Err(Error::Run {
                run,
                message: format!("it is {} and cannot be cancelled", row.status),
            });
        }
        self.fail_run(run, "cancelled")
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
        // A changeset that carries the ID of a stored draft is that draft or
        // it is refused: the ID is what a reviewer approved.
        if let Some(record) = drafts::get(&self.store.conn, changeset.id)? {
            let refuse = |message: String| Error::Draft {
                draft: changeset.id,
                message,
            };
            match record.draft.status {
                DraftStatus::Open if record.changeset != changeset => {
                    return Err(refuse(
                        "the changeset is not the one stored as this draft".to_owned(),
                    ));
                }
                DraftStatus::Rejected | DraftStatus::Superseded => {
                    return Err(refuse(format!(
                        "it was {} and cannot be committed",
                        record.draft.status
                    )));
                }
                // A committed draft is in the log, and the validator says so.
                DraftStatus::Open | DraftStatus::Committed => {}
            }
        }
        let validated = validate(&self.store, changeset)?;
        Ok(self.store.apply(validated)?)
    }

    fn open_draft(&self, draft: ChangesetId, verb: &str) -> Result<DraftRecord> {
        let record = drafts::get(&self.store.conn, draft)?.ok_or(Error::UnknownDraft(draft))?;
        if record.draft.status != DraftStatus::Open {
            return Err(Error::Draft {
                draft,
                message: format!("it is {} and cannot be {verb}", record.draft.status),
            });
        }
        Ok(record)
    }

    fn approve_draft(&mut self, draft: ChangesetId) -> Result<Committed> {
        let record = self.open_draft(draft, "approved")?;
        let validated = validate(&self.store, record.changeset)?;
        Ok(self.store.apply(validated)?)
    }

    fn reject_draft(&mut self, draft: ChangesetId, reason: Option<&str>) -> Result<DraftSummary> {
        self.open_draft(draft, "rejected")?;
        let now = self.store.clock.now_micros();
        drafts::reject(&self.store.conn, draft, now, reason)?;
        Ok(drafts::get(&self.store.conn, draft)?
            .ok_or(Error::UnknownDraft(draft))?
            .draft)
    }

    fn resolve(&mut self, options: &ResolveOptions) -> Result<ResolveReport> {
        let manifest = self.manifest.as_deref().ok_or(Error::NoManifest)?;
        let entity_types: Vec<EntityType> = match &options.entity_type {
            Some(entity_type) => vec![self.declared(entity_type.as_str())?.1.name.clone()],
            None => manifest
                .entity_types
                .iter()
                .map(|entry| entry.name.clone())
                .collect(),
        };
        let then = if options.leave_for_review {
            Disposition::LeaveForReview
        } else {
            Disposition::Commit
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
            let run = self.begin_match(&StrategyRef::rules(), &scope, RunStatus::Running)?;
            let (_, resolution) = self.in_run(run, |inner| {
                inner.judge_with_rules(run)?;
                inner.conclude(run, &Policy::default(), then)
            })?;
            report.resolved.push(resolution);
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
        checks.extend([
            (
                "entity types with more than one open draft".to_owned(),
                "SELECT count(*) FROM (
                   SELECT entity_type FROM spool.draft WHERE status = 'open'
                   GROUP BY 1 HAVING count(*) > 1)"
                    .to_owned(),
            ),
            (
                "open drafts whose match run is not waiting on them".to_owned(),
                "SELECT count(*) FROM spool.draft d
                 LEFT JOIN spool.run r ON r.run_id = d.match_run
                 WHERE d.status = 'open' AND r.status IS DISTINCT FROM 'drafted'"
                    .to_owned(),
            ),
            (
                "committed drafts that are not in the log".to_owned(),
                "SELECT count(*) FROM spool.draft d
                 WHERE d.status = 'committed' AND NOT EXISTS (
                   SELECT 1 FROM spool.changeset c
                   WHERE c.seq = d.closed_seq AND c.changeset_id = d.changeset_id)"
                    .to_owned(),
            ),
        ]);
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
