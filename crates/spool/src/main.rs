//! The `spool` command. For now it runs one benchmark scenario.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use spool::arrow::array::{Array, AsArray};
use spool::core::ReviewItem;
use spool::{BenchReport, Engine, Options, Scenario, StepReport};

const USAGE: &str = "\
Usage: spool bench <scenario> [options]

Runs a benchmark scenario: ingests each step's files, resolves, and scores
committed state against the scenario's truth.

  <scenario>      a scenario.toml, or the directory that holds one

Options:
  --db <path>     the database file to build; replaced if it exists
                  (default: .spool/bench.duckdb in the project directory)
  --seed <n>      the seed for identity and changeset IDs (default: 0)
  --out <path>    write the results as JSON to this file
  --list <n>      print at most this many identities per step (default: 50)
";

struct Arguments {
    scenario: PathBuf,
    db: Option<PathBuf>,
    seed: u64,
    out: Option<PathBuf>,
    list: usize,
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.is_empty() || arguments.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let arguments = match parse(&arguments) {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("spool: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match bench(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spool: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut rest = arguments.iter();
    match rest.next().map(String::as_str) {
        Some("bench") => {}
        Some(other) => return Err(format!("unknown command `{other}`")),
        None => return Err("a command is needed".to_owned()),
    }
    let mut parsed = Arguments {
        scenario: PathBuf::new(),
        db: None,
        seed: 0,
        out: None,
        list: 50,
    };
    let mut scenario = None;
    while let Some(argument) = rest.next() {
        let mut value = || {
            rest.next()
                .ok_or_else(|| format!("`{argument}` needs a value"))
        };
        let number = |value: &String| {
            value
                .parse::<u64>()
                .map_err(|_| format!("`{argument}` needs a whole number, not `{value}`"))
        };
        match argument.as_str() {
            "--db" => parsed.db = Some(PathBuf::from(value()?)),
            "--out" => parsed.out = Some(PathBuf::from(value()?)),
            "--seed" => parsed.seed = number(value()?)?,
            "--list" => parsed.list = number(value()?)? as usize,
            option if option.starts_with('-') => return Err(format!("unknown option `{option}`")),
            path if scenario.is_none() => scenario = Some(PathBuf::from(path)),
            extra => return Err(format!("unexpected argument `{extra}`")),
        }
    }
    parsed.scenario = scenario.ok_or("`bench` needs a scenario")?;
    Ok(parsed)
}

fn bench(arguments: &Arguments) -> spool::Result<()> {
    let scenario = Scenario::load(&arguments.scenario)?;
    let db = match &arguments.db {
        Some(db) => db.clone(),
        None => scenario.project_dir().join(".spool").join("bench.duckdb"),
    };
    start_afresh(&db)?;

    let engine = Engine::open(&db, Options::seeded(arguments.seed))?;
    println!("scenario  {}", scenario.name);
    println!("database  {}", db.display());
    let report = scenario.run(&engine, |step| print_step(&engine, step, arguments.list))?;

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
    if step.resolve.resolved.is_empty() {
        println!("resolved  nothing new");
    }
    for resolved in &step.resolve.resolved {
        println!(
            "resolved  `{}`: {} new observations, {} candidate pairs, {} match and {} no-match judgements",
            resolved.entity_type,
            resolved.new_observations,
            resolved.candidates,
            resolved.judgements.match_,
            resolved.judgements.no_match,
        );
        match &resolved.committed {
            Some(committed) => println!(
                "committed changeset {} at {}: {} identities created, {} observations assigned, {} left unassigned",
                committed.changeset,
                committed.seq,
                resolved.identities_created,
                resolved.observations_assigned,
                resolved.observations_unassigned,
            ),
            None => println!(
                "committed nothing: {} observations left unassigned",
                resolved.observations_unassigned
            ),
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
        println!("\nfor review:");
        let keys = observation_keys(engine)?;
        let name = |id: &spool::core::ObservationId| {
            keys.get(&id.to_string())
                .cloned()
                .unwrap_or_else(|| id.to_string())
        };
        let names = |ids: &[spool::core::ObservationId]| {
            ids.iter().map(&name).collect::<Vec<_>>().join(", ")
        };
        for item in review {
            match item {
                ReviewItem::PossibleMerge {
                    observations,
                    identities,
                } => println!(
                    "  possible merge: {} matched identities {}",
                    names(observations),
                    identities
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" and "),
                ),
                ReviewItem::Conflict {
                    observations,
                    identity,
                } => match identity {
                    Some(identity) => println!(
                        "  conflict: {} both matched and did not match identity {identity}",
                        names(observations)
                    ),
                    None => println!(
                        "  conflict: {} matched each other, with a no-match among them",
                        names(observations)
                    ),
                },
            }
        }
    }

    let metrics = &step.metrics;
    println!(
        "\nagainst the truth ({} observations):",
        metrics.observations
    );
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
        "  ID stability  {} of {} observations kept their identity ({})",
        stability.unchanged,
        stability.observations,
        share(stability.strict)
    );
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

/// Observation ID to `source:key`, for messages a person reads.
fn observation_keys(engine: &Engine) -> spool::Result<std::collections::HashMap<String, String>> {
    let batches = engine.query(
        "SELECT observation_id::VARCHAR, source_id || ':' || coalesce(source_key, '?')
         FROM spool.observation",
    )?;
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
                string_agg(o.source_id || ':' || coalesce(o.source_key, '?'), ', '
                           ORDER BY o.source_id, o.source_key)
         FROM spool.membership_current m
         JOIN spool.identity_current i ON i.identity_id = m.identity_id
         JOIN spool.observation o ON o.observation_id = m.observation_id
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
