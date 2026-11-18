//! `spool bench`: runs one benchmark scenario in this process.

use std::path::{Path, PathBuf};

use spool::arrow::array::{Array, AsArray};
use spool::core::ReviewItem;
use spool::{BenchReport, Engine, Options, Scenario, StepReport, StrategyRef, UserCode};

use crate::Failure;
use crate::args::Spec;

pub const USAGE: &str = "\
Usage: spool bench <scenario> [options]

Runs a benchmark scenario: ingests each step's files, stores its documents and
runs its extractors over them, applies its corrections, resolves, and scores
committed state against the scenario's truth. A scenario with labelled
mentions is also scored on what its extractors found.

  <scenario>      a scenario.toml, or the directory that holds one

Options:
  --db <path>         the database file to build; replaced if it exists
                      (default: .spool/bench.duckdb in the project directory)
  --seed <n>          the seed for identity and changeset IDs (default: 0)
  --out <path>        write the results as JSON to this file
  --list <n>          print at most this many identities per step (default: 50)
  --strategy <name>   the strategy of every step that does not name its own:
                      `rules`, or one of the project's user-code strategies
                      (default: the scenario's, or `rules`)
  --task-timeout <s>  how long a user-code strategy or an extractor may run,
                      in seconds (default: 600)
  --tasks <dir>       where the task directories of user code are made
                      (default: spool-tasks in the temporary directory)

Strategies and extractors that are `.py` files run with `uv`, and `.ts` files
with `bun`.
SPOOL_UV and SPOOL_BUN name the commands if they are not on the search path.
";

const SPEC: Spec = Spec {
    values: &[
        "--db",
        "--seed",
        "--out",
        "--list",
        "--strategy",
        "--task-timeout",
        "--tasks",
    ],
    flags: &[],
    positional: 1,
};

struct Arguments {
    scenario: PathBuf,
    db: Option<PathBuf>,
    seed: u64,
    out: Option<PathBuf>,
    list: usize,
    strategy: Option<String>,
}

pub fn bench(arguments: &[String]) -> Result<(), Failure> {
    let parsed = super::arguments(arguments, &SPEC)?;
    let usage = Failure::Usage;
    let arguments = Arguments {
        scenario: parsed
            .positional
            .first()
            .map(PathBuf::from)
            .ok_or_else(|| usage("`bench` needs a scenario".to_owned()))?,
        db: parsed.path("--db"),
        seed: parsed.number("--seed").map_err(usage)?.unwrap_or(0),
        out: parsed.path("--out"),
        list: parsed.number("--list").map_err(usage)?.unwrap_or(50) as usize,
        strategy: parsed.value("--strategy").map(str::to_owned),
    };
    let mut scenario = Scenario::load(&arguments.scenario)?;
    if let Some(strategy) = &arguments.strategy {
        scenario = scenario.with_strategy(strategy);
    }
    let user_code = super::user_code(&parsed, &scenario.project_dir())?;
    Ok(run(&arguments, scenario, user_code)?)
}

fn run(arguments: &Arguments, scenario: Scenario, user_code: UserCode) -> spool::Result<()> {
    let db = match &arguments.db {
        Some(db) => db.clone(),
        None => scenario.project_dir().join(".spool").join("bench.duckdb"),
    };
    start_afresh(&db)?;

    let engine = Engine::open(&db, Options::seeded(arguments.seed))?;
    engine.set_user_code(user_code);
    println!("scenario  {}", scenario.name);
    println!("database  {}", db.display());
    let report = scenario.run(&engine, |step| print_step(&engine, step, arguments.list))?;

    if let Some(order) = &report.order_sensitivity {
        println!("\n== the batches in reverse order, without corrections ==");
        println!(
            "  same group    {} of {} records ({})",
            order.same_group,
            order.records,
            share(order.agreement)
        );
        println!(
            "  same pairs    {} of {} ({})",
            order.pairs_in_both,
            order.pairs_in_either,
            share(order.pair_agreement)
        );
    }

    if let Some(out) = &arguments.out {
        write_results(out, &report)?;
        println!("\nresults   {}", out.display());
    }
    Ok(())
}

/// Removes the database file a previous run left, with its write-ahead log.
fn start_afresh(db: &Path) -> spool::Result<()> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| spool::Error::Io { path, source }
    };
    if let Some(parent) = db.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let mut wal = db.as_os_str().to_owned();
    wal.push(".wal");
    for path in [db.to_owned(), PathBuf::from(wal)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(&path)(error)),
        }
    }
    Ok(())
}

fn write_results(out: &Path, report: &BenchReport) -> spool::Result<()> {
    let mut json = serde_json::to_string_pretty(report).expect("a report serialises to JSON");
    json.push('\n');
    std::fs::write(out, json).map_err(|source| spool::Error::Io {
        path: out.to_owned(),
        source,
    })
}

fn print_step(engine: &Engine, step: &StepReport, list: usize) -> spool::Result<()> {
    println!("\n== {} ==", step.name);
    for ingest in &step.ingest {
        println!(
            "ingested  {} rows of `{}` from `{}`: {} new, {} already known",
            ingest.received, ingest.entity_type, ingest.source, ingest.inserted, ingest.duplicates
        );
    }
    for documents in &step.documents {
        println!(
            "stored    {} documents from `{}`: {} new, {} already known",
            documents.received, documents.source, documents.inserted, documents.duplicates
        );
    }
    for extract in &step.extract {
        println!(
            "extracted `{}` read {} documents and found {} mentions: {} new, {} already known",
            extract.extractor,
            extract.documents,
            extract.mentions,
            extract.inserted,
            extract.duplicates
        );
    }
    for correction in &step.corrections {
        let name = match &correction.name {
            Some(name) => format!(" `{name}`"),
            None => String::new(),
        };
        println!(
            "corrected {}{name}: changeset at {}",
            correction.kind, correction.seq
        );
    }
    if step.resolve.resolved.is_empty() {
        println!("resolved  nothing new");
    }
    for resolved in &step.resolve.resolved {
        let again = match resolved.reexamined {
            0 => String::new(),
            count => format!(" ({count} left for review earlier)"),
        };
        let with = match step.strategy.as_str() {
            StrategyRef::RULES => String::new(),
            strategy => format!(" with `{strategy}`"),
        };
        // A rematch examines every record, whether it is new or not.
        let examined = match &resolved.suggestions {
            Some(_) => "every record again".to_owned(),
            None => format!("{} records", resolved.new_records),
        };
        println!(
            "resolved  `{}`{with}: {examined}{again}, {} candidate pairs, {} match and {} no-match judgements",
            resolved.entity_type,
            resolved.candidates,
            resolved.judgements.match_,
            resolved.judgements.no_match,
        );
        match &resolved.committed {
            Some(committed) => println!(
                "committed changeset {} at {}: {} identities created, {} records assigned, {} left unassigned",
                committed.changeset,
                committed.seq,
                resolved.identities_created,
                resolved.records_assigned,
                resolved.records_unassigned,
            ),
            None => println!(
                "committed nothing: {} records left unassigned",
                resolved.records_unassigned
            ),
        }
        if let Some(suggestions) = &resolved.suggestions {
            println!(
                "suggested {} merges and {} splits of existing identities; none was applied",
                suggestions.merges.len(),
                suggestions.splits.len()
            );
            let lines = suggestions
                .merges
                .iter()
                .map(|merge| {
                    format!(
                        "  merge {} and {}: {} pairs matched, {} did not",
                        merge.identities[0], merge.identities[1], merge.matches, merge.no_matches
                    )
                })
                .chain(suggestions.splits.iter().map(|split| {
                    format!(
                        "  split {}: {} pairs did not match, {} did",
                        split.identity, split.no_matches, split.matches
                    )
                }));
            let count = suggestions.merges.len() + suggestions.splits.len();
            for line in lines.take(list) {
                println!("{line}");
            }
            if count > list {
                println!(
                    "  ... and {} more; raise --list to print them",
                    count - list
                );
            }
        }
    }

    print_identities(engine, list)?;

    let review: Vec<&ReviewItem> = step
        .resolve
        .resolved
        .iter()
        .flat_map(|resolved| &resolved.review)
        .collect();
    if !review.is_empty() {
        println!("\nfor review ({}):", review.len());
        let more = review.len().saturating_sub(list);
        let review = &review[..review.len() - more];
        let keys = if review.is_empty() {
            Default::default()
        } else {
            record_keys(engine)?
        };
        let name = |id: &spool::core::RecordId| {
            keys.get(&id.to_string())
                .cloned()
                .unwrap_or_else(|| id.to_string())
        };
        let names =
            |ids: &[spool::core::RecordId]| ids.iter().map(&name).collect::<Vec<_>>().join(", ");
        for item in review {
            match item {
                ReviewItem::PossibleMerge {
                    records,
                    identities,
                } => println!(
                    "  possible merge: {} matched identities {}",
                    names(records),
                    identities
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" and "),
                ),
                ReviewItem::Conflict { records, identity } => match identity {
                    Some(identity) => println!(
                        "  conflict: {} both matched and did not match identity {identity}",
                        names(records)
                    ),
                    None => println!(
                        "  conflict: {} matched each other, with a no-match among them",
                        names(records)
                    ),
                },
            }
        }
        if more > 0 {
            println!("  ... and {more} more; raise --list to print them");
        }
    }

    if let Some(extraction) = &step.extraction {
        println!("\nextraction ({} labelled mentions):", extraction.labelled);
        println!(
            "  mentions      {} extracted, {} of them labelled",
            extraction.extracted, extraction.correct
        );
        println!(
            "  spans         precision {}  recall {}  (F1 {})",
            share(extraction.precision),
            share(extraction.recall),
            share(extraction.f1)
        );
    }

    let metrics = &step.metrics;
    println!("\nagainst the truth ({} records):", metrics.records);
    println!(
        "  identities    {} for {} true entities",
        metrics.identities, metrics.true_entities
    );
    println!(
        "  pairs         precision {}  recall {}  (F1 {})",
        share(metrics.pairs.precision),
        share(metrics.pairs.recall),
        share(metrics.pairs.f1)
    );
    println!(
        "  B-cubed       precision {}  recall {}",
        share(metrics.b_cubed.precision),
        share(metrics.b_cubed.recall)
    );
    println!("  false merges  {}", metrics.false_merges);
    println!("  false splits  {}", metrics.false_splits);
    println!(
        "  unassigned    {} ({})",
        metrics.unassigned,
        share(metrics.unassigned_share)
    );
    let stability = &step.id_stability;
    println!(
        "  ID stability  {} of {} records kept their identity ({})",
        stability.unchanged,
        stability.records,
        share(stability.strict)
    );
    if stability.unchanged_following_redirects != stability.unchanged {
        println!(
            "                {} once merged IDs are followed ({})",
            stability.unchanged_following_redirects,
            share(stability.following_redirects)
        );
    }
    let churn = &step.churn;
    println!(
        "  churn         {} identities created, {} merged, {} records reassigned, for {} new records",
        churn.identities_created,
        churn.identities_merged,
        churn.records_reassigned,
        churn.new_records
    );
    let splits = &step.split_recovery;
    if splits.splits > 0 {
        println!(
            "  splits        {} of {} still apart ({})",
            splits.kept_apart,
            splits.splits,
            share(splits.share)
        );
    }
    let overrides = &step.override_persistence;
    if overrides.constraints > 0 {
        println!(
            "  constraints   {} of {} still honoured ({})",
            overrides.honoured,
            overrides.constraints,
            share(overrides.share)
        );
    }
    if let Some(upgrade) = &step.upgrade_stability {
        println!(
            "  upgrade       `{}` matched everything again: {} of {} records kept their identity ({}); \
             {} merges and {} splits suggested, {} changes applied",
            upgrade.strategy,
            upgrade.retained,
            upgrade.records,
            share(upgrade.share),
            upgrade.suggested_merges,
            upgrade.suggested_splits,
            upgrade.applied_changes
        );
    }
    Ok(())
}

fn share(value: Option<f64>) -> String {
    match value {
        Some(value) => format!("{value:.4}"),
        None => "n/a".to_owned(),
    }
}

fn text(batch: &spool::arrow::record_batch::RecordBatch, column: usize, row: usize) -> String {
    let values = batch.column(column).as_string::<i32>();
    if values.is_null(row) {
        String::new()
    } else {
        values.value(row).to_owned()
    }
}

/// What to call the record `o` in messages a person reads: `source:key`,
/// and for a mention `source:document[start-end]`, since its own key names
/// the document by ID.
const RECORD_NAME: &str = "o.source_id || ':' || CASE
        WHEN json_extract_string(o.evidence, '$.document') IS NULL THEN coalesce(o.source_key, '?')
        ELSE json_extract_string(o.evidence, '$.key')
             || '[' || json_extract_string(o.evidence, '$.start')
             || '-' || json_extract_string(o.evidence, '$.end') || ']'
    END";

/// Record ID to its name, for messages a person reads.
fn record_keys(engine: &Engine) -> spool::Result<std::collections::HashMap<String, String>> {
    let batches = engine.query(&format!(
        "SELECT o.record_id::VARCHAR, {RECORD_NAME} FROM spool.record o"
    ))?;
    let mut keys = std::collections::HashMap::new();
    for batch in &batches {
        for row in 0..batch.num_rows() {
            keys.insert(text(batch, 0, row), text(batch, 1, row));
        }
    }
    Ok(keys)
}

fn print_identities(engine: &Engine, list: usize) -> spool::Result<()> {
    let count = engine.query("SELECT count(*)::VARCHAR FROM spool.identity_current")?;
    let total: usize = count
        .first()
        .and_then(|batch| text(batch, 0, 0).parse().ok())
        .unwrap_or(0);
    println!("\nidentities ({total}):");

    let batches = engine.query(&format!(
        "SELECT m.identity_id::VARCHAR, i.entity_type, count(*)::VARCHAR,
                string_agg({RECORD_NAME}, ', ' ORDER BY o.source_id, {RECORD_NAME})
         FROM spool.membership_current m
         JOIN spool.identity_current i ON i.identity_id = m.identity_id
         JOIN spool.record o ON o.record_id = m.record_id
         GROUP BY m.identity_id, i.entity_type
         ORDER BY m.identity_id
         LIMIT {list}"
    ))?;
    let mut printed = 0;
    for batch in &batches {
        for row in 0..batch.num_rows() {
            println!(
                "  {}  {}  {}  {}",
                text(batch, 0, row),
                text(batch, 1, row),
                text(batch, 2, row),
                text(batch, 3, row)
            );
            printed += 1;
        }
    }
    if total > printed {
        println!(
            "  ... and {} more; raise --list to print them",
            total - printed
        );
    }
    Ok(())
}
