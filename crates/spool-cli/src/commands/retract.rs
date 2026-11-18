//! `spool retract`: tells a running server that a source no longer holds
//! some of its rows.

use std::io::Read;

use serde_json::json;
use spool::{RunEntry, RunStatus};

use super::remote::{self, Remote, print_json, typed};
use crate::Failure;
use crate::args::Spec;

pub const USAGE: &str = "\
Usage: spool retract --source <name> --type <name> [<key>...] [options]

Takes back rows a source no longer holds, on a running server. The rows are
named by their keys: the values of the column the source's file names as its
key.

Every version of each row stays in the store as evidence, and leaves its
identity by a changeset recorded under the caller. The identity keeps its ID
and its other members. No run examines the records after that, and they
cannot be given an identity, until the source sends the row again: ingesting
a row under the same key, changed or not, brings every version back.

A key the source has no row under changes nothing, so the same keys can be
sent twice.

  <key>              the key of a row to take back

Options:
  --source <name>    the source that held the rows
  --type <name>      their entity type
  --keys <file>      a file of keys, one per line; `-` reads standard input
  --reason <text>    why, for the changeset that detaches the records
  --url <url>        the server (default: $SPOOL_URL, or http://127.0.0.1:7765)
  --token <token>    a token of that server (default: $SPOOL_TOKEN); `spool dev`
                     asks for none
  --json             print the run the server answered with, as JSON
";

/// How long one request waits for the run before asking again.
const WAIT_MS: u64 = 30_000;

pub fn retract(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &[
                "--source",
                "--type",
                "--keys",
                "--reason",
                remote::VALUES[0],
                remote::VALUES[1],
            ],
            flags: &["--json"],
            positional: usize::MAX,
        },
    )?;
    let needed = |option: &str| {
        arguments
            .value(option)
            .map(str::to_owned)
            .ok_or_else(|| Failure::Usage(format!("`{option}` is needed")))
    };
    let source = needed("--source")?;
    let entity_type = needed("--type")?;

    let mut keys = arguments.positional.clone();
    if let Some(file) = arguments.value("--keys") {
        let text = if file == "-" {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| Failure::Failed(format!("standard input: {error}")))?;
            text
        } else {
            std::fs::read_to_string(file)
                .map_err(|error| Failure::Failed(format!("{file}: {error}")))?
        };
        keys.extend(
            text.lines()
                .map(|line| line.trim_end_matches('\r'))
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
        );
    }
    if keys.is_empty() {
        return Err(Failure::Usage(
            "name the keys of the rows to take back, or a file of them with `--keys`".to_owned(),
        ));
    }

    let remote = Remote::new(&arguments);
    let mut answer = remote.post(
        "/retract",
        &json!({
            "source": source,
            "entity_type": entity_type,
            "keys": keys,
            "reason": arguments.value("--reason"),
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
    if arguments.flag("--json") {
        print_json(&answer);
    }
    if run.status != RunStatus::Succeeded {
        return Err(Failure::Failed(format!(
            "run {} {}: {}",
            run.run,
            run.status,
            run.error.as_deref().unwrap_or("the server did not say why")
        )));
    }
    if arguments.flag("--json") {
        return Ok(());
    }
    let report = run.retraction.ok_or_else(|| {
        Failure::Failed(format!(
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
    if report.unknown > 0 {
        println!(
            "{}: `{source}` has no `{entity_type}` row under {}",
            plural(report.unknown, "key was unknown", "keys were unknown"),
            if report.unknown == 1 { "it" } else { "them" }
        );
    }
    Ok(())
}
