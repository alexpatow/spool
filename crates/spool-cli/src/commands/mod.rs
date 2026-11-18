pub mod bench;
pub mod extract;
pub mod init;
pub mod load;
pub mod matching;
pub mod project;
mod remote;
pub mod retract;
pub mod serve;
pub mod show;
pub mod token;

use std::path::{Path, PathBuf};
use std::time::Duration;

use spool::UserCode;
use spool_cli::accounts::Accounts;

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

/// Where the accounts of the server over `store` are: a file beside it,
/// named after it.
fn accounts_path(store: &Path) -> PathBuf {
    store.with_extension("auth.sqlite")
}

fn open_accounts(store: &Path) -> Result<Accounts, Failure> {
    let path = accounts_path(store);
    Accounts::open(&path)
        .map_err(|refused| Failure::Failed(format!("{}: {refused}", path.display())))
}

/// How user code of the project at `project` is run: `--task-timeout` and
/// `--tasks`, and the `uv` and `bun` that `SPOOL_UV` and `SPOOL_BUN` name.
fn user_code(arguments: &Arguments, project: &Path) -> Result<UserCode, Failure> {
    let mut user_code = UserCode::new(project);
    if let Some(seconds) = arguments.number("--task-timeout").map_err(Failure::Usage)? {
        if seconds == 0 {
            return Err(Failure::Usage(
                "`--task-timeout` needs at least one second".to_owned(),
            ));
        }
        user_code.timeout = Duration::from_secs(seconds);
    }
    if let Some(dir) = arguments.path("--tasks") {
        user_code.work_dir = dir;
    }
    let named = |variable: &str| std::env::var_os(variable).filter(|value| !value.is_empty());
    if let Some(uv) = named("SPOOL_UV") {
        user_code.uv = uv;
    }
    if let Some(bun) = named("SPOOL_BUN") {
        user_code.bun = bun;
    }
    Ok(user_code)
}

pub const SCHEMA_USAGE: &str = "\
Usage: spool schema

Prints the JSON Schema of every value the server's API takes and returns.
Client types are generated from it.
";

pub fn schema(arguments: &[String]) -> Result<(), Failure> {
    self::arguments(
        arguments,
        &Spec {
            values: &[],
            flags: &[],
            positional: 0,
        },
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&spool_cli::api::schema())
            .expect("a schema serialises to JSON")
    );
    Ok(())
}
