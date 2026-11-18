//! `spool token`: the tokens of callers that are not people, made and taken
//! away on the server's own machine.
//!
//! These open the accounts file, not the store, so they work while the
//! server runs: it looks a token up each time one is presented.

use std::path::PathBuf;

use spool_cli::accounts::Accounts;
use spool_cli::api::TokenList;

use super::show::table;
use crate::Failure;
use crate::args::{Arguments, Spec};

pub const USAGE: &str = "\
Usage: spool token add <name> [options]
       spool token list [options]
       spool token revoke <name> [options]

Makes, lists and revokes the tokens of a server, on the server's machine. A
token is what a caller that is not a person presents: an agent or a pipeline.
What it changes is recorded under the token's name, as `agent:<name>`. An
admin does the same on the studio's Tokens page.

`add` prints the token, and nothing else, on standard output. It is shown
this once: the server keeps a hash of it. A caller sends it as
`Authorization: Bearer <token>`.

These work whether or not the server is running.

  <name>             1 to 64 letters, digits, `.`, `_` or `-`

Options:
  --store <path>     the store's database file (default: $SPOOL_STORE). The
                     tokens are in the accounts file beside it.
  --json             print the tokens as JSON (list)
";

pub fn token(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &["--store"],
            flags: &["--json"],
            positional: 2,
        },
    )?;
    let words: Vec<&str> = arguments.positional.iter().map(String::as_str).collect();
    match words[..] {
        ["add", name] => add(&arguments, name),
        ["list"] => list(&arguments),
        ["revoke", name] => revoke(&arguments, name),
        ["add"] | ["revoke"] => Err(Failure::Usage(format!(
            "`spool token {}` needs the token's name",
            words[0]
        ))),
        [] => Err(Failure::Usage(
            "say what to do: `add`, `list` or `revoke`".to_owned(),
        )),
        [other, ..] if !["add", "list", "revoke"].contains(&other) => Err(Failure::Usage(format!(
            "`spool token` does `add`, `list` and `revoke`, not `{other}`"
        ))),
        _ => Err(Failure::Usage(format!(
            "unexpected argument `{}`",
            words[words.len() - 1]
        ))),
    }
}

/// The accounts beside the store the arguments name. The store has to be
/// there: a token made beside a path nobody serves would open nothing.
fn accounts(arguments: &Arguments) -> Result<(Accounts, PathBuf), Failure> {
    let store = super::store_path(arguments)?;
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

fn add(arguments: &Arguments, name: &str) -> Result<(), Failure> {
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

fn list(arguments: &Arguments) -> Result<(), Failure> {
    let (accounts, _) = accounts(arguments)?;
    let tokens = accounts.tokens().map_err(refused)?;
    if arguments.flag("--json") {
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

fn revoke(arguments: &Arguments, name: &str) -> Result<(), Failure> {
    let (accounts, _) = accounts(arguments)?;
    let token = accounts.revoke_token_named(name).map_err(refused)?;
    println!("revoked `{}`: the server no longer accepts it", token.name);
    Ok(())
}
