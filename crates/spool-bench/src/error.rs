//! What stops a scenario.

use std::path::PathBuf;

use spool::arrow::error::ArrowError;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The engine refused what a step asked of it.
    #[error(transparent)]
    Engine(#[from] spool::Error),

    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    /// The scenario, one of its files or one of its datasets is wrong, or a
    /// step left what the scenario does not allow.
    #[error("{0}")]
    Scenario(String),
}

impl From<ArrowError> for Error {
    fn from(error: ArrowError) -> Self {
        Error::Engine(error.into())
    }
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Error {
        let path = path.into();
        move |source| Error::Io { path, source }
    }
}
