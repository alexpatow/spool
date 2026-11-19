//! `spool retract`: tells a running server that a source no longer holds
//! some of its rows.

use std::io::Read;

use anstream::println;
use clap::Args;
use serde_json::json;
use spool::{RunEntry, RunStatus};

use super::remote::{Remote, RemoteArgs, typed};
use crate::Failure;
use crate::output::{Output, OutputArgs};
use crate::progress;

/// Retract rows a source no longer holds
///
/// Retracts rows from a source, on a running server. Rows are named by their
/// key, the column the source's file declares as `key`.
///
/// Every version of each row stays in the store, and leaves its identity in
/// a changeset recorded under the caller. The identity keeps its ID and its
/// other members. No run looks at the records again, and they can't be
/// placed, until the source sends the row again: loading the same key,
/// changed or not, restores every version.
///
/// Keys the source doesn't hold change nothing, so retracting is idempotent.
#[derive(Args)]
#[command(
    after_help = "Examples:\n  spool retract --source crm --type company c-6 --reason \"deleted in the CRM\"\n  spool retract --source crm --type company --keys gone.txt"
)]
pub struct RetractArgs {
    /// The keys of the rows to take back
    #[arg(required_unless_present = "keys_file")]
    keys: Vec<String>,
    /// The source that held the rows
    #[arg(long, value_name = "NAME")]
    source: String,
    /// Their entity type
    #[arg(long = "type", value_name = "NAME")]
    entity_type: String,
    /// A file of keys, one per line; `-` reads standard input
    #[arg(long = "keys", value_name = "FILE")]
    keys_file: Option<String>,
    /// Why, for the changeset that detaches the records
    #[arg(long, value_name = "TEXT")]
    reason: Option<String>,
    #[command(flatten)]
    output: OutputArgs,
    #[command(flatten)]
    remote: RemoteArgs,
}

/// How long one request waits for the run before asking again.
const WAIT_MS: u64 = 30_000;

pub fn retract(arguments: RetractArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool retract`")?;
    let source = arguments.source.clone();
    let entity_type = arguments.entity_type.clone();

    let mut keys = arguments.keys.clone();
    if let Some(file) = &arguments.keys_file {
        let text = if file == "-" {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| Failure::failed(format!("standard input: {error}")))?;
            text
        } else {
            std::fs::read_to_string(file)
                .map_err(|error| Failure::failed(format!("{file}: {error}")))?
        };
        keys.extend(
            text.lines()
                .map(|line| line.trim_end_matches('\r'))
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        );
    }
    if keys.is_empty() {
        return Err(Failure::usage(
            "name the keys of the rows to take back, or a file of them with `--keys`".to_owned(),
        ));
    }

    let remote = Remote::new(&arguments.remote);
    let waiting = progress::spinner(format!(
        "Taking back {}",
        super::show::count(keys.len() as u64, "row")
    ));
    let mut answer = remote.post(
        "/retract",
        &json!({
            "source": source,
            "entity_type": entity_type,
            "keys": keys,
            "reason": arguments.reason,
            "wait_ms": WAIT_MS,
        }),
    )?;
    let mut run: RunEntry = typed(&answer["run"])?;
    while run.status == RunStatus::Running {
        answer = remote.get(
            &format!("/runs/{}", run.run),
            &[("wait_ms", WAIT_MS.to_string())],
            || format!("the server no longer has run {}", run.run),
        )?;
        run = typed(&answer["run"])?;
    }
    waiting.finish_and_clear();
    if !out.is_table() {
        out.json(&answer);
    }
    if run.status != RunStatus::Succeeded {
        return Err(Failure::failed(format!(
            "run {} {}: {}",
            run.run,
            run.status,
            run.error.as_deref().unwrap_or("the server did not say why")
        )));
    }
    if !out.is_table() {
        return Ok(());
    }
    let report = run.retraction.ok_or_else(|| {
        Failure::failed(format!(
            "run {} succeeded and reported no retraction; is the server of this version?",
            run.run
        ))
    })?;
    let plural = |number: u64, one: &str, many: &str| {
        format!("{number} {}", if number == 1 { one } else { many })
    };
    println!(
        "run {}: {} named, {} taken back",
        report.run,
        plural(report.received, "key", "keys"),
        plural(report.retracted, "record", "records"),
    );
    match &report.committed {
        Some(committed) => println!(
            "{} left an identity, by the changeset at log position {}",
            plural(report.detached, "record", "records"),
            committed.seq
        ),
        None => println!("none of them had an identity"),
    }
    if report.possible_splits > 0 {
        println!(
            "{} left for review: what held {} records together went through a record that was taken back. See `spool show review --status open`.",
            plural(report.possible_splits, "identity was", "identities were"),
            if report.possible_splits == 1 {
                "its"
            } else {
                "their"
            }
        );
    }
    if report.unknown > 0 {
        println!(
            "{}: `{source}` has no `{entity_type}` row under {}",
            plural(report.unknown, "key was unknown", "keys were unknown"),
            if report.unknown == 1 { "it" } else { "them" }
        );
    }
    Ok(())
}
