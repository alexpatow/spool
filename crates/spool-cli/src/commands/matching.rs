//! `spool match`: starts a match run on a running server and says what it
//! did.

use serde_json::{Value, json};
use spool::core::EntityType;
use spool::{RunEntry, RunStatus, StrategyRef, TypeInfo};

use super::remote::{self, Remote, WAIT_MS, print_json, unsuccessful};
use super::show::{count, fields, placed};
use crate::Failure;
use crate::args::Spec;

pub const USAGE: &str = "\
Usage: spool match [options]

Matches the records that no match has examined, on a running server, and
places them in identities: the rules judge pairs of records, and each new
record joins the identity of the records it matches or gets a new one.
Identities that exist keep their IDs and their members.

Each entity type the project declares is matched in a run of its own, one
after the other, and the command reports each. A run that fails does not
stop the ones after it, and the command then fails. `--type` matches one.

`spool show runs <run>` shows the whole report of a run, and
`spool show records` what each record belongs to now.

Options:
  --type <name>      only this entity type (default: every one the project
                     declares)
  --rematch          examine every record again, not only the new ones.
                     Records that have an identity keep it, and the run
                     reports where the rules disagree with the identities
  --strategy <name>  who judges: `rules`, or one of the project's strategies
                     for the type, which `--type` names when the project
                     declares more than one (default: rules)
  --url <url>        the server (default: $SPOOL_URL, or http://127.0.0.1:7765)
  --token <token>    a token of that server (default: $SPOOL_TOKEN); `spool dev`
                     asks for none
  --json             print the runs the server answered with, as JSON: with
                     `--type`, its answer, and without, `runs`, a list of
                     each type's `entity_type` and `run`
";

pub fn run(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &["--type", "--strategy", remote::VALUES[0], remote::VALUES[1]],
            flags: &["--rematch", "--json"],
            positional: 0,
        },
    )?;
    let remote = Remote::new(&arguments);
    let info = remote.info()?;
    let types = &info.store.entity_types;
    if types.is_empty() {
        return Err(Failure::Failed(
            "the project declares no entity type, so there is nothing to match. An entity \
             type is a directory entities/<name> with an entity.yaml."
                .to_owned(),
        ));
    }
    let named = arguments.value("--type");
    let chosen: Vec<&TypeInfo> = match named {
        Some(name) => vec![
            types
                .iter()
                .find(|declared| declared.name.as_str() == name)
                .ok_or_else(|| {
                    Failure::Failed(format!(
                        "the project has no entity type `{name}`. It has {}.",
                        declared(types)
                    ))
                })?,
        ],
        None => types.iter().collect(),
    };

    // The server takes a name it does not know for a strategy the caller
    // runs, and starts a run that waits for judgements. Nobody would send
    // them here.
    let strategy = arguments.value("--strategy");
    if let Some(strategy) = strategy
        && strategy != StrategyRef::RULES
    {
        let [entity_type] = chosen.as_slice() else {
            return Err(Failure::Failed(format!(
                "a strategy judges one entity type: say which `{strategy}` is for with \
                 `--type`. The project has {}.",
                declared(types)
            )));
        };
        if !entity_type
            .strategies
            .iter()
            .any(|known| known.name == strategy)
        {
            let known: Vec<String> = std::iter::once(StrategyRef::RULES)
                .chain(
                    entity_type
                        .strategies
                        .iter()
                        .map(|known| known.name.as_str()),
                )
                .map(|known| format!("`{known}`"))
                .collect();
            return Err(Failure::Failed(format!(
                "`{}` has no strategy `{strategy}`. It has {}.",
                entity_type.name,
                known.join(", ")
            )));
        }
    }

    let rematch = arguments.flag("--rematch");
    let as_json = arguments.flag("--json");
    let mut answered = Vec::new();
    let mut hints: Vec<&str> = Vec::new();
    let mut failures = Vec::new();
    for (index, entity_type) in chosen.iter().enumerate() {
        let name = &entity_type.name;
        let mut request = json!({ "entity_type": name, "wait_ms": WAIT_MS });
        if let Some(strategy) = strategy {
            request["strategy"] = json!({ "name": strategy });
        }
        if rematch {
            request["rematch"] = json!(true);
        }
        let started = remote.post("/match", &request)?;
        let ended = remote.ended(started)?;
        if as_json {
            let run = ended.as_ref().map_or(Value::Null, |(answer, _)| answer["run"].clone());
            match named {
                Some(_) => print_json(&json!({ "run": run })),
                None => answered.push(json!({ "entity_type": name, "run": run })),
            }
        } else if index > 0 {
            println!();
        }
        let Some((_, run)) = ended else {
            if !as_json {
                let (said, hint) = nothing(entity_type, rematch);
                println!("nothing to match: {said}");
                if !hints.contains(&hint) {
                    hints.push(hint);
                }
            }
            continue;
        };
        if run.status != RunStatus::Succeeded {
            let failure = unsuccessful(&run);
            if !as_json {
                println!("run         {}, type `{name}`: {}", run.run, run.status);
            }
            failures.push(failure);
            continue;
        }
        if !as_json {
            report(&run, name, rematch)?;
        }
    }
    if as_json && named.is_none() {
        print_json(&json!({ "runs": answered }));
    }
    for hint in hints.iter().filter(|hint| !hint.is_empty()) {
        println!("{hint}");
    }
    match failures.len() {
        0 => Ok(()),
        1 => Err(failures.remove(0)),
        _ => Err(Failure::Failed(
            failures
                .into_iter()
                .map(Failure::message)
                .collect::<Vec<_>>()
                .join("\n"),
        )),
    }
}

/// The entity types a project declares, in a sentence.
fn declared(types: &[TypeInfo]) -> String {
    types
        .iter()
        .map(|declared| format!("`{}`", declared.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why a type had nothing to match, and what would give it something.
fn nothing(entity_type: &TypeInfo, rematch: bool) -> (String, &'static str) {
    let name = &entity_type.name;
    if entity_type.records == 0 {
        (
            format!("there are no `{name}` records yet"),
            "`spool load <file>` sends rows",
        )
    } else if rematch {
        (
            format!("`{name}` has no records a match can examine"),
            "",
        )
    } else {
        (
            format!("no `{name}` record is new since the last match"),
            "`spool load <file>` sends more rows, and `spool match --rematch` examines them all \
             again",
        )
    }
}

/// Prints the report of a match run that succeeded.
fn report(run: &RunEntry, name: &EntityType, rematch: bool) -> Result<(), Failure> {
    let report = run.resolution.as_ref().ok_or_else(|| {
        Failure::Failed(format!(
            "run {} succeeded and reported no match; is the server of this version?",
            run.run
        ))
    })?;

    let mut first = format!("{}, type `{name}`", run.run);
    if let Some(strategy) = &run.strategy
        && strategy.name != StrategyRef::RULES
    {
        first.push_str(&format!(", strategy `{}`", strategy.name));
    }
    // A rematch reads the records that have an identity too, and the run
    // does not count them.
    let mut examined = count(report.new_records, "record");
    if rematch {
        examined.push_str(" without an identity");
    }
    if report.reexamined > 0 {
        examined.push_str(&format!(
            " ({} of them left for review before)",
            report.reexamined
        ));
    }
    if rematch {
        examined.push_str(" and every record placed before");
    }
    let mut judgements = format!(
        "{} match, {} no_match",
        report.judgements.match_, report.judgements.no_match
    );
    if report.judgements.unsure > 0 {
        judgements.push_str(&format!(", {} unsure", report.judgements.unsure));
    }
    let mut rows = vec![
        ("run", first),
        (
            "examined",
            format!("{examined}, {}", count(report.candidates, "candidate pair")),
        ),
        ("judgements", judgements),
        ("placed", placed(report)),
    ];
    if report.records_unassigned > 0 || !report.review.is_empty() {
        rows.push((
            "left out",
            format!(
                "{} in {}: `spool show review` says why",
                count(report.records_unassigned, "record"),
                count(report.review.len() as u64, "review item")
            ),
        ));
    }
    if let Some(draft) = report.draft {
        rows.push((
            "draft",
            format!("{draft} waits for a decision: nothing was placed yet"),
        ));
    }
    if let Some(suggestions) = &report.suggestions {
        if !(suggestions.merges.is_empty() && suggestions.splits.is_empty()) {
            rows.push((
                "suggested",
                format!(
                    "{}, {}: `spool show runs {}` lists them",
                    count(suggestions.merges.len() as u64, "merge"),
                    count(suggestions.splits.len() as u64, "split"),
                    run.run
                ),
            ));
        } else if rematch {
            rows.push((
                "suggested",
                "nothing: the rules agree with the identities".to_owned(),
            ));
        }
    }
    fields(&rows);
    Ok(())
}
