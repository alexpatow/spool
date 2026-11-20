//! `spool match`: starts a match run on a running server and says what it
//! did.

use anstream::println;
use clap::Args;
use serde_json::{Value, json};
use spool::core::EntityType;
use spool::{RunEntry, RunStatus, StrategyRef, TypeInfo};

use super::remote::{Remote, RemoteArgs, WAIT_MS, unsuccessful};
use super::show::runs::placed;
use crate::Failure;
use crate::output::{Output, OutputArgs, fields, hint};
use crate::progress;
use spool_cli::text::count;

/// Match new records and place them in identities
///
/// Matches the records no match has examined yet, on a running server. The
/// rules judge pairs, and each new record joins the identity of the records
/// it matches or starts a new one. Existing identities keep their IDs and
/// their members.
///
/// Each entity type the project declares is matched in its own run, one
/// after another, with a report for each. A failed run doesn't stop the
/// rest, but the command then fails. `--type` matches one type.
///
/// `spool show runs <run>` shows a run's full report, and `spool show
/// records` shows where each record is now.
///
/// With `--type`, the JSON output is the run the server returned. Without
/// it, the output is `runs`, with each type's `entity_type` and `run`.
#[derive(Args)]
pub struct MatchArgs {
    /// Only this entity type [default: every one the project declares]
    #[arg(long = "type", value_name = "NAME")]
    entity_type: Option<String>,
    /// Examine every record again, not only the new ones
    ///
    /// Records that have an identity keep it, and the run reports where the
    /// rules disagree with the identities.
    #[arg(long)]
    rematch: bool,
    /// Who judges: `rules`, or one of the project's strategies for the type
    ///
    /// A strategy is for one entity type, which `--type` names when the
    /// project declares more than one.
    #[arg(long, value_name = "NAME", default_value = StrategyRef::RULES)]
    strategy: String,
    #[command(flatten)]
    output: OutputArgs,
    #[command(flatten)]
    remote: RemoteArgs,
}

#[expect(clippy::too_many_lines)]
pub fn run(arguments: MatchArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool match`")?;
    let remote = Remote::new(&arguments.remote);
    let info = remote.info()?;
    let types = &info.store.entity_types;
    if types.is_empty() {
        return Err(Failure::failed(
            "the project declares no entity type, so there is nothing to match. An entity \
             type is a directory entities/<name> with an entity.yaml."
                .to_owned(),
        ));
    }
    let named = arguments.entity_type.as_deref();
    let chosen: Vec<&TypeInfo> = match named {
        Some(name) => vec![
            types
                .iter()
                .find(|declared| declared.name.as_str() == name)
                .ok_or_else(|| {
                    Failure::failed(format!(
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
    let strategy = Some(arguments.strategy.as_str()).filter(|name| *name != StrategyRef::RULES);
    if let Some(strategy) = strategy {
        let [entity_type] = chosen.as_slice() else {
            return Err(Failure::failed(format!(
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
            return Err(Failure::failed(format!(
                "`{}` has no strategy `{strategy}`. It has {}.",
                entity_type.name,
                known.join(", ")
            )));
        }
    }

    let rematch = arguments.rematch;
    let as_json = !out.is_table();
    let mut answered = Vec::new();
    let mut hints: Vec<&str> = Vec::new();
    let mut failures = Vec::new();
    // A report is set apart from what is printed before and after it.
    let mut printed: Option<bool> = None;
    let mut part = |report: bool| {
        if printed.is_some_and(|before| before || report) {
            println!();
        }
        printed = Some(report);
    };
    for entity_type in &chosen {
        let name = &entity_type.name;
        let mut request = json!({ "entity_type": name, "wait_ms": WAIT_MS });
        if let Some(strategy) = strategy {
            request["strategy"] = json!({ "name": strategy });
        }
        if rematch {
            request["rematch"] = json!(true);
        }
        // A run that fails within the wait is refused as the answer, and
        // one type's failure does not keep the others from being matched.
        let waiting = progress::spinner(format!("Matching `{name}`"));
        let ended = remote
            .post("/match", &request)
            .and_then(|started| remote.ended(started));
        waiting.finish_and_clear();
        let ended = match ended {
            Ok(ended) => ended,
            Err(failure) if named.is_none() => {
                let message = failure.message();
                if as_json {
                    answered.push(json!({ "entity_type": name, "run": null, "error": message }));
                } else {
                    part(false);
                    println!("failed      type `{name}`: the match did not run to the end");
                }
                failures.push(Failure::failed(format!("type `{name}`: {message}")));
                continue;
            }
            Err(failure) => return Err(failure),
        };
        if as_json {
            let run = ended
                .as_ref()
                .map_or(Value::Null, |(answer, _)| answer["run"].clone());
            match named {
                Some(_) => out.json(&json!({ "run": run })),
                None => answered.push(json!({ "entity_type": name, "run": run })),
            }
        }
        let Some((_, run)) = ended else {
            if !as_json {
                part(false);
                let (said, hint) = nothing(entity_type, rematch);
                println!("nothing to match: {said}");
                if !hints.contains(&hint) {
                    hints.push(hint);
                }
            }
            continue;
        };
        if run.status != RunStatus::Succeeded {
            let failure = match named {
                Some(_) => unsuccessful(&run),
                None => Failure::failed(format!("type `{name}`: {}", unsuccessful(&run).message())),
            };
            if !as_json {
                part(false);
                println!("run         {}, type `{name}`: {}", run.run, run.status);
            }
            failures.push(failure);
            continue;
        }
        if !as_json {
            part(true);
            report(&run, name, rematch)?;
        }
    }
    if as_json && named.is_none() {
        out.json(&json!({ "runs": answered }));
    }
    for said in hints.iter().filter(|said| !said.is_empty()) {
        hint(said);
    }
    match failures.len() {
        0 => Ok(()),
        1 => Err(failures.remove(0)),
        _ => Err(Failure::failed(
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
        (format!("`{name}` has no records a match can examine"), "")
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
        Failure::failed(format!(
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
