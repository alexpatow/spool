//! The engine: one type that owns the store and is the whole API.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use duckdb::{Connection, params};
use spool_core::{
    Actor, Clock, DocumentId, IdGenerator, RandomIds, RunId, SeededIds, StepClock, SystemClock,
};

use crate::error::{Error, Result};
use crate::project::{Manifest, hash_json};
use crate::runs::RunStatus;
use crate::schema;
use crate::store::{self, DuckStore};
use crate::usercode::UserCode;

mod acting;
mod apply;
mod changes;
mod extract;
mod load;
mod matching;
mod mentions;
mod read;
mod recompute;
mod reconcile;
mod resolve;
mod verify;

pub use acting::Acting;
pub use apply::ApplyReport;
pub use changes::{Authored, ConstraintFilter, MintedIds, SplitReport};
pub use load::AddRecordReport;
pub use matching::{Scope, StrategyRef};
pub use reconcile::{
    Disposition, JudgementCounts, SuggestedMerge, SuggestedSplit, Suggestions, TypeResolution,
};
pub use resolve::{ResolveOptions, ResolveReport};

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
        let idle = lock_ignoring_poison(&self.idle).pop();
        let conn = match idle {
            Some(conn) => conn,
            None => store::open_reader(&lock_ignoring_poison(&self.source), self.attached)?,
        };
        // One transaction, so a read made of several queries sees one
        // committed state however many changesets commit while it runs.
        conn.execute_batch("BEGIN TRANSACTION")?;
        let result = read(&conn);
        // A read has nothing to commit. A connection whose transaction did
        // not end is not kept for the next read.
        if conn.execute_batch("ROLLBACK").is_ok() {
            lock_ignoring_poison(&self.idle).push(conn);
        }
        result
    }
}

fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
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
        lock_ignoring_poison(&self.inner)
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

    /// The manifest this engine runs on, if one was applied or adopted.
    pub fn manifest(&self) -> Option<Arc<Manifest>> {
        self.manifest
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The hash of the manifest the store was last applied with.
    pub fn applied_manifest_hash(&self) -> Result<Option<String>> {
        self.read(|conn| schema::read_meta(conn, "manifest_hash"))
    }
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

/// An ID read from the store could not be parsed, which means the store is
/// corrupt.
fn id_error(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::State(spool_core::StateError::new(error))
}

impl Inner {
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

    fn count(&self, sql: &str) -> Result<u64> {
        let count: i64 = self.store.conn.query_row(sql, [], |row| row.get(0))?;
        Ok(count as u64)
    }
}
