//! `spool extract`: gives a running server documents and has one of the
//! project's extractors read them.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use anstream::println;
use indicatif::ProgressBar;
use serde_json::{Value, json};
use spool::core::RunId;
use spool::{ExtractReport, FailedDocument, RunEntry, RunStatus};

use super::remote::{self, Remote, RemoteArgs, WAIT_MS};
use crate::Failure;
use crate::output::{Output, OutputArgs, aligned, hint};
use crate::progress;
use spool_cli::text::count;

mod batches;
mod choose;
mod files;
mod report;
mod running;

use batches::{group_files_to_send, next_batch};
use choose::{check_source_readable, choose_extractor, source_of};
use files::read_files;
use report::{describe_stopped_extraction, print_found_records, shorten_found_text};
use running::Progress;

/// Find records in documents with one of the project's extractors
///
/// Extracts records from text, on a running server. Each file is one
/// document: the server stores its text and one of the project's extractors
/// reads it. What the extractor finds are records like any source's rows, and
/// the next match places them in identities.
///
/// A folder means every file under it, recursively. Dotfiles and dot-folders
/// are skipped, and every other file must be UTF-8 text. A file you name is
/// stored under its name. A file found in a folder is stored under its path
/// from that folder, so naming `documents` stores documents/dev/a.txt as
/// `dev/a.txt`.
///
/// With no path, the extractor reads every stored document it hasn't read.
///
/// Documents are read in batches, and each batch is kept when it ends. If the
/// command stops or fails, earlier batches are kept, and running it again
/// picks up the rest. A batch the command stops waiting for still finishes on
/// the server. Batches are sized to a fraction of the server's user-code
/// limit (`--task-timeout` on `spool dev` or `spool serve`), so long
/// extractions don't need a longer limit.
///
/// An extractor's first run may download its dependencies, such as a model.
/// On a terminal, a spinner shows the extractor's output while the first
/// batch starts, then a progress bar counts documents. On standard error the
/// command reports a slow start, batch totals when there are several, and the
/// extractor's own output when its batch ends.
#[derive(clap::Args)]
#[command(
    after_help = "Examples:\n  spool extract samples/notes-1.txt\n  spool extract documents/ --source notes\n  spool extract --again"
)]
pub struct ExtractArgs {
    /// Files of UTF-8 text, or folders of them
    #[arg(value_name = "PATH")]
    paths: Vec<String>,
    /// The source the documents belong to
    ///
    /// The default is the project's only source, or the one the files are
    /// named after, as notes-1.txt is after `notes`.
    #[arg(long, value_name = "NAME")]
    source: Option<String>,
    /// The extractor [default: the project's only one]
    #[arg(long, value_name = "NAME")]
    extractor: Option<String>,
    /// Read every document of the source again, not only the new ones
    ///
    /// Stopped, the command starts over when it is run again.
    #[arg(long)]
    again: bool,
    #[command(flatten)]
    output: OutputArgs,
    #[command(flatten)]
    remote: RemoteArgs,
}

/// The most records that are listed.
const LISTED: usize = 20;

/// How long an extraction is waited for in silence. One that takes longer
/// is followed: what its extractor says is printed as it says it.
const QUIET_MS: u64 = 3_000;

/// How often a running extractor is asked what it has said.
const ASK_MS: u64 = 1_000;

/// The most of a found text that a line shows, in characters.
const SHOWN: usize = 60;

/// The most documents sent to the server in one request, and the most text.
const SENT_DOCUMENTS: usize = 1_000;
const SENT_BYTES: usize = 16 * 1024 * 1024;

/// The documents of the first batch: few, so that an extractor that fails
/// fails early, and one that is slow is timed before it is given more.
const FIRST_BATCH: u64 = 5;

/// The most documents of one batch.
const MAX_BATCH: u64 = 1_000;

/// How much larger a batch may be than the one before it.
const GROWTH: u64 = 4;

/// What part of the time the server lets user code run a batch is made to
/// take, and the longest it is made to take whatever that time is. A batch
/// is sized from how long the ones before it took, and a document can take
/// longer than the ones before it.
const PART_OF_TIMEOUT: u32 = 5;
const LONGEST_BATCH: Duration = Duration::from_secs(120);

/// Whether Ctrl-C was pressed. The first press is caught, and the command
/// cancels the batch in progress and says what is kept; the second ends
/// the command at once.
static INTERRUPTED: LazyLock<Arc<AtomicBool>> = LazyLock::new(|| Arc::new(AtomicBool::new(false)));

/// Catches the first Ctrl-C. A request in flight is not cut short by it, so
/// the command notices within the second it asks the server again.
fn catch_ctrl_c() {
    #[cfg(unix)]
    {
        use signal_hook::consts::SIGINT;
        use signal_hook::flag;
        // The two are run in this order on each press. The first does what
        // an uncaught Ctrl-C does once the flag is set, which the second
        // does: so the first press sets it, and the next ends the command.
        let _ = flag::register_conditional_default(SIGINT, Arc::clone(&INTERRUPTED));
        let _ = flag::register(SIGINT, Arc::clone(&INTERRUPTED));
    }
}

fn was_interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// A file to store as a document.
struct File {
    /// Where it was read from.
    path: String,
    /// The document's key: the file's name, or its path from the folder
    /// that was named.
    name: String,
    text: String,
}

/// What the batches of an extraction found, together.
#[derive(Default)]
struct Found {
    /// The documents read: those that could not be are in `failed`.
    documents: u64,
    mentions: u64,
    inserted: u64,
    duplicates: u64,
    superseded: u64,
    failed: Vec<FailedDocument>,
    taken_back: u64,
}

impl Found {
    fn add(&mut self, report: &ExtractReport) {
        self.documents += report.documents - report.failed.len() as u64;
        self.failed.extend(report.failed.iter().cloned());
        self.taken_back += report.taken_back;
        self.mentions += report.mentions;
        self.inserted += report.inserted;
        self.duplicates += report.duplicates;
        self.superseded += report.superseded;
    }
}

/// The runs the server answered with, for `--json`.
#[derive(Default)]
struct Answered {
    documents: Vec<Value>,
    extractions: Vec<Value>,
}

impl Answered {
    fn print(&self, out: Output) {
        out.json(&json!({ "documents": self.documents, "extractions": self.extractions }));
    }
}

#[expect(clippy::too_many_lines)]
pub fn extract(arguments: ExtractArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool extract`")?;
    let files = read_files(&arguments.paths)?;
    catch_ctrl_c();

    let remote = Remote::new(&arguments.remote);
    let info = remote.info()?;
    let timeout = info.task_timeout_ms.map(Duration::from_millis);
    let info = info.store;
    let extractor = choose_extractor(&info.extractors, arguments.extractor.as_deref())?;
    let source = source_of(&info.sources, arguments.source.as_deref(), &files)?;
    if let Some(source) = &source {
        check_source_readable(&info.sources, source)?;
    }
    let as_json = !out.is_table();
    let again = arguments.again;
    let mut answered = Answered::default();

    if let Some(source) = &source
        && !files.is_empty()
    {
        let (mut inserted, mut duplicates, mut superseded) = (0, 0, 0);
        let mut stored = 0;
        let parts = group_files_to_send(&files);
        let storing = match parts.len() {
            1 => ProgressBar::hidden(),
            _ => progress::bar(
                "Storing",
                files.len() as u64,
                " documents",
                0,
                String::new(),
            ),
        };
        for sent in parts {
            if was_interrupted() {
                return Err(Failure::stopped(format!(
                    "stopped: {stored} of {} are stored and none is read yet. The same command \
                     again stores the rest and reads them.",
                    count(files.len() as u64, "document")
                )));
            }
            let documents: Vec<Value> = sent
                .iter()
                .map(|file| json!({ "key": file.name, "text": file.text }))
                .collect();
            // A request the server refuses at once, and a run that fails
            // later, both leave the documents sent before them stored.
            let sending = remote
                .post(
                    "/documents",
                    &json!({ "source": source, "documents": documents, "wait_ms": WAIT_MS }),
                )
                .and_then(|started| wait_for_run(&remote, started))
                .and_then(|(answer, run)| {
                    answered.documents.push(answer["run"].clone());
                    match run.status {
                        RunStatus::Succeeded => Ok(run),
                        _ => Err(remote::unsuccessful(&run)),
                    }
                });
            let run = match sending {
                Ok(run) => run,
                Err(failure) => {
                    if as_json {
                        answered.print(out);
                    }
                    if stored > 0 {
                        hint(format!(
                            "stored     {stored} of {} before this: the same command again \
                             stores the rest and reads them",
                            count(files.len() as u64, "document")
                        ));
                    }
                    return Err(failure);
                }
            };
            let report = run
                .load
                .as_ref()
                .ok_or_else(|| missing_report_failure(&run, "what it stored"))?;
            inserted += report.inserted;
            duplicates += report.duplicates;
            superseded += report.superseded;
            stored += sent.len();
            storing.set_position(stored as u64);
        }
        storing.finish_and_clear();
        if !as_json {
            println!("source     {source}");
            println!(
                "read       {} from {}",
                count(files.len() as u64, "document"),
                match (arguments.paths.as_slice(), files.as_slice()) {
                    (_, [file]) => file.path.clone(),
                    ([named], _) if Path::new(named).is_dir() => named.clone(),
                    (_, files) => count(files.len() as u64, "file"),
                }
            );
            println!(
                "loaded     {} new, {} already held, {}",
                inserted - superseded,
                duplicates,
                count(superseded, "new version"),
            );
        }
    }

    let target = timeout.map_or(LONGEST_BATCH, |timeout| {
        (timeout / PART_OF_TIMEOUT).min(LONGEST_BATCH)
    });
    let mut progress = Progress::new(extractor);
    let mut found = Found::default();
    let mut size = FIRST_BATCH;
    let mut since: Option<RunId> = None;
    loop {
        if was_interrupted() {
            return Err(Failure::stopped(describe_stopped_extraction(
                &found, again, None,
            )));
        }
        let mut request = json!({
            "extractor": extractor.name,
            "source": source,
            "again": again,
            "limit": size,
            "wait_ms": QUIET_MS,
        });
        if let Some(since) = since {
            request["since"] = json!(since);
        }
        let batch_began = Instant::now();
        // A batch the server refuses at once, as it does one that fails
        // within the wait, keeps the batches before it as well as one that
        // fails later.
        let waited = remote
            .post("/extractions", &request)
            .and_then(|started| progress.follow(&remote, started))
            .and_then(|answer| wait_for_run(&remote, answer));
        let (answer, run) = match waited {
            Ok(ended) => ended,
            Err(failure) => {
                if as_json {
                    answered.print(out);
                }
                progress.stopped(found.documents, again, None);
                return Err(failure);
            }
        };
        answered.extractions.push(answer["run"].clone());
        if run.status != RunStatus::Succeeded {
            if as_json {
                answered.print(out);
            }
            if progress.cancelled {
                return Err(Failure::stopped(describe_stopped_extraction(
                    &found,
                    again,
                    Some(run.run),
                )));
            }
            progress.stopped(found.documents, again, Some(run.run));
            return Err(remote::unsuccessful(&run));
        }
        let report = run
            .extraction
            .as_ref()
            .ok_or_else(|| missing_report_failure(&run, "what it found"))?;
        found.add(report);
        progress.relay(&report.stderr);
        // An extractor that finds nothing anywhere takes back all that was
        // found before, which is what a broken one does too. It is taken
        // back all the same, and comes back when it is found again.
        if report.mentions == 0 && report.taken_back > 0 {
            progress.say(&format!(
                "warning: took back {} that run {} found nowhere, though it found nothing at all: \
                 if the extractor is wrong, mend it and run `spool extract --again`, and what it \
                 finds again goes back to the identity it left",
                count(report.taken_back, "record"),
                run.run
            ));
        }
        // A batch none of whose documents could be read says more about the
        // extractor than about them: a key that is wrong, a model that is
        // not there. Going on would fail every batch after it.
        if report.documents > 0 && report.failed.len() as u64 == report.documents {
            if as_json {
                answered.print(out);
            }
            progress.stopped(found.documents, again, Some(run.run));
            let first = &report.failed[0];
            return Err(Failure::failed(format!(
                "run {} could not read any of its {}, as {}:{} says: {}. They are not \
                 read, and the same command reads them again.",
                run.run,
                count(report.documents, "document"),
                first.source,
                first.key,
                first.reason
            )));
        }
        if was_interrupted() && report.left > 0 {
            if as_json {
                answered.print(out);
            }
            return Err(Failure::stopped(describe_stopped_extraction(
                &found, again, None,
            )));
        }
        if since.is_none() {
            since = Some(run.run);
        }
        progress.kept(&found, report.left);
        if report.left == 0 || report.documents == 0 {
            break;
        }
        size = next_batch(report.documents, batch_began.elapsed(), target);
    }
    progress.finish(&found);

    if as_json {
        answered.print(out);
        return Ok(());
    }
    println!("extractor  {}", extractor.name);
    if found.documents == 0 && found.failed.is_empty() {
        let name = &extractor.name;
        println!(
            "found      0 records: {}",
            if again {
                match &source {
                    Some(source) => format!("the server holds no document of `{source}`."),
                    None => "the server holds no document.".to_owned(),
                }
            } else if info.documents == 0 && files.is_empty() {
                "the server holds no document. `spool extract <file>` gives it one.".to_owned()
            } else {
                format!("`{name}` has read every document. `--again` reads them again.")
            }
        );
        return Ok(());
    }
    println!(
        "found      {} in {}: {} new, {} already held{}",
        count(found.mentions, "record"),
        count(found.documents, "document"),
        found.inserted.saturating_sub(found.superseded),
        found.duplicates,
        match found.superseded {
            0 => String::new(),
            changed => format!(", {changed} changed"),
        }
    );

    if found.taken_back > 0 {
        println!(
            "taken back {}: no extractor finds {} any more",
            count(found.taken_back, "record"),
            if found.taken_back == 1 { "it" } else { "them" }
        );
    }
    if !found.failed.is_empty() {
        println!(
            "failed     {}: they are not read, and the same command reads them again",
            count(found.failed.len() as u64, "document")
        );
        let rows: Vec<Vec<String>> = found
            .failed
            .iter()
            .take(LISTED)
            .map(|failed| {
                vec![
                    format!("{}:{}", failed.source, failed.key),
                    shorten_found_text(&failed.reason),
                ]
            })
            .collect();
        aligned(&rows, 2);
        if found.failed.len() > LISTED {
            hint("and more: `spool show documents` says why each failed");
        }
    }

    if let Some(source) = &source
        && !files.is_empty()
    {
        print_found_records(
            &remote,
            source,
            &files[..files.len().min(LISTED)],
            found.mentions,
        )?;
    }
    if found.inserted > 0 {
        hint(format!(
            "next: `spool match` places {} in identities",
            if found.inserted == 1 { "it" } else { "them" }
        ));
    }
    Ok(())
}

/// Waits for the run a call started.
fn wait_for_run(remote: &Remote, answer: Value) -> Result<(Value, RunEntry), Failure> {
    remote.ended(answer)?.ok_or_else(|| {
        Failure::failed(
            "the server started no run; is it a Spool server of this version?".to_owned(),
        )
    })
}

fn missing_report_failure(run: &RunEntry, what: &str) -> Failure {
    Failure::failed(format!(
        "run {} succeeded and did not report {what}; is the server of this version?",
        run.run
    ))
}
