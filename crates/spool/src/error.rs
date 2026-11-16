use std::path::PathBuf;

use spool_core::{IdentityId, Rejection, ReplayError, RunId, StateError, ValidationError};

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

    #[error(
        "the declaration of entity type `{0}` differs from the one this store was built with; changing a declaration is not supported yet"
    )]
    DeclarationChanged(String),

    #[error("no manifest has been applied to this engine")]
    NoManifest,

    #[error("ingest rejected: {0}")]
    Ingest(String),

    #[error("run {run}: {message}")]
    Run { run: RunId, message: String },

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
