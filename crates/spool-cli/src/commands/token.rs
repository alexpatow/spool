//! `spool token`: the tokens of callers that are not people, made and taken
//! away on the server's own machine.
//!
//! These open the accounts file, not the store, so they work while the
//! server runs: it looks a token up each time one is presented.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use spool_cli::accounts::Accounts;
use spool_cli::api::TokenList;

use super::show::table;
use crate::Failure;

/// Make, list and revoke the tokens of callers that are not people
///
/// Makes, lists and revokes the tokens of a server, on the server's machine.
/// A token is what a caller that is not a person presents: an agent or a
/// pipeline. What it changes is recorded under the token's name, as
/// `agent:<name>`. An admin does the same on the studio's Tokens page.
///
/// These work whether or not the server is running.
#[derive(Args)]
pub struct TokenArgs {
    #[command(subcommand)]
    pub action: Action,
    /// The store's database file; the tokens are in the accounts file beside it
    #[arg(
        long,
        env = "SPOOL_STORE",
        hide_env_values = true,
        value_name = "PATH",
        global = true
    )]
    store: Option<PathBuf>,
    /// Print the tokens as JSON
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
pub enum Action {
    /// Make a token, and print it
    ///
    /// Makes a token and prints it, and nothing else, on standard output. It
    /// is shown this once: the server keeps a hash of it. A caller sends it
    /// as `Authorization: Bearer <token>`.
    Add {
        /// The token's name: 1 to 64 letters, digits, `.`, `_` or `-`
        name: String,
    },
    /// List the tokens
    List,
    /// Revoke a token: the server no longer accepts it
    Revoke {
        /// The token's name
        name: String,
    },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Add { .. } => "add",
            Action::List => "list",
            Action::Revoke { .. } => "revoke",
        }
    }
}

pub fn token(arguments: TokenArgs) -> Result<(), Failure> {
    match &arguments.action {
        Action::Add { name } => add(&arguments, name),
        Action::List => list(&arguments),
        Action::Revoke { name } => revoke(&arguments, name),
    }
}

/// The accounts beside the store the arguments name. The store has to be
/// there: a token made beside a path nobody serves would open nothing.
fn accounts(arguments: &TokenArgs) -> Result<(Accounts, PathBuf), Failure> {
    let store = super::store_path(&arguments.store)?;
    if !store.is_file() {
        return Err(Failure::Failed(format!(
            "there is no store at {}: `spool serve` or `spool apply` makes one",
            store.display()
        )));
    }
    Ok((super::open_accounts(&store)?, super::accounts_path(&store)))
}

fn refused(refused: spool_cli::accounts::Refused) -> Failure {
    Failure::Failed(refused.to_string())
}

fn add(arguments: &TokenArgs, name: &str) -> Result<(), Failure> {
    let (accounts, path) = accounts(arguments)?;
    let (token, secret) = accounts.add_token(name, None).map_err(refused)?;
    println!("{secret}");
    eprintln!(
        "That is the token `{name}`, kept in {path}. It is shown this once.\n\
         A caller sends it as `Authorization: Bearer <token>`, and what it changes is recorded \
         under `agent:{name}`.",
        name = token.name,
        path = path.display(),
    );
    Ok(())
}

fn list(arguments: &TokenArgs) -> Result<(), Failure> {
    let (accounts, _) = accounts(arguments)?;
    let tokens = accounts.tokens().map_err(refused)?;
    if arguments.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&TokenList { tokens }).expect("tokens serialise to JSON")
        );
        return Ok(());
    }
    let people = accounts.list().map_err(refused)?;
    let made_by = |id: &Option<String>| match id {
        None => "spool token add".to_owned(),
        Some(id) => people
            .iter()
            .find(|person| &person.id == id)
            .map_or_else(|| id.clone(), |person| person.username.clone()),
    };
    table(
        "tokens",
        &["token", "made at", "made by", "last used"],
        tokens
            .iter()
            .map(|token| {
                vec![
                    token.name.clone(),
                    token.created_at.clone(),
                    made_by(&token.created_by),
                    token
                        .last_used_at
                        .clone()
                        .unwrap_or_else(|| "never".to_owned()),
                ]
            })
            .collect(),
    );
    Ok(())
}

fn revoke(arguments: &TokenArgs, name: &str) -> Result<(), Failure> {
    let (accounts, _) = accounts(arguments)?;
    let token = accounts.revoke_token_named(name).map_err(refused)?;
    println!("revoked `{}`: the server no longer accepts it", token.name);
    Ok(())
}
