use std::path::PathBuf;

use spool_core::{
    ChangesetId, DocumentId, IdentityId, RecordId, Rejection, ReplayError, RunId, Seq, StateError,
    ValidationError,
};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Duckdb(#[from] duckdb::Error),

    #[error(transparent)]
    Arrow(#[from] duckdb::arrow::error::ArrowError),

    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    /// A file in the project directory is missing or wrong.
    #[error("{}: {message}", file.display())]
    Project { file: PathBuf, message: String },

    #[error(
        "the store has format version {found}, which this engine cannot open (current format: {expected})"
    )]
    FormatVersion { found: String, expected: String },

    /// Another process has the store's file open. `holder` is that
    /// process's ID, where the system tells.
    #[error(
        "the store at {} is open in another process{}. One process opens a store at a time: stop that one first.",
        path.display(),
        match holder { Some(pid) => format!(" (PID {pid})"), None => String::new() }
    )]
    StoreInUse { path: PathBuf, holder: Option<u32> },

    #[error(transparent)]
    InvalidEntityType(#[from] spool_core::InvalidEntityType),

    #[error("entity type `{0}` is not declared; apply a manifest that declares it first")]
    UndeclaredType(String),

    #[error("no manifest has been applied to this engine")]
    NoManifest,

    /// The store was built with another manifest than the one it is asked to
    /// run on. `applied` is `None` when nothing was ever applied to it.
    #[error(
        "the store has {} and was asked to run on manifest {manifest}; apply the manifest to the store first",
        match applied { Some(hash) => format!("manifest {hash} applied"), None => "no manifest applied".to_owned() }
    )]
    ManifestMismatch {
        applied: Option<String>,
        manifest: String,
    },

    #[error("apply rejected: {0}")]
    Apply(String),

    #[error("load rejected: {0}")]
    Load(String),

    #[error("retraction rejected: {0}")]
    Retract(String),

    /// A changeset would give an identity to a record its source took
    /// back.
    #[error(
        "record {0} was taken back by its source and cannot be given an identity; it comes back when the source sends the row again"
    )]
    Retracted(RecordId),

    #[error("there is no run {0}")]
    UnknownRun(RunId),

    /// The run exists and cannot do what was asked in the state it is in.
    #[error("run {run}: {message}")]
    Run { run: RunId, message: String },

    #[error("there is no draft {0}")]
    UnknownDraft(ChangesetId),

    /// The draft exists and cannot do what was asked in the state it is in.
    #[error("draft {draft}: {message}")]
    Draft { draft: ChangesetId, message: String },

    /// The entity type has no strategy of that name.
    #[error(
        "entity type `{entity_type}` has no strategy `{strategy}`; it has {}",
        match known.as_slice() {
            [] => "only the built-in `rules`".to_owned(),
            names => format!("`rules` and {}", names.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ")),
        }
    )]
    UnknownStrategy {
        entity_type: String,
        strategy: String,
        /// The user-code strategies the type does have.
        known: Vec<String>,
    },

    /// A user-code strategy could not be run, or what it wrote was refused.
    #[error("strategy `{strategy}`: {message}")]
    Strategy { strategy: String, message: String },

    /// The project has no extractor of that name.
    #[error(
        "the project has no extractor `{extractor}`; it has {}",
        match known.as_slice() {
            [] => "none".to_owned(),
            names => names.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", "),
        }
    )]
    UnknownExtractor {
        extractor: String,
        known: Vec<String>,
    },

    /// An extractor could not be run, or what it wrote was refused.
    #[error("extractor `{extractor}`: {message}")]
    Extractor { extractor: String, message: String },

    /// The manifest names user code and the engine was not told where the
    /// project's files are.
    #[error(
        "strategy `{0}` is user code, and this engine was not told where the project directory is"
    )]
    NoUserCode(String),

    #[error("there is no document {0}")]
    UnknownDocument(DocumentId),

    #[error("documents rejected: {0}")]
    Documents(String),

    #[error("mentions rejected: {0}")]
    Mentions(String),

    #[error("rows rejected: {0}")]
    Rows(String),

    #[error("judgements rejected: {0}")]
    Judgements(String),

    /// The changeset broke a rule. Each rejection names the operation at fault.
    #[error("changeset rejected: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Rejected(Vec<Rejection>),

    #[error("identity {0} does not exist")]
    UnknownIdentity(IdentityId),

    #[error("identity {id} did not exist at changeset {seq}. It was created at {created}")]
    IdentityNotYetCreated {
        id: IdentityId,
        seq: Seq,
        created: Seq,
    },

    #[error("changeset {seq} is beyond the store head at {head}")]
    FutureIdentitySnapshot { seq: Seq, head: Seq },

    #[error("there is no record {0}")]
    UnknownRecord(RecordId),

    /// An explanation is about two records.
    #[error("{0} was named twice, and an explanation is about two records")]
    SameRecord(RecordId),

    #[error("the log has no changeset at position {0}")]
    UnknownChangeset(Seq),

    /// The changeset exists and there is nothing in it to take back.
    #[error(
        "the changeset at position {0} only created identities, and an identity ID is never taken back"
    )]
    NothingToUndo(Seq),

    /// The identity exists and cannot be split the way that was asked.
    #[error("identity {identity} cannot be split: {message}")]
    Split {
        identity: IdentityId,
        message: String,
    },

    #[error("recompute: {0}")]
    Recompute(String),

    /// What recomputing finds now is not what the plan named.
    #[error(
        "plan {plan} is not what recomputing finds now, which is plan {now}: the records, the rules or the identities changed since it was shown, and nothing was applied. `spool recompute` shows what would change now"
    )]
    StalePlan { plan: String, now: String },

    #[error("there is no review item {0}")]
    UnknownReviewItem(u64),

    /// The review item exists and cannot do what was asked in the state it
    /// is in.
    #[error("review item {id}: {message}")]
    Review { id: u64, message: String },

    #[error(transparent)]
    State(#[from] StateError),

    #[error("the changeset log cannot be replayed: {0}")]
    Replay(#[from] ReplayError),

    #[error("the store failed verification: {}", .0.join("; "))]
    Verify(Vec<String>),

    #[error("query rejected: {0}")]
    Query(String),
}

impl From<ValidationError> for Error {
    fn from(error: ValidationError) -> Self {
        match error {
            ValidationError::Rejected(rejections) => Error::Rejected(rejections),
            ValidationError::State(error) => Error::State(error),
        }
    }
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Error {
        let path = path.into();
        move |source| Error::Io { path, source }
    }
}
