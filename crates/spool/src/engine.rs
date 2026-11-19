//! The engine: one type that owns the store and is the whole API.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use duckdb::arrow::record_batch::{RecordBatch, RecordBatchReader};
use duckdb::vtab::arrow::ArrowBatchRegistration;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use spool_core::{
    Actor, Changeset, ChangesetId, Clock, Committed, ConstraintEntry, ConstraintKind, DocumentId,
    Draft, EntityType, EvidenceSummary, IdGenerator, IdentityId, IdentityLink, IdentityState,
    IdentityStatus, IdentityStore, LogEntry, MemoryState, NewPair, NewRecord, Op, Policy,
    RandomIds, RecordId, ReviewItem, RunId, SeededIds, Seq, Snapshot, StepClock, SystemClock,
    ValidationError, Verdict, inverse, reconcile, validate,
};

use crate::attributes::{AttributeFilter, AttributeGroup};
use crate::drafts::{self, DraftEntry, DraftFilter, DraftStatus, DraftSummary};
use crate::error::{Error, Result};
use crate::explain::{self, Explanation, IdentityExplanation};
use crate::features;
use crate::following::following;
use crate::load::{self, LoadReport};
use crate::lookup::{
    self, ChangesetSummary, HistoryEntry, IdentityFilter, IdentitySummary, IdentityView, LogSort,
    LoggedChangeset, RecordEntry, RecordFilter, StoreInfo,
};
use crate::metrics::{self, Churn, IdStability, Metrics};
use crate::order::Sort;
use crate::possible_split;
use crate::project::{EntityTypeManifest, Manifest, Strategy, hash_json};
use crate::recompute::{self, AppliedType, RecomputeApplied, RecomputeReport, RecomputeRequest};
use crate::retract::{self, RetractReport};
use crate::review::{self, ReviewCase, ReviewEntry, ReviewFilter};
use crate::rows::Rows;
use crate::rules;
use crate::runs::{self, RunEntry, RunFilter, RunStatus};
use crate::schema;
use crate::store::{self, DuckStore};
use crate::usercode::{self, Task, TaskInfo, UserCode};

mod acting;
mod extract;

pub use acting::Acting;

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

/// Which records a match run treats as new: those of one entity type
/// loaded since the last match run of that type that succeeded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub entity_type: EntityType,
    /// Examine every record of the type again, not only the new ones.
    /// See [`ResolveOptions::rematch`].
    pub rematch: bool,
}

impl Scope {
    /// The records of a type that are new.
    pub fn new(entity_type: EntityType) -> Self {
        Self {
            entity_type,
            rematch: false,
        }
    }

    /// Every record of a type.
    pub fn rematch(entity_type: EntityType) -> Self {
        Self {
            entity_type,
            rematch: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolveOptions {
    /// Resolve only this entity type. `None` resolves every declared type.
    pub entity_type: Option<EntityType>,
    /// Store each type's draft for review instead of committing it.
    pub leave_for_review: bool,
    /// The strategy that judges: `rules`, or the name of one of the
    /// project's user-code strategies. `None` is `rules`. Every type that is
    /// resolved must have it.
    pub strategy: Option<String>,
    /// Match every record again, not only the new ones, which is what
    /// a new strategy or a changed rule calls for.
    ///
    /// A rematch changes nothing about a record that has an identity:
    /// judgements never merge or split existing identities. Where the
    /// strategy disagrees with the identities as they are, the run reports
    /// it as [`Suggestions`]. Records without an identity are placed
    /// as in any run.
    pub rematch: bool,
}

impl ResolveOptions {
    /// Resolves with the named strategy.
    pub fn with_strategy(strategy: impl Into<String>) -> Self {
        Self {
            strategy: Some(strategy.into()),
            ..Self::default()
        }
    }
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
    /// computed again from the stored records.
    #[serde(default)]
    pub rebuilt: Vec<EntityType>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ResolveReport {
    /// One entry per entity type that had new records.
    pub resolved: Vec<TypeResolution>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TypeResolution {
    pub entity_type: EntityType,
    pub match_run: RunId,
    pub reconcile_run: RunId,
    /// Records the run examined: those loaded since the last run, and
    /// those an earlier run left for review.
    pub new_records: u64,
    /// How many of them an earlier run had left for review.
    pub reexamined: u64,
    /// Distinct candidate pairs.
    pub candidates: u64,
    pub judgements: JudgementCounts,
    pub identities_created: u64,
    pub records_assigned: u64,
    /// Earlier versions of the rows the run placed, moved along with them
    /// from another identity or from none. They are not counted in
    /// `records_assigned`.
    #[serde(default)]
    pub versions_moved: u64,
    /// Examined records left without an identity. Each is named in
    /// `review`.
    pub records_unassigned: u64,
    /// What the run would not decide. The store keeps these as review items.
    pub review: Vec<ReviewItem>,
    /// The changeset, once it is committed. `None` when there was nothing to
    /// commit, and while a draft waits.
    pub committed: Option<Committed>,
    /// The draft that was stored for review, if the run was asked to leave
    /// one. It is named by its changeset ID.
    #[serde(default)]
    pub draft: Option<ChangesetId>,
    /// Where the strategy disagrees with the identities as they are. Only a
    /// rematch has these: no other run judges pairs of records that
    /// both have an identity.
    #[serde(default)]
    pub suggestions: Option<Suggestions>,
}

/// What a rematch would change about existing identities if judgements were
/// allowed to. Nothing here was applied: a reviewer who agrees merges or
/// splits with a changeset.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Suggestions {
    /// Pairs of identities with a `match` between their members, ordered by
    /// their IDs.
    pub merges: Vec<SuggestedMerge>,
    /// Identities with a `no_match` between two of their members, ordered
    /// by ID.
    pub splits: Vec<SuggestedSplit>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SuggestedMerge {
    /// The two identities, the smaller ID first.
    pub identities: [IdentityId; 2],
    /// Pairs of their members the strategy judged `match`.
    pub matches: u64,
    /// Pairs of their members the strategy judged `no_match`. More than
    /// none means the strategy contradicts itself about these two.
    pub no_matches: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SuggestedSplit {
    pub identity: IdentityId,
    /// Pairs of its members the strategy judged `no_match`.
    pub no_matches: u64,
    /// Pairs of its members the strategy judged `match`.
    pub matches: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JudgementCounts {
    #[serde(rename = "match")]
    pub match_: u64,
    pub no_match: u64,
    pub unsure: u64,
}

/// A changeset the engine wrote for a caller, and its receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Authored {
    pub changeset: Changeset,
    /// `None` when the caller asked only to see the changeset. It was
    /// validated all the same.
    pub committed: Option<Committed>,
}

/// What a split did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SplitReport {
    /// The identity that was split. It keeps its ID and the larger side.
    pub kept: IdentityId,
    /// The new identity.
    pub identity: IdentityId,
    /// The records that moved to the new identity, in order.
    pub moved: Vec<RecordId>,
    pub changeset: Changeset,
    pub committed: Committed,
}

/// Which constraints to list. The default lists those in force.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConstraintFilter {
    /// Only those that name this record.
    pub record: Option<RecordId>,
    /// Only those that name a current member of this identity.
    pub identity: Option<IdentityId>,
    /// Also those that were removed.
    pub include_removed: bool,
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
    user_code: RwLock<Option<Arc<UserCode>>>,
    /// Held by a call to [`resolve`](Engine::resolve) or to
    /// [`extract`](Engine::extract) from start to finish, so that one piece
    /// of user code runs at a time. The writer is let go while user code
    /// runs, and a second resolve that began in that gap would match the
    /// same records again, as a second extraction would read the same
    /// documents again.
    resolving: Mutex<()>,
    /// The documents the extraction in progress was given, so that a list
    /// of documents can say they are being read.
    reading: Mutex<Option<Reading>>,
}

/// What the extraction in progress reads.
struct Reading {
    run: RunId,
    extractor: String,
    documents: HashSet<DocumentId>,
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
        // One transaction, so a read made of several queries sees one
        // committed state however many changesets commit while it runs.
        conn.execute_batch("BEGIN TRANSACTION")?;
        let result = read(&conn);
        // A read has nothing to commit. A connection whose transaction did
        // not end is not kept for the next read.
        if conn.execute_batch("ROLLBACK").is_ok() {
            lock(&self.idle).push(conn);
        }
        result
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How often an explanation begins again when the declaration changed
/// under it, and how long it waits in between.
const EXPLAIN_ATTEMPTS: usize = 20;
const EXPLAIN_PAUSE: std::time::Duration = std::time::Duration::from_millis(5);

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
    /// New records are those loaded after this run.
    after_run: RunId,
    /// The review items that were open when the run began. Their records
    /// are examined again.
    #[serde(default)]
    review_items: Vec<u64>,
    /// Every record of the type is examined, not only the new ones.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    rematch: bool,
    /// The run judged every record for a recomputation that was applied.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    recompute: bool,
    rules: crate::project::RuleSet,
}

/// The strategy a resolve runs for an entity type.
enum Chosen {
    Rules,
    UserCode(Strategy),
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
    /// Opens the store at `path`, creating it if it does not exist and
    /// upgrading it in place if its format has a migration.
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
            user_code: RwLock::new(None),
            resolving: Mutex::new(()),
            reading: Mutex::new(None),
        })
    }

    /// Tells the engine where the project's files are, so that it can run
    /// the user-code strategies and the extractors the manifest names.
    /// Without this it runs the SQL rules only.
    pub fn set_user_code(&self, mut user_code: UserCode) {
        let mut held = self
            .user_code
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        // Code that is running was started by what is replaced here, and
        // what it says is still asked for.
        if let Some(previous) = held.as_ref() {
            user_code.running = previous.running.clone();
        }
        *held = Some(Arc::new(user_code));
    }

    /// The last line the code of a run has written to its standard error,
    /// while it runs. `None` for a run whose code is not running, which is
    /// every run that has finished, and for code that has said nothing.
    pub fn said(&self, run: RunId) -> Option<String> {
        self.user_code()?.running.said(run)
    }

    /// Where the engine runs user code from, if it was told.
    pub fn user_code(&self) -> Option<Arc<UserCode>> {
        self.user_code
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
    /// stored records. Identities and the log are not touched. Match
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

    /// Stores rows from one source as records of one entity type and
    /// computes their feature rows. A row that is already known, by its
    /// source, key and content, creates nothing.
    pub fn load(
        &self,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
    ) -> Result<LoadReport> {
        self.lock()
            .load(&Actor::Engine, source, entity_type, rows, true)
    }

    /// As [`Engine::load`], for a caller that means to add rows and not to
    /// change any. A row under a key the store holds with other content
    /// would become the next version of that row. Here it is refused, and
    /// the whole call with it: nothing is stored, and the error names the
    /// keys. Content the key held before, and not now, is other content:
    /// a row that would change back is refused too. A row the store holds
    /// as it is creates nothing, as in any load, and so does one a source
    /// took back and sends again unchanged.
    pub fn load_without_replacing(
        &self,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
    ) -> Result<LoadReport> {
        self.lock()
            .load(&Actor::Engine, source, entity_type, rows, false)
    }

    /// Takes back rows a source no longer holds, named by their keys. Every
    /// version of each row stays in the store as evidence and leaves its
    /// identity, by one changeset under `actor`'s name. From then on no run
    /// examines the records, and a changeset that would give one an
    /// identity is refused, until the source sends the row again: an
    /// load of the key, with the same content or other, brings every
    /// version back to the identity it left.
    ///
    /// A key the source has no row under is counted and changes nothing, so
    /// the same retraction can be sent twice. A row without a key cannot be
    /// taken back.
    pub fn retract(
        &self,
        source: &str,
        entity_type: &str,
        keys: &[String],
        actor: Actor,
        reason: Option<String>,
    ) -> Result<RetractReport> {
        self.lock()
            .retract(source, entity_type, keys, actor, reason)
    }

    /// Starts a match run and generates its candidate pairs with the
    /// project's block rules. The run examines the records loaded
    /// since the last match run of the type that succeeded, and again those
    /// of open review items.
    ///
    /// The run is `open`: it takes judgements until it is reconciled, and it
    /// stays open across a restart.
    pub fn begin_run(&self, strategy: &StrategyRef, scope: &Scope) -> Result<RunId> {
        self.lock()
            .begin_match(&Actor::Engine, strategy, scope, RunStatus::Open)
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
    /// The whole submission is refused if a row names a record the
    /// store does not hold as this run's entity type, or a pair in which
    /// neither record is new to the run.
    pub fn submit_judgements(&self, run: RunId, rows: impl RecordBatchReader) -> Result<u64> {
        self.lock().submit_judgements(run, rows)
    }

    /// Turns a match run's judgements into a draft changeset and leaves it
    /// for review. Nothing is committed. What the draft leaves out is stored
    /// as review items, which stay open until their records have an
    /// identity.
    ///
    /// The same as [`reconcile_with`](Self::reconcile_with) and
    /// [`Disposition::LeaveForReview`], returning the draft itself.
    pub fn reconcile(&self, run: RunId, policy: &Policy) -> Result<Draft> {
        let (draft, _) =
            self.lock()
                .reconcile(&Actor::Engine, run, policy, Disposition::LeaveForReview)?;
        Ok(draft)
    }

    /// Turns a match run's judgements into a draft changeset, and commits the
    /// draft or stores it for review.
    ///
    /// A stored draft is named by its changeset ID. The run is `drafted`
    /// until the draft is committed, and only then has it succeeded: what is
    /// new to the next run is counted from the last run that succeeded, so
    /// the records of a draft nobody commits are examined again. A run
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
        let (_, report) = self.lock().reconcile(&Actor::Engine, run, policy, then)?;
        Ok(report)
    }

    /// Gives up an open match run, or a running extract run. Either is
    /// recorded as failed. The records of a match run are still new to the
    /// next run. An extract run's extractor is stopped with every process it
    /// started, nothing it found is stored, and its documents are still
    /// unread; the run ends when the extractor has stopped, which may be
    /// after this returns. Once what the extractor found is being stored,
    /// it is too late, and the cancellation is refused.
    pub fn cancel_run(&self, run: RunId) -> Result<()> {
        let row = self.read(|conn| run_row(conn, run))?;
        if row.kind == "extract" {
            if row.status != RunStatus::Running {
                return Err(Error::Run {
                    run,
                    message: format!("it is {} and cannot be cancelled", row.status),
                });
            }
            if let Some(user_code) = self.user_code()
                && !user_code.running.cancel(run)
            {
                return Err(Error::Run {
                    run,
                    message: "its extractor has finished and what it found is being stored, so \
                              it is too late to cancel it"
                        .to_owned(),
                });
            }
            return Ok(());
        }
        self.lock().cancel_run(run)
    }

    /// Validates a changeset against current state and commits it as it
    /// is. This is the only way identity state changes, whoever wrote the
    /// changeset.
    ///
    /// Committing the changeset of a stored draft approves the draft.
    pub fn commit(&self, changeset: Changeset) -> Result<Committed> {
        self.lock().commit(changeset, false)
    }

    /// Commits a changeset someone wrote, with the earlier versions of the
    /// rows it places moved along with them, as [`author`](Self::author)
    /// does. A changeset that carries the ID of a stored draft is committed
    /// as it was stored. What was made to be committed as it is, such as an
    /// undo that was returned without being committed, goes to
    /// [`commit`](Self::commit).
    pub fn commit_with_versions(&self, changeset: Changeset) -> Result<Committed> {
        self.lock().commit(changeset, true)
    }

    /// Writes a changeset from operations and commits it. The engine mints
    /// the changeset's ID and bases it on the head of the log, so this is for
    /// a caller that has just read the state it is changing. A caller that
    /// wants its own base checked, as a merge written from an older read
    /// should be, writes the changeset itself and calls
    /// [`commit`](Self::commit).
    pub fn author(&self, actor: Actor, reason: Option<String>, ops: Vec<Op>) -> Result<Authored> {
        self.lock().author(actor, reason, ops, true, true)
    }

    /// Takes back the changeset at a log position with a new changeset, which
    /// is validated like any other: an undo that no longer fits the state is
    /// rejected, and the rejection names the operation that does not fit.
    /// See [`inverse`] for what undoing each kind of operation means.
    ///
    /// With `commit` unset the changeset is validated and returned without
    /// being committed.
    pub fn undo(
        &self,
        seq: Seq,
        actor: Actor,
        reason: Option<String>,
        commit: bool,
    ) -> Result<Authored> {
        self.lock().undo(seq, actor, reason, commit)
    }

    /// Splits an identity in two: the named records on one side, the
    /// rest of its members on the other. The larger side keeps the ID, the
    /// side that was not named on a tie, and the other side gets a new
    /// identity. With `keep_apart`, a `cannot_link` constraint between the
    /// two sides is recorded in the same changeset, so that nothing puts
    /// them back together unnoticed.
    ///
    /// Which side moves depends on every member, also on those that were
    /// not named. A caller that chose the sides from what it read gives the
    /// log position it read at as `base`, and the split is refused if the
    /// identity's members changed after that.
    pub fn split(
        &self,
        identity: IdentityId,
        records: &[RecordId],
        keep_apart: bool,
        base: Option<Seq>,
        actor: Actor,
        reason: Option<String>,
    ) -> Result<SplitReport> {
        self.lock()
            .split(identity, records, keep_apart, base, actor, reason)
    }

    /// Constraints, ordered by their records.
    pub fn constraints(&self, filter: &ConstraintFilter) -> Result<Vec<ConstraintEntry>> {
        self.read(|conn| constraints(conn, filter))
    }

    /// Closes an open review item without giving its records an
    /// identity. Later runs no longer examine them on its account.
    pub fn dismiss_review(
        &self,
        id: u64,
        actor: &Actor,
        reason: Option<&str>,
    ) -> Result<ReviewEntry> {
        let mut inner = self.lock();
        let now = inner.store.clock.now_micros();
        review::dismiss(&inner.store.conn, id, actor, now, reason)
    }

    /// Commits a stored draft. It is validated like any changeset: if state
    /// has moved under it, the rejections name the operations that no longer
    /// hold, and the draft stays open.
    pub fn approve_draft(&self, draft: ChangesetId) -> Result<Committed> {
        self.lock().approve_draft(draft)
    }

    /// Closes a stored draft without committing it. Its match run has not
    /// succeeded, so the next run examines the same records again.
    pub fn reject_draft(&self, draft: ChangesetId, reason: Option<&str>) -> Result<DraftSummary> {
        self.lock().reject_draft(draft, reason)
    }

    /// A stored draft with its changeset.
    pub fn draft(&self, draft: ChangesetId) -> Result<DraftEntry> {
        self.read(|conn| drafts::get(conn, draft))?
            .ok_or(Error::UnknownDraft(draft))
    }

    /// Stored drafts without their operations, newest first.
    pub fn drafts(&self, filter: &DraftFilter) -> Result<Vec<DraftSummary>> {
        self.read(|conn| drafts::list(conn, filter))
    }

    /// Matches new records with a strategy, reconciles with the default
    /// policy, and commits or leaves a draft for review. A type is resolved
    /// when it has newly loaded records or open review items, and
    /// always by a [rematch](ResolveOptions::rematch).
    ///
    /// The strategy is the built-in runner of the project's SQL rules, or
    /// user code the project holds, which runs as a process of its own.
    /// While it runs the engine is free: changesets are committed, rows are
    /// loaded and reads are answered. What the user code hands back is
    /// evidence, reconciled against the state there is when it finishes. A
    /// strategy that fails, or does not finish in time, fails its run and
    /// changes nothing.
    ///
    /// The match run is recorded as succeeded only after the commit. A
    /// resolve that is cut short leaves the run unfinished, and the next
    /// resolve examines the same records again.
    pub fn resolve(&self, options: &ResolveOptions) -> Result<ResolveReport> {
        self.resolve_by(&Actor::Engine, options)
    }

    fn resolve_by(&self, by: &Actor, options: &ResolveOptions) -> Result<ResolveReport> {
        let _turn = lock(&self.resolving);
        let plan = self.lock().plan(options)?;
        // Before anything is started: user code needs the project's files,
        // and a file that is not the one the manifest was compiled from is
        // never run.
        let user_code = self.user_code();
        for (_, chosen) in &plan {
            if let Chosen::UserCode(strategy) = chosen {
                user_code
                    .as_deref()
                    .ok_or_else(|| Error::NoUserCode(strategy.name.clone()))?
                    .check(&strategy.into())?;
            }
        }
        let then = if options.leave_for_review {
            Disposition::LeaveForReview
        } else {
            Disposition::Commit
        };

        let mut report = ResolveReport::default();
        for (entity_type, chosen) in plan {
            let mut inner = self.lock();
            if !options.rematch && !inner.has_work(&entity_type)? {
                continue;
            }
            let scope = Scope {
                entity_type,
                rematch: options.rematch,
            };
            let resolution = match chosen {
                Chosen::Rules => {
                    let run =
                        inner.begin_match(by, &StrategyRef::rules(), &scope, RunStatus::Running)?;
                    inner.in_run(run, |inner| {
                        inner.judge_with_rules(run)?;
                        inner.conclude(by, run, &Policy::default(), then)
                    })?
                }
                Chosen::UserCode(strategy) => {
                    let user_code = user_code
                        .clone()
                        .ok_or_else(|| Error::NoUserCode(strategy.name.clone()))?;
                    let reference = StrategyRef {
                        name: strategy.name.clone(),
                        version: Some(strategy.hash.clone()),
                    };
                    let run = inner.begin_match(by, &reference, &scope, RunStatus::Running)?;
                    let mut task =
                        inner.in_run(run, |inner| inner.write_task(run, &strategy, &user_code))?;
                    // The user code runs with the writer let go. The run
                    // stays `running`, and what it was given stays as it
                    // was written, whatever is committed meanwhile.
                    drop(inner);
                    let executed = task.execute();
                    let mut inner = self.lock();
                    let resolution = inner.in_run(run, |inner| {
                        executed?;
                        inner
                            .take_judgements(run, &task)
                            .map_err(|error| task.rejected(error))?;
                        inner.conclude(by, run, &Policy::default(), then)
                    })?;
                    task.remove();
                    resolution
                }
            };
            report.resolved.push(resolution.1);
        }
        Ok(report)
    }

    /// What the identities of a type, or of every type, would be if every
    /// record were matched again, all at once, by the rules as they are
    /// now, against the identities there are. Nothing is changed or
    /// recorded to find out, and writes do not wait for it.
    ///
    /// See [`apply_recompute`](Self::apply_recompute) for applying it.
    pub fn recompute(&self, request: &RecomputeRequest) -> Result<RecomputeReport> {
        let manifest = self.manifest().ok_or(Error::NoManifest)?;
        self.read(|conn| {
            let applied = schema::meta(conn, "manifest_hash")?;
            if applied.as_deref() != Some(manifest.hash.as_str()) {
                return Err(Error::ManifestMismatch {
                    applied,
                    manifest: manifest.hash.clone(),
                });
            }
            let detail_of = match request.identity {
                Some(id) => Some(match store::read_identities(conn, &[id])?.remove(0) {
                    None => return Err(Error::UnknownIdentity(id)),
                    Some(entry) => match entry.status {
                        IdentityStatus::Active => id,
                        IdentityStatus::MergedInto(survivor) => survivor,
                    },
                }),
                None => None,
            };
            let entries = recomputed_types(&manifest, request.entity_type.as_ref())?;
            let mut plans = Vec::new();
            let mut types = Vec::new();
            for entry in entries {
                let judgements = recompute::judge_for_preview(conn, &entry.name, &entry.rules)?;
                let outcome = recompute::compute(conn, &entry.name, &judgements)?;
                plans.push(outcome.plan());
                types.push(outcome.report(conn, request, detail_of)?);
            }
            Ok(RecomputeReport {
                plan: recompute::combined_plan(&plans),
                types,
            })
        })
    }

    /// Makes the identities of a type, or of every type, what
    /// [`recompute`](Self::recompute) showed under `plan`. It works the
    /// result out again, and applies nothing unless that is the plan: if
    /// the records, the rules or the identities changed in a way that
    /// changes the result, it is refused with [`Error::StalePlan`].
    ///
    /// For each type, a match run judges every record by the rules and its
    /// judgements are kept as any run's are, and a reconcile run commits
    /// one changeset by the engine, which validates like any other. The
    /// records that wait for review are left in `conflict` items, and an
    /// open draft of the type is superseded.
    pub fn apply_recompute(
        &self,
        entity_type: Option<&EntityType>,
        plan: &str,
        reason: Option<String>,
    ) -> Result<RecomputeApplied> {
        self.apply_recompute_by(&Actor::Engine, entity_type, plan, reason)
    }

    fn apply_recompute_by(
        &self,
        by: &Actor,
        entity_type: Option<&EntityType>,
        plan: &str,
        reason: Option<String>,
    ) -> Result<RecomputeApplied> {
        let _turn = lock(&self.resolving);
        self.lock().apply_recompute(by, entity_type, plan, reason)
    }

    /// A run as the store records it: the task record of whatever started
    /// it.
    pub fn run(&self, run: RunId) -> Result<RunEntry> {
        let mut entry = self
            .read(|conn| runs::get(conn, run))?
            .ok_or(Error::UnknownRun(run))?;
        if entry.status == RunStatus::Running {
            entry.said = self.said(run);
        }
        Ok(entry)
    }

    /// Runs, newest first.
    pub fn runs(&self, filter: &RunFilter) -> Result<Vec<RunEntry>> {
        self.read(|conn| runs::list(conn, filter))
    }

    /// Review items, oldest first.
    pub fn review_items(&self, filter: &ReviewFilter) -> Result<Vec<ReviewEntry>> {
        self.read(|conn| review::list(conn, filter))
    }

    /// A review item with what a reviewer reads to decide it: the records
    /// it holds back, the identities they matched with their members, and
    /// what was judged between them.
    pub fn review_case(&self, id: u64) -> Result<ReviewCase> {
        self.read(|conn| review::case(conn, id))
    }

    pub fn identity_of(&self, record: RecordId) -> Result<Option<IdentityId>> {
        Ok(self.identities_of(&[record])?[0])
    }

    /// The identity of each record, in the order asked. `None` for an
    /// record that has no identity or that the store does not hold.
    pub fn identities_of(&self, records: &[RecordId]) -> Result<Vec<Option<IdentityId>>> {
        self.read(|conn| store::read_assignments(conn, records))
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

    /// An identity with the records that belong to it now.
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

    /// A record as it was loaded, or `None` if the store does not
    /// hold it.
    pub fn record(&self, id: RecordId) -> Result<Option<RecordEntry>> {
        self.read(|conn| lookup::record(conn, id))
    }

    /// Records, ordered by source and key. A caller that knows a row by its
    /// source and key finds its record IDs here: only the engine computes
    /// them.
    pub fn records(&self, filter: &RecordFilter) -> Result<Vec<RecordEntry>> {
        self.read(|conn| lookup::records(conn, filter))
    }

    /// What the attributes of the records that stand hold, for each
    /// entity type and source the filter takes: see [`AttributeGroup`].
    pub fn attributes(&self, filter: &AttributeFilter) -> Result<Vec<AttributeGroup>> {
        let manifest = self.manifest();
        self.read(|conn| crate::attributes::attributes(conn, manifest.as_deref(), filter))
    }

    /// Why two records share an identity, or do not: where each is,
    /// what every run that looked at the pair concluded, and what the
    /// project's rules say about it now.
    pub fn explain(&self, a: RecordId, b: RecordId) -> Result<Explanation> {
        self.with_rules(|conn, manifest| explain::explain(conn, manifest, a, b))
    }

    /// What holds an identity together: how many of its pairs of records
    /// are linked directly and how many only through chains, and the
    /// fewest links whose removal would part it, where a wrong merge
    /// entered.
    pub fn explain_identity(&self, id: IdentityId) -> Result<IdentityExplanation> {
        self.with_rules(|conn, manifest| explain::explain_identity(conn, manifest, id))
    }

    /// A read that needs the rules the feature tables were built for. The
    /// manifest is read before the store is, and `apply` can commit
    /// another declaration in between, so the read checks that the store
    /// was applied with the manifest it was given, and begins again with
    /// the engine's present manifest if not. When the declaration keeps
    /// changing, the read is done without one.
    fn with_rules<T>(
        &self,
        read: impl Fn(&Connection, Option<&Manifest>) -> Result<T>,
    ) -> Result<T> {
        for _ in 0..EXPLAIN_ATTEMPTS {
            let manifest = self.manifest();
            let done = self.read(|conn| {
                if let Some(manifest) = &manifest {
                    let applied = schema::meta(conn, "manifest_hash")?;
                    if applied.as_deref() != Some(manifest.hash.as_str()) {
                        return Ok(None);
                    }
                }
                read(conn, manifest.as_deref()).map(Some)
            })?;
            if let Some(done) = done {
                return Ok(done);
            }
            std::thread::sleep(EXPLAIN_PAUSE);
        }
        self.read(|conn| read(conn, None))
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

    /// The same changesets as [`log_after`](Self::log_after), each with who
    /// started the run it came from. The log is what was changed and who
    /// decided it; this adds who asked.
    pub fn logged_after(&self, after: Seq, limit: u64) -> Result<Vec<LoggedChangeset>> {
        self.read(|conn| lookup::logged(conn, store::read_log_after(conn, after, limit)?))
    }

    /// The log head and up to `limit` summaries of changesets after
    /// `after`, ordered by position through that head.
    pub fn changeset_summaries(
        &self,
        after: Seq,
        limit: u64,
    ) -> Result<(Seq, Vec<ChangesetSummary>)> {
        self.read(|conn| {
            let head = store::read_head(conn)?;
            Ok((head, lookup::changeset_summaries(conn, after, head, limit)?))
        })
    }

    /// At most `limit` operations starting at the zero-based `offset` in
    /// the committed changeset at `seq`.
    pub fn changeset_ops(&self, seq: Seq, offset: u64, limit: u64) -> Result<Vec<Op>> {
        self.read(|conn| store::read_log_ops(conn, seq, offset, limit))
    }

    /// At most `limit` committed changesets in another order than the
    /// log's, each with who started the run it came from. A page continues
    /// after the changeset at `after`, wherever that stands in the order.
    /// `names` says what people are called, for sorting by who decided.
    pub fn logged_sorted(
        &self,
        sort: Sort<LogSort>,
        after: Option<Seq>,
        limit: u64,
        names: &[(Actor, String)],
    ) -> Result<Vec<LoggedChangeset>> {
        self.read(|conn| lookup::logged(conn, lookup::changesets(conn, sort, after, limit, names)?))
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
    /// `record_id` and `true_entity`.
    pub fn score(&self, truth: impl RecordBatchReader) -> Result<Metrics> {
        let inner = self.lock();
        let conn = &inner.store.conn;
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {} (record_id UUID, true_entity VARCHAR)",
            metrics::TRUTH
        ))?;
        for batch in truth {
            conn.execute(
                &format!(
                    "INSERT INTO {} SELECT record_id::UUID, true_entity::VARCHAR FROM arrow(?)",
                    metrics::TRUTH
                ),
                [&ArrowBatchRegistration::new(batch?)],
            )?;
        }
        metrics::score(conn)
    }

    /// Compares each record's identity at two log positions.
    pub fn id_stability(&self, before: Seq, after: Seq) -> Result<IdStability> {
        self.read(|conn| metrics::id_stability(conn, before, after))
    }

    /// What changed between two log positions, against the number of
    /// records that arrived in between.
    pub fn churn(&self, before: Seq, after: Seq, new_records: u64) -> Result<Churn> {
        self.read(|conn| metrics::churn(conn, before, after, new_records))
    }
}

/// The manifest's entry for the type a recomputation is asked about, or
/// for every type it declares.
fn recomputed_types<'a>(
    manifest: &'a Manifest,
    entity_type: Option<&EntityType>,
) -> Result<Vec<&'a EntityTypeManifest>> {
    match entity_type {
        Some(name) => Ok(vec![
            manifest
                .entity_type(name.as_str())
                .ok_or_else(|| Error::UndeclaredType(name.to_string()))?,
        ]),
        None => Ok(manifest.entity_types.iter().collect()),
    }
}

fn constraints(conn: &Connection, filter: &ConstraintFilter) -> Result<Vec<ConstraintEntry>> {
    let mut statement = conn.prepare(
        "SELECT c.record_a::VARCHAR, c.record_b::VARCHAR, c.kind, c.from_seq, c.to_seq
         FROM spool.constraint_link c
         WHERE ($1::BOOLEAN OR c.to_seq IS NULL)
           AND ($2::VARCHAR IS NULL OR $2::UUID IN (c.record_a, c.record_b))
           AND ($3::VARCHAR IS NULL OR EXISTS (
                 SELECT 1 FROM spool.membership m
                 WHERE m.to_seq IS NULL AND m.identity_id = $3::UUID
                   AND m.record_id IN (c.record_a, c.record_b)))",
    )?;
    let mut rows = statement.query(params![
        filter.include_removed,
        filter.record.map(|id| id.to_string()),
        filter.identity.map(|id| id.to_string()),
    ])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(ConstraintEntry {
            constraint: store::constraint(row, 0)?,
            from: Seq(row.get::<_, i64>(3)? as u64),
            to: row.get::<_, Option<i64>>(4)?.map(|seq| Seq(seq as u64)),
        });
    }
    found.sort();
    Ok(found)
}

/// The name of the temporary table that holds the judgements a reconcile run
/// goes by.
const JUDGED: &str = "spool_judged";

/// The candidate pairs of a run with both sides' features.
fn candidates(conn: &Connection, run: RunId) -> Result<Rows> {
    let (_, config) = match_run(conn, run)?;
    let table = features::table(&config.entity_type);
    let columns = feature_columns(conn, &table)?;
    let mut statement = conn.prepare(&candidates_query(&table, &columns, run, false))?;
    let arrow = statement.query_arrow([])?;
    let schema = arrow.get_schema();
    Ok(Rows::new(schema, arrow.collect()))
}

/// The feature columns of a feature table with their types, without the two
/// columns the engine keeps for itself.
fn feature_columns(conn: &Connection, table: &str) -> Result<Vec<(String, String)>> {
    let mut statement = conn.prepare(&format!("DESCRIBE {table}"))?;
    let mut rows = statement.query([])?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        if name != "record_id" && name != "declaration_version" {
            columns.push((name, row.get(1)?));
        }
    }
    Ok(columns)
}

/// The feature columns of the table aliased `alias`, as the end of a select
/// list, each named with `prefix` before its own name. In a file, where IDs
/// are text, a UUID is written as text.
fn select_features(columns: &[(String, String)], alias: &str, prefix: &str, file: bool) -> String {
    let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
    columns
        .iter()
        .map(|(name, data_type)| {
            let cast = if file && data_type == "UUID" {
                "::VARCHAR"
            } else {
                ""
            };
            format!(
                ", {alias}.{}{cast} AS {}",
                quote(name),
                quote(&format!("{prefix}{name}"))
            )
        })
        .collect()
}

/// A query for a run's candidate pairs with both sides' features: `left_id`,
/// `right_id`, then each feature column as `left_<name>` and as
/// `right_<name>`.
fn candidates_query(table: &str, columns: &[(String, String)], run: RunId, file: bool) -> String {
    let id = if file { "::VARCHAR" } else { "" };
    format!(
        "SELECT c.left_id{id} AS left_id, c.right_id{id} AS right_id{left}{right}
         FROM (SELECT DISTINCT left_id, right_id FROM spool.candidate WHERE run_id = {run}) c
         JOIN {table} l ON l.record_id = c.left_id
         JOIN {table} r ON r.record_id = c.right_id
         ORDER BY c.left_id, c.right_id",
        left = select_features(columns, "l", "left_", file),
        right = select_features(columns, "r", "right_", file),
    )
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
    /// The moves that take the earlier versions of the rows `changeset`
    /// places along with them, as far as the state allows: a move it
    /// refuses, such as one a constraint forbids, is left out. Nothing when
    /// the state refuses the changeset's own operations, so that the
    /// refusal is theirs alone, or when it still refuses a move once those
    /// it named are left out, so it is validated with the moves twice at
    /// most.
    fn versions_along(&self, changeset: &Changeset) -> Result<Vec<Op>> {
        let own = changeset.ops.len();
        let mut along = following(&self.store.conn, &changeset.ops)?;
        let mut rounds = 0;
        while !along.is_empty() {
            if rounds == 2 {
                return Ok(Vec::new());
            }
            rounds += 1;
            let mut whole = changeset.clone();
            whole.ops.extend(along.iter().cloned());
            let rejections = match validate(&self.store, whole) {
                Ok(_) => break,
                Err(ValidationError::Rejected(rejections)) => rejections,
                Err(_) => return Ok(Vec::new()),
            };
            let refused: HashSet<usize> = rejections
                .iter()
                .filter_map(|rejection| rejection.op_index)
                .filter(|index| *index >= own)
                .map(|index| index - own)
                .collect();
            if refused.is_empty() {
                return Ok(Vec::new());
            }
            let mut at = 0;
            along.retain(|_| {
                at += 1;
                !refused.contains(&(at - 1))
            });
        }
        Ok(along)
    }

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
            // That is a run a caller holds open, and a run whose user code
            // is at work: nothing else is unfinished while this call has the
            // writer.
            tx.execute(
                "UPDATE spool.run
                 SET status = 'failed', finished_at = make_timestamptz(?), error = ?
                 WHERE kind = 'match' AND status IN ('open', 'running')
                   AND json_extract_string(config, '$.entity_type') = ?",
                params![
                    now,
                    format!("the declaration of `{name}` changed while the run was open"),
                    name.as_str()
                ],
            )?;
            report.rebuilt.push(name.clone());
        }
        features::label_view(
            &tx,
            manifest
                .entity_types
                .iter()
                .map(|entity_type| (&entity_type.name, &entity_type.declaration)),
        )?;
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

    /// Records a run as begun. `by` is who asked for it.
    fn start_run(
        &mut self,
        kind: &str,
        by: &Actor,
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
             VALUES (?, ?, 'running', ?, ?, ?, ?, ?, make_timestamptz(?), NULL, NULL, NULL, ?)",
            params![
                run,
                kind,
                strategy.map(|strategy| strategy.name.as_str()),
                strategy.and_then(|strategy| strategy.version.as_deref()),
                config.to_string(),
                hash_json(config),
                env!("CARGO_PKG_VERSION"),
                now,
                by.to_string()
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
             WHERE run_id = ? AND status <> 'failed'",
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

    fn load(
        &mut self,
        by: &Actor,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
        replace: bool,
    ) -> Result<LoadReport> {
        if source.is_empty() {
            return Err(Error::Load("the source needs a name".to_owned()));
        }
        let (manifest, entry, version) = self.declared(entity_type)?;
        let name = entry.name.clone();
        let declaration = entry.declaration.clone();
        let source = &manifest
            .source_of(source, entry)
            .map_err(Error::Load)?
            .clone();

        let run = self.start_run(
            "load",
            by,
            None,
            &json!({ "source": source.name, "entity_type": name }),
        )?;
        let report = self.in_run(run, |inner| {
            let now = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            let mut seen = HashSet::new();
            let (mut received, mut inserted, mut restored) = (0u64, 0u64, 0u64);

            for incoming in rows {
                let incoming = incoming?;
                let staged = load::stage(source, &incoming, &mut seen, received as usize)?;
                received += incoming.num_rows() as u64;
                let Some(staged) = staged else { continue };

                // The ID does not cover the entity type, so the same row sent
                // as another type would otherwise be dropped without a word.
                let clash: Option<String> = {
                    let mut statement = tx.prepare(
                        "SELECT o.entity_type FROM arrow(?) s
                         JOIN spool.record o ON o.record_id = s.record_id::UUID
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
                    return Err(Error::Load(format!(
                        "a row with the same source, key and content is already stored as entity type `{other}`"
                    )));
                }

                let stored = load::insert(&tx, name.as_str(), &source.name, now, run, staged)?;
                inserted += stored.inserted;
                restored += stored.restored;
            }
            if !replace && let Some(refusal) = load::replacing(&tx, run)? {
                return Err(Error::Load(refusal));
            }

            let problem = features::fill(
                &tx,
                &name,
                &declaration,
                version,
                &features::of_run(&name, run),
            )?;
            if let Some(problem) = problem {
                return Err(Error::Load(problem));
            }
            let superseded: i64 = tx.query_row(
                "SELECT count(*) FROM spool.record
                 WHERE load_run = ? AND supersedes IS NOT NULL",
                [run.0 as i64],
                |row| row.get(0),
            )?;
            let report = LoadReport {
                run,
                received,
                inserted,
                duplicates: received - inserted,
                superseded: superseded as u64,
                restored,
                returned: 0,
                committed: None,
                possible_splits: 0,
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
        })?;
        self.give_back(
            report,
            format!("source `{}` sent the rows again", source.name),
        )
    }

    /// Puts the records that came back in a load into the identity each
    /// of them left, by one changeset, and says so in the run's report.
    ///
    /// The load is already stored when this begins. Cut short in between,
    /// the store holds records that came back and have no identity, and
    /// the next match run takes them as new.
    pub(crate) fn give_back(
        &mut self,
        mut report: LoadReport,
        reason: String,
    ) -> Result<LoadReport> {
        if report.restored == 0 {
            return Ok(report);
        }
        let mut ops = retract::returning(&self.store.conn, report.run)?;
        // A record cannot go back where a constraint that was made
        // meanwhile forbids it. It is left out, and the rest go back.
        let committed = loop {
            if ops.is_empty() {
                return Ok(report);
            }
            let changeset = Changeset {
                id: lock(&self.ids).changeset_id(),
                base: self.store.head()?,
                actor: Actor::Engine,
                reason: Some(reason.clone()),
                run: None,
                ops: ops.clone(),
            };
            match validate(&self.store, changeset) {
                Ok(validated) => break self.store.apply(validated)?,
                Err(ValidationError::Rejected(rejections)) => {
                    let refused: HashSet<usize> = rejections
                        .iter()
                        .filter_map(|rejection| rejection.op_index)
                        .collect();
                    if refused.is_empty() {
                        return Ok(report);
                    }
                    let mut at = 0;
                    ops.retain(|_| {
                        at += 1;
                        !refused.contains(&(at - 1))
                    });
                }
                Err(error) => return Err(error.into()),
            }
        };
        report.returned = ops.len() as u64;
        report.committed = Some(committed);
        // A record is back where it was, and what held it there may still
        // be gone. That is put to a reviewer, as when a record leaves.
        let returned_to: Vec<IdentityId> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Assign {
                    to: Some(identity), ..
                } => Some(*identity),
                _ => None,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let tx = self.store.conn.transaction()?;
        report.possible_splits = possible_split::raise(&tx, report.run, &returned_to)?;
        tx.execute(
            "UPDATE spool.run SET result = ?::JSON WHERE run_id = ?",
            params![json!(report).to_string(), report.run.0 as i64],
        )?;
        tx.commit()?;
        Ok(report)
    }

    fn retract(
        &mut self,
        source: &str,
        entity_type: &str,
        keys: &[String],
        actor: Actor,
        reason: Option<String>,
    ) -> Result<RetractReport> {
        if keys.is_empty() {
            return Err(Error::Retract("no keys were named".to_owned()));
        }
        let (manifest, entry, _) = self.declared(entity_type)?;
        let name = entry.name.clone();
        let source = manifest
            .source_of(source, entry)
            .map_err(Error::Retract)?
            .name
            .clone();

        let run = self.start_run(
            "retract",
            &actor,
            None,
            &json!({ "source": source, "entity_type": name, "reason": reason }),
        )?;
        self.in_run(run, |inner| {
            let named = retract::name(&inner.store.conn, &name, &source, keys)?;

            // First the records leave their identities, and only then are
            // they recorded as taken back. Cut short in between, the store
            // holds records without an identity, which is a state it
            // can be in, and the same retraction sent again finishes the
            // work. What it then no longer knows is the identity they left,
            // so when they come back they are new to the next match run.
            let members = retract::members(&inner.store.conn)?;
            let detached = members.len() as u64;
            let left: Vec<IdentityId> = members
                .iter()
                .map(|(_, identity)| *identity)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let committed = if members.is_empty() {
                None
            } else {
                let changeset = Changeset {
                    id: lock(&inner.ids).changeset_id(),
                    base: inner.store.head()?,
                    actor,
                    reason: reason
                        .or_else(|| Some(format!("source `{source}` took the rows back"))),
                    // A changeset names the reconcile run that drafted it,
                    // and none drafted this one. The run names the
                    // changeset instead.
                    run: None,
                    ops: members
                        .into_iter()
                        .map(|(record, identity)| Op::Assign {
                            record,
                            from: Some(identity),
                            to: None,
                        })
                        .collect(),
                };
                let validated = validate(&inner.store, changeset)?;
                Some(inner.store.apply(validated)?)
            };

            let finished = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            let retracted = retract::record(
                &tx,
                &name,
                run,
                committed.as_ref().map(|committed| committed.seq),
            )?;
            // What is left of an identity may have been held together only
            // through a record that is gone now. That is put to a reviewer.
            let possible_splits = possible_split::raise(&tx, run, &left)?;
            let report = RetractReport {
                run,
                received: named.received,
                unknown: named.unknown,
                retracted,
                detached,
                committed,
                possible_splits,
            };
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
        by: &Actor,
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
            rematch: scope.rematch,
            recompute: false,
            rules,
        };

        let run = self.start_run("match", by, Some(strategy), &json!(config))?;
        self.in_run(run, |inner| {
            let tx = inner.store.conn.transaction()?;
            rules::create_scope(
                &tx,
                &entity_type,
                config.after_run,
                run,
                &config.review_items,
                config.rematch,
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
    /// was loaded after it is new.
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
            config.rematch,
        )?;
        Ok(config)
    }

    fn submit_judgements(&mut self, run: RunId, rows: impl RecordBatchReader) -> Result<u64> {
        self.store_judgements(run, rows, RunStatus::Open)
    }

    /// Checks judgements and adds them to a match run that is in the state
    /// `expected`: `open` for a caller's, `running` for those of the
    /// engine's own user code.
    fn store_judgements(
        &mut self,
        run: RunId,
        rows: impl RecordBatchReader,
        expected: RunStatus,
    ) -> Result<u64> {
        let config = self.judged_run(run, expected)?;
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
                             THEN 'both sides are the same record'
                           WHEN j.verdict IS NULL OR j.verdict NOT IN ('match', 'no_match', 'unsure')
                             THEN 'the verdict must be match, no_match or unsure'
                           WHEN {detail} IS NOT NULL AND NOT json_valid({detail})
                             THEN 'the detail is not valid JSON'
                           WHEN l.record_id IS NULL OR r.record_id IS NULL
                             THEN 'it names a record that is not a known {entity_type}'
                           WHEN ls.record_id IS NULL AND rs.record_id IS NULL
                             THEN 'neither record is new to this run'
                         END AS problem
                       FROM arrow(?) j
                       LEFT JOIN spool.record l
                         ON l.record_id = try_cast(j.left_id AS UUID) AND l.entity_type = '{entity_type}'
                       LEFT JOIN spool.record r
                         ON r.record_id = try_cast(j.right_id AS UUID) AND r.entity_type = '{entity_type}'
                       LEFT JOIN {scope} ls ON ls.record_id = l.record_id
                       LEFT JOIN {scope} rs ON rs.record_id = r.record_id
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
        by: &Actor,
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
        self.conclude(by, run, policy, then)
    }

    /// Turns a match run's judgements into a draft, stores its review items,
    /// and commits the draft or stores it.
    ///
    /// The match run has succeeded once its draft is committed, or when there
    /// is nothing to commit, and not before: what is new to the next run is
    /// counted from the last run that succeeded. If the process stops between
    /// the commit and the line that records it, the next run examines the
    /// same records again, and those the commit placed are no longer
    /// new.
    fn conclude(
        &mut self,
        by: &Actor,
        run: RunId,
        policy: &Policy,
        then: Disposition,
    ) -> Result<(Draft, TypeResolution)> {
        let (_, config) = self.match_run(run)?;
        let entity_type = config.entity_type.clone();

        let reconcile_run = self.start_run(
            "reconcile",
            by,
            None,
            &json!({ "match_run": run, "policy": policy }),
        )?;
        let (draft, mut report) = self.in_run(reconcile_run, |inner| {
            let evidence = inner.evidence(run, &config)?;
            let new_records = evidence.records.len() as u64;
            // A record that came back after its source had taken it back
            // was loaded long ago too, and no run left it for review.
            let reexamined = inner.count(&format!(
                "SELECT count(*) FROM {} s
                 JOIN spool.record o ON o.record_id = s.record_id
                 WHERE o.load_run <= {after_run}
                   AND s.record_id NOT IN ({restored})",
                rules::SCOPE,
                after_run = config.after_run,
                restored = retract::restored_between(config.after_run, run),
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
            let (mut identities_created, mut records_assigned) = (0, 0);
            for op in &draft.changeset.ops {
                match op {
                    spool_core::Op::CreateIdentity { .. } => identities_created += 1,
                    spool_core::Op::Assign { .. } => records_assigned += 1,
                    _ => {}
                }
            }
            let along = inner.versions_along(&draft.changeset)?;
            let versions_moved = along.len() as u64;
            draft.changeset.ops.extend(along);
            let held = then == Disposition::LeaveForReview && !draft.changeset.ops.is_empty();
            let report = TypeResolution {
                entity_type: entity_type.clone(),
                match_run: run,
                reconcile_run,
                new_records,
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
                records_assigned,
                versions_moved,
                records_unassigned: new_records - records_assigned,
                review: draft.review.clone(),
                committed: None,
                draft: held.then_some(draft.changeset.id),
                suggestions: match config.rematch {
                    true => Some(inner.suggestions(run)?),
                    false => None,
                },
            };

            let finished = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            review::record(&tx, &entity_type, reconcile_run, &draft.review)?;
            // The match run may have judged a match between records that
            // nothing held together. If it commits nothing that touches
            // their identity, the question is closed where the run is
            // concluded: here when its draft is left for review, and after
            // the commit otherwise.
            if then == Disposition::LeaveForReview {
                possible_split::supersede_answered(&tx, &entity_type, reconcile_run, run)?;
            }
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
            // The run has succeeded, and its judgements are evidence from
            // here on: both are recorded together.
            let finished = self.store.clock.now_micros();
            let tx = self.store.conn.transaction()?;
            tx.execute(
                "UPDATE spool.run
                 SET status = 'succeeded', finished_at = make_timestamptz(?), result = ?::JSON
                 WHERE run_id = ?",
                params![finished, json!(report).to_string(), run.0 as i64],
            )?;
            possible_split::supersede_answered(&tx, &entity_type, reconcile_run, run)?;
            tx.commit()?;
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
            config.rematch,
        )?;
        // A record that has an identity is not new: one that got it
        // since the run began, and in a rematch every one that had it.
        conn.execute_batch(&format!(
            "DELETE FROM {scope} WHERE record_id IN (
               SELECT record_id FROM spool.membership WHERE to_seq IS NULL)"
        ))?;
        // What the reconciler goes by is the run's judgements, except where
        // something stronger has spoken for a pair. A constraint is a
        // decision someone committed: `must_link` counts as a match and
        // `cannot_link` as a no-match, whatever the strategy said and whether
        // or not it looked at the pair. Next comes the source: a row sent
        // again under its key with other content is the same thing as the
        // version before it.
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {JUDGED} AS
             WITH spoken AS (
               SELECT least(c.record_a, c.record_b) AS left_id,
                      greatest(c.record_a, c.record_b) AS right_id,
                      CASE c.kind WHEN 'must_link' THEN 'match' ELSE 'no_match' END AS verdict,
                      1 AS rank
               FROM spool.constraint_link c
               WHERE c.to_seq IS NULL
                 AND (c.record_a IN (SELECT record_id FROM {scope})
                   OR c.record_b IN (SELECT record_id FROM {scope}))
               UNION ALL
               SELECT least(o.record_id, o.supersedes),
                      greatest(o.record_id, o.supersedes), 'match', 2
               FROM spool.record o
               WHERE o.supersedes IS NOT NULL AND o.entity_type = '{entity_type}'
                 AND (o.record_id IN (SELECT record_id FROM {scope})
                   OR o.supersedes IN (SELECT record_id FROM {scope}))
             ), decided AS (
               SELECT left_id, right_id, arg_min(verdict, rank) AS verdict
               FROM spoken GROUP BY left_id, right_id
             )
             SELECT left_id, right_id, verdict, NULL::DOUBLE AS score FROM decided
             UNION ALL
             SELECT j.left_id, j.right_id, j.verdict, j.score
             FROM spool.judgement j
             WHERE j.run_id = {run} AND NOT EXISTS (
               SELECT 1 FROM decided d
               WHERE d.left_id = j.left_id AND d.right_id = j.right_id)",
            entity_type = config.entity_type,
        ))?;
        let mut evidence = EvidenceSummary::default();

        let mut statement = conn.prepare(&format!(
            "SELECT record_id::VARCHAR FROM {scope} ORDER BY record_id"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            evidence.records.push(NewRecord {
                record: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                entity_type: config.entity_type.clone(),
            });
        }

        let mut statement = conn.prepare(&format!(
            "SELECT left_id::VARCHAR, right_id::VARCHAR, verdict
             FROM {JUDGED}
             WHERE left_id IN (SELECT record_id FROM {scope})
               AND right_id IN (SELECT record_id FROM {scope})
             ORDER BY left_id, right_id, verdict"
        ))?;
        let mut rows = statement.query([])?;
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
               FROM {JUDGED}
               WHERE left_id IN (SELECT record_id FROM {scope})
               UNION ALL
               SELECT right_id, left_id, verdict, score
               FROM {JUDGED}
               WHERE right_id IN (SELECT record_id FROM {scope})
             ) j
             JOIN spool.membership m ON m.record_id = j.other_id AND m.to_seq IS NULL
             GROUP BY j.new_id, m.identity_id
             ORDER BY j.new_id, m.identity_id"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            evidence.links.push(IdentityLink {
                record: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                identity: row.get::<_, String>(1)?.parse().map_err(id_error)?,
                matches: row.get::<_, i64>(2)? as u64,
                no_matches: row.get::<_, i64>(3)? as u64,
                best_score: row.get(4)?,
            });
        }
        Ok(evidence)
    }

    fn commit(&mut self, mut changeset: Changeset, mut along: bool) -> Result<Committed> {
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
            along = false;
        }
        if along {
            let along = self.versions_along(&changeset)?;
            changeset.ops.extend(along);
        }
        retract::refuse_placing(&self.store.conn, &changeset.ops)?;
        let validated = validate(&self.store, changeset)?;
        Ok(self.store.apply(validated)?)
    }

    fn author(
        &mut self,
        actor: Actor,
        reason: Option<String>,
        ops: Vec<Op>,
        commit: bool,
        along: bool,
    ) -> Result<Authored> {
        let mut changeset = Changeset {
            id: lock(&self.ids).changeset_id(),
            base: self.store.head()?,
            actor,
            reason,
            run: None,
            ops,
        };
        if along {
            let along = self.versions_along(&changeset)?;
            changeset.ops.extend(along);
        }
        retract::refuse_placing(&self.store.conn, &changeset.ops)?;
        let validated = validate(&self.store, changeset.clone())?;
        let committed = match commit {
            true => Some(self.store.apply(validated)?),
            false => None,
        };
        Ok(Authored {
            changeset,
            committed,
        })
    }

    fn undo(
        &mut self,
        seq: Seq,
        actor: Actor,
        reason: Option<String>,
        commit: bool,
    ) -> Result<Authored> {
        let entry = match seq.0.checked_sub(1) {
            Some(before) => store::read_log_after(&self.store.conn, Seq(before), 1)?.pop(),
            None => None,
        }
        .ok_or(Error::UnknownChangeset(seq))?;
        let ops = inverse(&self.store, &entry)?;
        if ops.is_empty() {
            return Err(Error::NothingToUndo(seq));
        }
        let reason = reason.unwrap_or_else(|| format!("undo of the changeset at position {seq}"));
        // An undo puts back what the changeset moved and nothing else: a
        // version it left where it was stays there.
        self.author(actor, Some(reason), ops, commit, false)
    }

    fn split(
        &mut self,
        identity: IdentityId,
        records: &[RecordId],
        keep_apart: bool,
        base: Option<Seq>,
        actor: Actor,
        reason: Option<String>,
    ) -> Result<SplitReport> {
        let refuse = |message: String| Error::Split { identity, message };
        let entry = self
            .store
            .identities(&[identity])?
            .remove(0)
            .ok_or(Error::UnknownIdentity(identity))?;
        if let IdentityStatus::MergedInto(survivor) = entry.status {
            return Err(refuse(format!(
                "it was merged into {survivor}, which has its members now"
            )));
        }
        if let Some(base) = base {
            let head = self.store.head()?;
            if base > head {
                return Err(refuse(format!(
                    "the split is based on log position {base}, and the log ends at {head}"
                )));
            }
            let changed = self.store.membership_changed(&[identity])?[0];
            if changed > base {
                return Err(refuse(format!(
                    "its members changed at log position {changed}, after log position {base} that the split is based on"
                )));
            }
        }
        let members: BTreeSet<RecordId> = self
            .store
            .members(&[identity])?
            .into_iter()
            .map(|(_, record)| record)
            .collect();
        let named: BTreeSet<RecordId> = records.iter().copied().collect();
        if named.is_empty() {
            return Err(refuse("no records were named".to_owned()));
        }
        if let Some(stranger) = named.difference(&members).next() {
            return Err(refuse(format!("record {stranger} does not belong to it")));
        }
        let rest: BTreeSet<RecordId> = members.difference(&named).copied().collect();
        if rest.is_empty() {
            return Err(refuse(
                "every member was named, and a split leaves records on both sides".to_owned(),
            ));
        }
        // The larger side keeps the ID: more of what refers to the identity
        // is likely to mean that side.
        let (moved, kept) = if named.len() > rest.len() {
            (rest, named)
        } else {
            (named, rest)
        };

        let new = lock(&self.ids).identity_id();
        let mut ops = vec![Op::CreateIdentity {
            identity: new,
            entity_type: entry.entity_type,
        }];
        ops.extend(moved.iter().map(|record| Op::Assign {
            record: *record,
            from: Some(identity),
            to: Some(new),
        }));
        if keep_apart {
            // One constraint between the sides is enough to stop a merge. If
            // one is there already, nothing is added.
            let moved_list: Vec<RecordId> = moved.iter().copied().collect();
            let apart = self.store.constraints_on(&moved_list)?.iter().any(|link| {
                link.kind == ConstraintKind::CannotLink
                    && (kept.contains(&link.a) || kept.contains(&link.b))
            });
            if !apart && let (Some(a), Some(b)) = (moved.first(), kept.first()) {
                ops.push(Op::Constrain {
                    kind: ConstraintKind::CannotLink,
                    a: *a,
                    b: *b,
                });
            }
        }
        // A split moves the records it was given and no other: the
        // reviewer named both sides, versions of one row included.
        let authored = self.author(actor, reason, ops, true, false)?;
        Ok(SplitReport {
            kept: identity,
            identity: new,
            moved: moved.into_iter().collect(),
            committed: authored
                .committed
                .expect("the changeset was committed above"),
            changeset: authored.changeset,
        })
    }

    fn open_draft(&self, draft: ChangesetId, verb: &str) -> Result<DraftEntry> {
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
        // A draft is approved as it was stored and shown: the run that
        // drafted it moved the earlier versions along already.
        let changeset = self.open_draft(draft, "approved")?.changeset;
        retract::refuse_placing(&self.store.conn, &changeset.ops)?;
        let validated = validate(&self.store, changeset)?;
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

    fn apply_recompute(
        &mut self,
        by: &Actor,
        entity_type: Option<&EntityType>,
        plan: &str,
        reason: Option<String>,
    ) -> Result<RecomputeApplied> {
        let manifest = self.manifest.clone().ok_or(Error::NoManifest)?;
        let names: Vec<EntityType> = recomputed_types(&manifest, entity_type)?
            .into_iter()
            .map(|entry| entry.name.clone())
            .collect();
        let mut worked = Vec::new();
        for name in &names {
            let (manifest, entry, version) = self.declared(name.as_str())?;
            let config = MatchConfig {
                entity_type: name.clone(),
                declaration_version: version,
                manifest_hash: manifest.hash.clone(),
                after_run: self.last_match_run(name)?,
                review_items: review::open_ids(&self.store.conn, name)?,
                rematch: true,
                recompute: true,
                rules: entry.rules.clone(),
            };
            let run = self.start_run("match", by, Some(&StrategyRef::rules()), &json!(config))?;
            let outcome = self.in_run(run, |inner| {
                let tx = inner.store.conn.transaction()?;
                recompute::create_scope(&tx, name, run)?;
                rules::generate_candidates(&tx, name, run, &config.rules.block)?;
                rules::judge_candidates(
                    &tx,
                    name,
                    run,
                    &config.rules.no_match,
                    &config.rules.match_,
                )?;
                tx.commit()?;
                recompute::compute(
                    &inner.store.conn,
                    name,
                    &format!("(SELECT * FROM spool.judgement WHERE run_id = {run})"),
                )
            })?;
            worked.push((run, outcome));
        }
        let plans: Vec<String> = worked.iter().map(|(_, outcome)| outcome.plan()).collect();
        let now = recompute::combined_plan(&plans);
        if now != plan {
            for (run, _) in &worked {
                self.fail_run(
                    *run,
                    &format!("plan {plan} is not what it found, plan {now}"),
                )?;
            }
            return Err(Error::StalePlan {
                plan: plan.to_owned(),
                now,
            });
        }

        let mut applied = RecomputeApplied {
            plan: now.clone(),
            types: Vec::new(),
        };
        for (run, outcome) in worked {
            let entity_type = outcome.entity_type().clone();
            let reconcile_run = self.start_run(
                "reconcile",
                by,
                None,
                &json!({ "match_run": run, "policy": Policy::default(), "recompute": now }),
            )?;
            let report = self.in_run(reconcile_run, |inner| {
                let (ops, identities) = {
                    let mut ids = lock(&inner.ids);
                    outcome.operations(recompute::mint(ids.as_mut()))
                };
                let changeset = Changeset {
                    id: lock(&inner.ids).changeset_id(),
                    base: inner.store.head()?,
                    actor: Actor::Engine,
                    reason: Some(match &reason {
                        Some(reason) => format!("recompute (plan {now}): {reason}"),
                        None => format!("recompute under the current rules (plan {now})"),
                    }),
                    run: Some(reconcile_run),
                    ops,
                };
                let review = outcome.review(&identities);
                let summary = outcome.summary();
                let finished = inner.store.clock.now_micros();
                let tx = inner.store.conn.transaction()?;
                review::record(&tx, &entity_type, reconcile_run, &review)?;
                drafts::supersede(&tx, &entity_type, reconcile_run)?;
                tx.execute(
                    "UPDATE spool.run SET status = 'succeeded', finished_at = make_timestamptz(?)
                     WHERE run_id = ?",
                    params![finished, reconcile_run.0 as i64],
                )?;
                tx.commit()?;
                let committed = match changeset.ops.is_empty() {
                    true => None,
                    false => {
                        retract::refuse_placing(&inner.store.conn, &changeset.ops)?;
                        let validated = validate(&inner.store, changeset)?;
                        Some(inner.store.apply(validated)?)
                    }
                };
                Ok(AppliedType {
                    entity_type: entity_type.clone(),
                    match_run: run,
                    reconcile_run,
                    summary,
                    committed,
                    review,
                })
            });
            let report = match report {
                Ok(report) => report,
                Err(error) => {
                    self.fail_run(run, &error.to_string())?;
                    return Err(error);
                }
            };
            let finished = self.store.clock.now_micros();
            let tx = self.store.conn.transaction()?;
            tx.execute(
                "UPDATE spool.run
                 SET status = 'succeeded', finished_at = make_timestamptz(?), result = ?::JSON
                 WHERE run_id = ?",
                params![finished, json!(report).to_string(), run.0 as i64],
            )?;
            possible_split::supersede_answered(&tx, &entity_type, reconcile_run, run)?;
            tx.commit()?;
            applied.types.push(report);
        }
        Ok(applied)
    }

    /// The entity types a resolve covers, each with the strategy it runs.
    /// Nothing is started if a type lacks the strategy that was named.
    fn plan(&self, options: &ResolveOptions) -> Result<Vec<(EntityType, Chosen)>> {
        let manifest = self.manifest.as_deref().ok_or(Error::NoManifest)?;
        let entries: Vec<&EntityTypeManifest> = match &options.entity_type {
            Some(entity_type) => vec![self.declared(entity_type.as_str())?.1],
            None => manifest.entity_types.iter().collect(),
        };
        entries
            .into_iter()
            .map(|entry| {
                let chosen = match options.strategy.as_deref() {
                    None | Some(StrategyRef::RULES) => Chosen::Rules,
                    Some(name) => match entry.strategy(name) {
                        Some(strategy) => Chosen::UserCode(strategy.clone()),
                        None => {
                            return Err(Error::UnknownStrategy {
                                entity_type: entry.name.to_string(),
                                strategy: name.to_owned(),
                                known: entry
                                    .strategies
                                    .iter()
                                    .map(|strategy| strategy.name.clone())
                                    .collect(),
                            });
                        }
                    },
                };
                Ok((entry.name.clone(), chosen))
            })
            .collect()
    }

    /// Whether a type has anything a run would examine: records
    /// loaded since its last match run that succeeded, records that
    /// came back since then after their source had taken them back, or open
    /// review items with records that wait for an identity.
    fn has_work(&self, entity_type: &EntityType) -> Result<bool> {
        let after_run = self.last_match_run(entity_type)?;
        let loaded = self.count(&format!(
            "SELECT count(*) FROM spool.record
             WHERE entity_type = '{entity_type}'
               AND (load_run > {after_run} OR (
                 record_id IN (
                   SELECT record_id FROM spool.retraction WHERE restored_run > {after_run})
                 AND record_id NOT IN (
                   SELECT record_id FROM spool.membership WHERE to_seq IS NULL)))
               AND {} AND {}",
            rules::NOT_DISMISSED,
            retract::NOT_RETRACTED
        ))?;
        Ok(loaded > 0 || !review::open_ids(&self.store.conn, entity_type)?.is_empty())
    }

    /// Writes what a user-code strategy is given: the run's candidate pairs
    /// with both sides' features, and the feature row of every record
    /// of the type, with whether it is new to the run and the identity it
    /// has.
    fn write_task(
        &mut self,
        run: RunId,
        strategy: &Strategy,
        user_code: &UserCode,
    ) -> Result<Task> {
        let config = self.judged_run(run, RunStatus::Running)?;
        let columns = self
            .manifest
            .as_deref()
            .and_then(|manifest| manifest.entity_type(config.entity_type.as_str()))
            .map(|entry| entry.declaration.columns.clone())
            .unwrap_or_default();
        let task = Task::create(
            user_code,
            strategy,
            &TaskInfo {
                run,
                entity_type: &config.entity_type,
                declaration_version: config.declaration_version,
                rematch: config.rematch,
                features: &columns,
            },
        )?;
        let conn = &self.store.conn;
        let table = features::table(&config.entity_type);
        let columns = feature_columns(conn, &table)?;
        let copy = |query: &str, file: &str| {
            conn.execute_batch(&format!(
                "COPY (\n{query}\n) TO {} (FORMAT parquet)",
                usercode::sql_path(&task.path(file))
            ))
        };
        copy(
            &candidates_query(&table, &columns, run, true),
            usercode::CANDIDATES,
        )?;
        copy(
            &format!(
                "SELECT f.record_id::VARCHAR AS record_id,
                        f.record_id IN (SELECT record_id FROM {scope}) AS is_new,
                        m.identity_id::VARCHAR AS identity_id{features}
                 FROM {table} f
                 LEFT JOIN spool.membership m
                   ON m.record_id = f.record_id AND m.to_seq IS NULL
                 ORDER BY f.record_id",
                scope = rules::SCOPE,
                features = select_features(&columns, "f", "", true),
            ),
            usercode::FEATURES,
        )?;
        Ok(task)
    }

    /// Reads the judgements a user-code strategy wrote and stores them,
    /// under the same checks as judgements a caller submits.
    fn take_judgements(&mut self, run: RunId, task: &Task) -> Result<u64> {
        let rows = {
            let mut statement = self.store.conn.prepare(&format!(
                "SELECT * FROM read_parquet({})",
                usercode::sql_path(&task.path(usercode::JUDGEMENTS))
            ))?;
            let arrow = statement.query_arrow([])?;
            let schema = arrow.get_schema();
            Rows::new(schema, arrow.collect())
        };
        self.store_judgements(run, rows, RunStatus::Running)
    }

    /// Where a rematch's judgements disagree with the identities as they
    /// are. A pair a constraint has spoken for is left out: a reviewer
    /// decided it, and the strategy's view of it is no suggestion.
    fn suggestions(&self, run: RunId) -> Result<Suggestions> {
        let conn = &self.store.conn;
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE spool_rejudged AS
             SELECT j.verdict, a.identity_id AS left_identity, b.identity_id AS right_identity
             FROM (
               SELECT DISTINCT left_id, right_id, verdict FROM spool.judgement
               WHERE run_id = {run} AND verdict <> 'unsure'
             ) j
             JOIN spool.membership a ON a.record_id = j.left_id AND a.to_seq IS NULL
             JOIN spool.membership b ON b.record_id = j.right_id AND b.to_seq IS NULL
             WHERE NOT EXISTS (
               SELECT 1 FROM spool.constraint_link c
               WHERE c.to_seq IS NULL
                 AND least(c.record_a, c.record_b) = j.left_id
                 AND greatest(c.record_a, c.record_b) = j.right_id)"
        ))?;
        let mut suggestions = Suggestions::default();

        // Two identities a `cannot_link` keeps apart cannot be merged, so
        // merging them is not suggested.
        let mut statement = conn.prepare(
            "WITH apart AS (
               SELECT a.identity_id AS one, b.identity_id AS other
               FROM spool.constraint_link c
               JOIN spool.membership a ON a.record_id = c.record_a AND a.to_seq IS NULL
               JOIN spool.membership b ON b.record_id = c.record_b AND b.to_seq IS NULL
               WHERE c.to_seq IS NULL AND c.kind = 'cannot_link'
             )
             SELECT least(left_identity, right_identity)::VARCHAR,
                    greatest(left_identity, right_identity)::VARCHAR,
                    count(*) FILTER (WHERE verdict = 'match'),
                    count(*) FILTER (WHERE verdict = 'no_match')
             FROM spool_rejudged r
             WHERE left_identity <> right_identity
               AND NOT EXISTS (
                 SELECT 1 FROM apart
                 WHERE (one = left_identity AND other = right_identity)
                    OR (one = right_identity AND other = left_identity))
             GROUP BY 1, 2
             HAVING count(*) FILTER (WHERE verdict = 'match') > 0
             ORDER BY 1, 2",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            suggestions.merges.push(SuggestedMerge {
                identities: [
                    row.get::<_, String>(0)?.parse().map_err(id_error)?,
                    row.get::<_, String>(1)?.parse().map_err(id_error)?,
                ],
                matches: row.get::<_, i64>(2)? as u64,
                no_matches: row.get::<_, i64>(3)? as u64,
            });
        }

        let mut statement = conn.prepare(
            "SELECT left_identity::VARCHAR,
                    count(*) FILTER (WHERE verdict = 'no_match'),
                    count(*) FILTER (WHERE verdict = 'match')
             FROM spool_rejudged
             WHERE left_identity = right_identity
             GROUP BY 1
             HAVING count(*) FILTER (WHERE verdict = 'no_match') > 0
             ORDER BY 1",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            suggestions.splits.push(SuggestedSplit {
                identity: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                no_matches: row.get::<_, i64>(1)? as u64,
                matches: row.get::<_, i64>(2)? as u64,
            });
        }
        Ok(suggestions)
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
                "record IDs that occur more than once".to_owned(),
                "SELECT count(*) FROM (
                   SELECT record_id FROM spool.record GROUP BY 1 HAVING count(*) > 1)"
                    .to_owned(),
            ),
            (
                "changeset IDs that occur more than once".to_owned(),
                "SELECT count(*) FROM (
                   SELECT changeset_id FROM spool.changeset GROUP BY 1 HAVING count(*) > 1)"
                    .to_owned(),
            ),
            (
                "records that belong to more than one identity".to_owned(),
                "SELECT count(*) FROM (
                   SELECT record_id FROM spool.membership WHERE to_seq IS NULL
                   GROUP BY 1 HAVING count(*) > 1)"
                    .to_owned(),
            ),
            (
                "memberships of records the store does not hold".to_owned(),
                "SELECT count(*) FROM spool.membership m
                 WHERE NOT EXISTS (
                   SELECT 1 FROM spool.record o WHERE o.record_id = m.record_id)"
                    .to_owned(),
            ),
            (
                "current memberships in an identity that is missing, merged, or of another entity type"
                    .to_owned(),
                "SELECT count(*) FROM spool.membership m
                 JOIN spool.record o ON o.record_id = m.record_id
                 LEFT JOIN spool.identity i ON i.identity_id = m.identity_id
                 WHERE m.to_seq IS NULL
                   AND (i.identity_id IS NULL OR i.merged_into IS NOT NULL
                        OR i.entity_type <> o.entity_type)"
                    .to_owned(),
            ),
            (
                "open review items none of whose records waits for an identity".to_owned(),
                format!(
                    "SELECT count(*) FROM spool.review_item i
                     WHERE i.status = 'open' AND {} AND i.review_id NOT IN ({})",
                    review::ABOUT_WAITING,
                    retract::WAITING
                ),
            ),
            (
                "`possible_split` review items that do not name one identity and two groups of its records, or review items of another kind that have groups"
                    .to_owned(),
                "SELECT count(*) FROM spool.review_item i
                 WHERE CASE WHEN i.kind = 'possible_split'
                   THEN len(i.identity_ids) <> 1 OR i.groups IS NULL OR len(i.groups) < 2
                        OR list_sort(flatten(i.groups)) <> list_sort(i.record_ids)
                   ELSE i.groups IS NOT NULL END"
                    .to_owned(),
            ),
            (
                "records that were taken back and belong to an identity".to_owned(),
                "SELECT count(*) FROM spool.retraction t
                 JOIN spool.membership m ON m.record_id = t.record_id AND m.to_seq IS NULL
                 WHERE t.restored_run IS NULL"
                    .to_owned(),
            ),
            (
                "records that are taken back more than once".to_owned(),
                "SELECT count(*) FROM (
                   SELECT record_id FROM spool.retraction WHERE restored_run IS NULL
                   GROUP BY 1 HAVING count(*) > 1)"
                    .to_owned(),
            ),
            (
                "retractions of records the store does not hold".to_owned(),
                "SELECT count(*) FROM spool.retraction t
                 WHERE NOT EXISTS (
                   SELECT 1 FROM spool.record o WHERE o.record_id = t.record_id)"
                    .to_owned(),
            ),
            (
                "retractions that name a changeset that did not take the record out of an identity"
                    .to_owned(),
                "SELECT count(*) FROM spool.retraction t
                 WHERE t.detached_seq IS NOT NULL AND NOT EXISTS (
                   SELECT 1 FROM spool.membership m
                   WHERE m.record_id = t.record_id AND m.to_seq = t.detached_seq)"
                    .to_owned(),
            ),
            (
                "open drafts that place a record that was taken back".to_owned(),
                "SELECT count(*) FROM spool.draft d
                 WHERE d.status = 'open' AND EXISTS (
                   SELECT 1 FROM spool.draft_op o
                   JOIN spool.retraction t
                     ON t.record_id = o.record_id AND t.restored_run IS NULL
                   WHERE o.changeset_id = d.changeset_id AND o.kind = 'assign'
                     AND o.identity_id IS NOT NULL)"
                    .to_owned(),
            ),
        ];
        checks.extend([
            (
                "merged identities that stand for an identity that is missing or merged".to_owned(),
                "SELECT count(*) FROM spool.identity i
                 LEFT JOIN spool.identity t ON t.identity_id = i.merged_into
                 WHERE i.merged_into IS NOT NULL
                   AND (t.identity_id IS NULL OR t.merged_into IS NOT NULL
                        OR t.entity_type <> i.entity_type)"
                    .to_owned(),
            ),
            (
                "constraints in force that current membership breaks".to_owned(),
                "SELECT count(*) FROM spool.constraint_link c
                 JOIN spool.membership a ON a.record_id = c.record_a AND a.to_seq IS NULL
                 JOIN spool.membership b ON b.record_id = c.record_b AND b.to_seq IS NULL
                 WHERE c.to_seq IS NULL
                   AND (a.identity_id = b.identity_id) = (c.kind = 'cannot_link')"
                    .to_owned(),
            ),
            (
                "pairs of records with more than one constraint in force".to_owned(),
                "SELECT count(*) FROM (
                   SELECT record_a, record_b FROM spool.constraint_link
                   WHERE to_seq IS NULL GROUP BY 1, 2 HAVING count(*) > 1)"
                    .to_owned(),
            ),
            (
                "records superseded by more than one record".to_owned(),
                "SELECT count(*) FROM (
                   SELECT supersedes FROM spool.record WHERE supersedes IS NOT NULL
                   GROUP BY 1 HAVING count(*) > 1)"
                    .to_owned(),
            ),
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
                format!("`{entity_type}` records with more than one feature row"),
                format!(
                    "SELECT count(*) FROM (
                       SELECT record_id FROM spool.features_{entity_type}
                       GROUP BY 1 HAVING count(*) > 1)"
                ),
            ));
            checks.push((
                format!("`{entity_type}` records that were taken back and have a feature row"),
                format!(
                    "SELECT count(*) FROM spool.features_{entity_type} f
                     JOIN spool.retraction t
                       ON t.record_id = f.record_id AND t.restored_run IS NULL"
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
        // An item is closed in the transaction that answers it, so none
        // stays open once what the store knows holds its groups together or
        // they no longer share the identity, and an identity has one at
        // most.
        let stale = possible_split::stale(&self.store.conn)?;
        if stale > 0 {
            problems.push(format!(
                "{stale} open `possible_split` review items with nothing left to ask, or that ask what another asks"
            ));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Verify(problems))
        }
    }
}
