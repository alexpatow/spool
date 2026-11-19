pub mod bench;
pub mod extract;
pub mod init;
pub mod load;
pub mod matching;
pub mod project;
pub mod recompute;
mod remote;
pub mod reset;
pub mod retract;
pub mod serve;
pub mod show;
pub mod token;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anstream::println;
use clap::Args;
use spool::UserCode;
use spool_cli::accounts::Accounts;

use crate::Failure;

/// Where `spool build` writes the manifest of a project.
fn manifest_path(project: &Path) -> PathBuf {
    project.join(".spool").join("manifest.json")
}

/// The project directory: the one named, or the current directory.
fn project_dir(dir: &Option<PathBuf>) -> PathBuf {
    dir.clone().unwrap_or_else(|| PathBuf::from("."))
}

/// The store's database file, from `--store` or `SPOOL_STORE`, which may be
/// set and empty.
fn store_path(store: &Option<PathBuf>) -> Result<PathBuf, Failure> {
    store
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            Failure::usage("say where the store is with `--store` or `SPOOL_STORE`".to_owned())
        })
}

/// A path that may be empty, as a variable that is set and empty is: the
/// command then says what it needs, as it does when nothing is set.
fn any_path() -> impl clap::builder::TypedValueParser<Value = PathBuf> {
    use clap::builder::TypedValueParser;
    clap::builder::OsStringValueParser::new().map(PathBuf::from)
}

/// Where the accounts of the server over `store` are: a file beside it,
/// named after it.
fn accounts_path(store: &Path) -> PathBuf {
    store.with_extension("auth.sqlite")
}

fn open_accounts(store: &Path) -> Result<Accounts, Failure> {
    let path = accounts_path(store);
    Accounts::open(&path)
        .map_err(|refused| Failure::failed(format!("{}: {refused}", path.display())))
}

/// How user code is run, for the commands that run it.
#[derive(Args, Debug)]
#[command(next_help_heading = "User code options")]
pub struct UserCodeArgs {
    /// How long a user-code strategy or an extractor may run
    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = 600,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    task_timeout: u64,
    /// Where the task directories of user code are made
    ///
    /// The default is `spool-tasks` in the temporary directory.
    #[arg(long, value_name = "DIR")]
    tasks: Option<PathBuf>,
}

/// How user code of the project at `project` is run: the `uv` and `bun` that
/// `SPOOL_UV` and `SPOOL_BUN` name, and the options, if the command takes
/// them.
fn user_code(options: Option<&UserCodeArgs>, project: &Path) -> UserCode {
    let mut user_code = UserCode::new(project);
    if let Some(options) = options {
        user_code.timeout = Duration::from_secs(options.task_timeout);
        if let Some(dir) = &options.tasks {
            user_code.work_dir = dir.clone();
        }
    }
    let named = |variable: &str| std::env::var_os(variable).filter(|value| !value.is_empty());
    if let Some(uv) = named("SPOOL_UV") {
        user_code.uv = uv;
    }
    if let Some(bun) = named("SPOOL_BUN") {
        user_code.bun = bun;
    }
    user_code
}

/// Print the JSON Schema of the server's API
///
/// Prints the JSON Schema of every value the server's API takes and returns.
/// Client types are generated from it.
#[derive(Args)]
pub struct SchemaArgs {}

pub fn schema(_: SchemaArgs) -> Result<(), Failure> {
    println!(
        "{}",
        serde_json::to_string_pretty(&spool_cli::api::schema())
            .expect("a schema serialises to JSON")
    );
    Ok(())
}
