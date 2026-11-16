use std::path::PathBuf;

use spool_core::{
    ChangesetId, IdentityId, Rejection, ReplayError, RunId, StateError, ValidationError,
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

    #[error("the store has format version {found}, and this engine reads version {expected}")]
    FormatVersion { found: String, expected: String },

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

    #[error("ingest rejected: {0}")]
    Ingest(String),

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

    #[error("rows rejected: {0}")]
    Rows(String),

    #[error("judgements rejected: {0}")]
    Judgements(String),

    /// The changeset broke a rule. Each rejection names the operation at fault.
    #[error("changeset rejected: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Rejected(Vec<Rejection>),

    #[error("identity {0} does not exist")]
    UnknownIdentity(IdentityId),

    #[error(transparent)]
    State(#[from] StateError),

    #[error("the changeset log cannot be replayed: {0}")]
    Replay(#[from] ReplayError),

    #[error("the store failed verification: {}", .0.join("; "))]
    Verify(Vec<String>),

    #[error("query rejected: {0}")]
    Query(String),

    #[error("{0}")]
    Scenario(String),
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
