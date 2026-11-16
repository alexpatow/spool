pub mod bench;
pub mod project;
pub mod serve;

use std::path::{Path, PathBuf};

use crate::Failure;
use crate::args::{self, Arguments, Spec};

/// Parses a command's arguments, turning a mistake into a usage failure.
fn arguments(arguments: &[String], spec: &Spec) -> Result<Arguments, Failure> {
    args::parse(arguments, spec).map_err(Failure::Usage)
}

/// Where `spool build` writes the manifest of a project.
fn manifest_path(project: &Path) -> PathBuf {
    project.join(".spool").join("manifest.json")
}

/// The store's database file: `--store`, or `$SPOOL_STORE`.
fn store_path(arguments: &Arguments) -> Result<PathBuf, Failure> {
    arguments
        .path("--store")
        .or_else(|| {
            std::env::var_os("SPOOL_STORE")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
        .ok_or_else(|| {
            Failure::Usage("say where the store is with `--store` or `SPOOL_STORE`".to_owned())
        })
}
