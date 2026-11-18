//! `spool match`: starts a match run on a running server and says what it
//! did.

use serde_json::json;
use spool::{RunStatus, StrategyRef};

use super::remote::{self, Remote, WAIT_MS, print_json, unsuccessful};
use super::show::{count, fields};
use crate::Failure;
use crate::args::Spec;

pub const USAGE: &str = "\
Usage: spool match [options]

Matches the records of one entity type that no match has examined, on a
running server, and places them in identities: the rules judge pairs of
records, and each new record joins the identity of the records it matches or
gets a new one. Identities that exist keep their IDs and their members.

`spool show runs <run>` shows the whole report of the run, and
`spool show records` what each record belongs to now.

Options:
  --type <name>      the entity type (default: the only one the project
                     declares)
  --rematch          examine every record of the type again, not only the new
                     ones. Records that have an identity keep it, and the run
                     reports where the rules disagree with the identities
  --strategy <name>  who judges: `rules`, or one of the project's strategies
                     for the type (default: rules)
  --url <url>        the server (default: $SPOOL_URL, or http://127.0.0.1:7765)
  --token <token>    a token of that server (default: $SPOOL_TOKEN); `spool dev`
                     asks for none
  --json             print the run the server answered with, as JSON
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
    let declared = || {
        types
            .iter()
            .map(|declared| format!("`{}`", declared.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let entity_type = match (arguments.value("--type"), types.as_slice()) {
        (_, []) => {
            return Err(Failure::Failed(
                "the project declares no entity type, so there is nothing to match. An entity \
                 type is a directory entities/<name> with an entity.yaml."
                    .to_owned(),
            ));
        }
        (Some(name), _) => types
            .iter()
            .find(|declared| declared.name.as_str() == name)
            .ok_or_else(|| {
                Failure::Failed(format!(
                    "the project has no entity type `{name}`. It has {}.",
                    declared()
                ))
            })?,
        (None, [only]) => only,
        (None, _) => {
            return Err(Failure::Failed(format!(
                "say which entity type to match with `--type`: the project has {}",
                declared()
            )));
        }
    };
    let name = &entity_type.name;

    // The server takes a name it does not know for a strategy the caller
    // runs, and starts a run that waits for judgements. Nobody would send
    // them here.
    let strategy = arguments.value("--strategy");
    if let Some(strategy) = strategy
        && strategy != StrategyRef::RULES
        && !entity_type
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
            "`{name}` has no strategy `{strategy}`. It has {}.",
            known.join(", ")
        )));
    }

    let rematch = arguments.flag("--rematch");
    let mut request = json!({ "entity_type": name, "wait_ms": WAIT_MS });
    if let Some(strategy) = strategy {
        request["strategy"] = json!({ "name": strategy });
    }
    if rematch {
        request["rematch"] = json!(true);
    }
    let started = remote.post("/match", &request)?;
    let Some((answer, run)) = remote.ended(started)? else {
        if arguments.flag("--json") {
            print_json(&json!({ "run": null }));
        } else if entity_type.records == 0 {
            println!(
                "nothing to match: there are no `{name}` records yet\n\
                 `spool load <file>` sends rows"
            );
        } else if rematch {
            println!("nothing to match: `{name}` has no records a match can examine");
        } else {
            println!(
                "nothing to match: no `{name}` record is new since the last match\n\
                 `spool load <file>` sends more rows, and `spool match --rematch` examines \
                 them all again"
            );
        }
        return Ok(());
    };
    if arguments.flag("--json") {
        print_json(&answer);
    }
    if run.status != RunStatus::Succeeded {
        return Err(unsuccessful(&run));
    }
    if arguments.flag("--json") {
        return Ok(());
    }
    let report = run.resolution.ok_or_else(|| {
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
        (
            "placed",
            format!(
                "{}, {} created",
                count(report.records_assigned, "record"),
                count(report.identities_created, "identity")
            ),
        ),
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
