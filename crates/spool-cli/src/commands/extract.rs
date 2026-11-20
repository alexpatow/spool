//! `spool extract`: gives a running server documents and has one of the
//! project's extractors read them.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use anstream::println;
use indicatif::ProgressBar;
use serde_json::{Value, json};
use spool::core::{DocumentId, RunId};
use spool::{
    DeclaredSource, ExtractReport, Extractor, FailedDocument, RecordEntry, RunEntry, RunStatus,
};

use super::remote::{self, Remote, RemoteArgs, WAIT_MS, typed};
use crate::Failure;
use crate::output::{Output, OutputArgs, aligned, hint};
use crate::progress;
use spool_cli::text::{count, listed};

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
    let files = read(&arguments.paths)?;
    catch_ctrl_c();

    let remote = Remote::new(&arguments.remote);
    let info = remote.info()?;
    let timeout = info.task_timeout_ms.map(Duration::from_millis);
    let info = info.store;
    let extractor = extractor(&info.extractors, arguments.extractor.as_deref())?;
    let source = source_of(&info.sources, arguments.source.as_deref(), &files)?;
    if let Some(source) = &source {
        readable(&info.sources, source)?;
    }
    let as_json = !out.is_table();
    let again = arguments.again;
    let mut answered = Answered::default();

    if let Some(source) = &source
        && !files.is_empty()
    {
        let (mut inserted, mut duplicates, mut superseded) = (0, 0, 0);
        let mut stored = 0;
        let parts = sending(&files);
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
                .and_then(|started| ended(&remote, started))
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
                .ok_or_else(|| unreported(&run, "what it stored"))?;
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
            return Err(Failure::stopped(stopped(&found, again, None)));
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
            .and_then(|answer| ended(&remote, answer));
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
                return Err(Failure::stopped(stopped(&found, again, Some(run.run))));
            }
            progress.stopped(found.documents, again, Some(run.run));
            return Err(remote::unsuccessful(&run));
        }
        let report = run
            .extraction
            .as_ref()
            .ok_or_else(|| unreported(&run, "what it found"))?;
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
            return Err(Failure::stopped(stopped(&found, again, None)));
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
                    shown(&failed.reason),
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
        list(
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

/// How many documents the next batch is given: as many as take `target`
/// at the pace of the batch before, which read `read` documents in `took`.
/// It is at most `GROWTH` times as many, so that one quick batch does not
/// make the next one too long.
fn next_batch(read: u64, took: Duration, target: Duration) -> u64 {
    let fits = target.as_secs_f64() * read as f64 / took.as_secs_f64().max(0.001);
    (fits as u64).clamp(1, (read * GROWTH).clamp(1, MAX_BATCH))
}

/// The files in groups that are each sent in one request.
fn sending(files: &[File]) -> Vec<&[File]> {
    let mut groups = Vec::new();
    let (mut start, mut bytes) = (0, 0);
    for (index, file) in files.iter().enumerate() {
        if index > start
            && (index - start == SENT_DOCUMENTS || bytes + file.text.len() > SENT_BYTES)
        {
            groups.push(&files[start..index]);
            (start, bytes) = (index, 0);
        }
        bytes += file.text.len();
    }
    if start < files.len() {
        groups.push(&files[start..]);
    }
    groups
}

/// Reads the files the paths name. A file is one document, stored under its
/// name, and a folder stands for the files under it, each stored under its
/// path from the folder.
fn read(paths: &[String]) -> Result<Vec<File>, Failure> {
    let mut found: Vec<(String, String)> = Vec::new();
    for path in paths {
        let metadata =
            std::fs::metadata(path).map_err(|error| Failure::failed(format!("{path}: {error}")))?;
        if metadata.is_dir() {
            let before = found.len();
            under(Path::new(path), "", &mut found)?;
            if found.len() == before {
                return Err(Failure::failed(format!(
                    "{path} is a folder with no files in it, other than ones whose names start \
                     with a dot"
                )));
            }
            continue;
        }
        let name = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| Failure::failed(format!("{path} does not name a file")))?;
        found.push((path.clone(), name));
    }

    // A second document under the same key would be taken for a new
    // version of the first.
    let mut keys: HashMap<&str, &str> = HashMap::new();
    for (path, name) in &found {
        if let Some(first) = keys.insert(name, path) {
            return Err(Failure::failed(format!(
                "{first} and {path} would both be stored as `{name}`: a file is stored under its \
                 name, or under its path from the folder that was named, and a second document \
                 under one key is taken for a new version of the first. Name the folder that \
                 holds both, or extract them into sources of their own."
            )));
        }
    }

    let mut files = Vec::with_capacity(found.len());
    for (path, name) in found {
        let bytes =
            std::fs::read(&path).map_err(|error| Failure::failed(format!("{path}: {error}")))?;
        let text = String::from_utf8(bytes).map_err(|_| {
            Failure::failed(format!(
                "{path} is not text: a document is stored as UTF-8 text. Turn a PDF or a web \
                 page into text first."
            ))
        })?;
        if text.trim().is_empty() {
            return Err(Failure::failed(format!("{path} is empty")));
        }
        files.push(File { path, name, text });
    }
    Ok(files)
}

/// Adds the files under `folder` to `found`, in the order of their names,
/// each with its key: its path from the folder that was named, after
/// `prefix`. What starts with a dot is left out. A link to a file is read
/// and a link to a folder is not followed, since it can lead back up.
fn under(folder: &Path, prefix: &str, found: &mut Vec<(String, String)>) -> Result<(), Failure> {
    let unreadable =
        |error: std::io::Error| Failure::failed(format!("{}: {error}", folder.display()));
    let mut entries = std::fs::read_dir(folder)
        .and_then(|entries| entries.collect::<Result<Vec<_>, _>>())
        .map_err(unreadable)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let key = match prefix {
            "" => name,
            prefix => format!("{prefix}/{name}"),
        };
        let kind = entry.file_type().map_err(unreadable)?;
        let is_file = if kind.is_dir() {
            under(&path, &key, found)?;
            false
        } else if kind.is_symlink() {
            std::fs::metadata(&path)
                .map_err(|error| Failure::failed(format!("{}: {error}", path.display())))?
                .is_file()
        } else {
            kind.is_file()
        };
        if is_file {
            found.push((path.display().to_string(), key));
        }
    }
    Ok(())
}

/// The extractor that was named, or the project's only one.
fn extractor<'a>(known: &'a [Extractor], named: Option<&str>) -> Result<&'a Extractor, Failure> {
    let names: Vec<&str> = known
        .iter()
        .map(|extractor| extractor.name.as_str())
        .collect();
    match (named, known) {
        (_, []) => Err(Failure::failed(
            "the project has no extractors: one is a file under extractors/, and \
             `spool init <dir> --example companies` makes a project with a sample"
                .to_owned(),
        )),
        (Some(named), _) => known
            .iter()
            .find(|extractor| extractor.name == named)
            .ok_or_else(|| {
                Failure::failed(format!(
                    "the project has no extractor `{named}`: it has {}",
                    listed(&names)
                ))
            }),
        (None, [only]) => Ok(only),
        (None, _) => Err(Failure::failed(format!(
            "the project has the extractors {}: say which with `--extractor`",
            listed(&names)
        ))),
    }
}

/// The source the documents belong to: the one that was named, the
/// project's only source, or the one every file is named after. `None` when
/// none was named and there is no file: the extractor then reads the
/// documents of every source.
fn source_of(
    sources: &[DeclaredSource],
    named: Option<&str>,
    files: &[File],
) -> Result<Option<String>, Failure> {
    let names: Vec<&str> = sources.iter().map(|source| source.name.as_str()).collect();
    if let Some(named) = named {
        if !names.is_empty() && !names.contains(&named) {
            return Err(Failure::failed(format!(
                "the project has no source `{named}`: there is no sources/{named}.yaml. It has \
                 {}.",
                listed(&names)
            )));
        }
        return Ok(Some(named.to_owned()));
    }
    if files.is_empty() {
        return Ok(None);
    }
    match names.as_slice() {
        [] => {
            return Err(Failure::failed(
                "the project declares no source, so there is nothing a document can belong to. \
                 A source is a file sources/<name>.yaml that lists what a record found in its \
                 documents may say."
                    .to_owned(),
            ));
        }
        [only] => return Ok(Some((*only).to_owned())),
        _ => {}
    }
    let mut found: Option<&str> = None;
    for file in files {
        let Some(source) = remote::named_after(&file.name, &names) else {
            return Err(Failure::failed(format!(
                "the project has the sources {}, and {} is named after {}: say which its \
                 documents belong to with `--source`",
                listed(&names),
                file.name,
                match names.len() {
                    2 => "neither",
                    _ => "none of them",
                }
            )));
        };
        match found {
            Some(first) if first != source => {
                return Err(Failure::failed(format!(
                    "the files are named after different sources, `{first}` and `{source}`: \
                     extract them one source at a time, or say which they all belong to with \
                     `--source`"
                )));
            }
            _ => found = Some(source),
        }
    }
    Ok(found.map(str::to_owned))
}

/// Refuses a source in whose documents the extractor can find no record,
/// before anything is stored: what an extractor finds is a record of an
/// entity type, and the type's mapping has to say what it reads from the
/// source.
fn readable(sources: &[DeclaredSource], source: &str) -> Result<(), Failure> {
    let Some(declared) = sources.iter().find(|declared| declared.name == source) else {
        return Ok(());
    };
    if declared.entity_types.is_empty() {
        return Err(Failure::failed(format!(
            "no entity type reads source `{source}`, so nothing found in its documents can be \
             a record. Add the source to the `mapping` in an entities/<type>/entity.yaml."
        )));
    }
    Ok(())
}

/// What the command shows on standard error while an extraction runs: a
/// spinner with what the extractor says while its first batch starts, then
/// a bar of the documents, and what the extractor wrote above them. What it
/// prints on standard output is the same however long it took.
struct Progress<'a> {
    extractor: &'a Extractor,
    /// The batches that have ended.
    batches: u64,
    /// The spinner, then the bar once the documents are counted.
    bar: ProgressBar,
    counted: bool,
    began: Instant,
    /// Whether the batch in progress was cancelled after Ctrl-C.
    cancelled: bool,
}

impl<'a> Progress<'a> {
    fn new(extractor: &'a Extractor) -> Self {
        Progress {
            extractor,
            batches: 0,
            bar: progress::spinner(format!("{}: starting", extractor.file)),
            counted: false,
            began: Instant::now(),
            cancelled: false,
        }
    }

    /// Waits for a batch that did not end at once, and returns the
    /// server's answer once it has. While the first batch runs, the spinner
    /// says what its extractor says: the first run of a Python file takes as
    /// long as its downloads, and this is all there is to see of them.
    fn follow(&mut self, remote: &Remote, mut answer: Value) -> Result<Value, Failure> {
        loop {
            if answer["run"].is_null() {
                return Ok(answer);
            }
            let run: RunEntry = typed(&answer["run"])?;
            if run.status != RunStatus::Running {
                return Ok(answer);
            }
            if was_interrupted() && !self.cancelled {
                self.cancel(remote, run.run)?;
            }
            if self.batches == 0
                && let Some(said) = run.said
            {
                self.bar
                    .set_message(format!("{}: {said}", self.extractor.file));
            }
            answer = remote.get(
                &format!("/runs/{}", run.run),
                &[("wait_ms", ASK_MS.to_string())],
                || format!("the server no longer has run {}", run.run),
            )?;
        }
    }

    /// Says what the extractor of a batch that ended wrote to standard
    /// error, but for the helpers' own count of how far a run is, which the
    /// bar says better.
    fn relay(&self, stderr: &[String]) {
        for line in stderr.iter().filter(|line| !counts_documents(line)) {
            self.say(&format!("{:>12} {line}", ""));
        }
    }

    fn say(&self, line: &str) {
        progress::say(&self.bar, line);
    }

    /// Cancels the batch in progress after Ctrl-C. The server stops its
    /// extractor and keeps nothing it found; what the batches before it
    /// found is kept.
    fn cancel(&mut self, remote: &Remote, run: RunId) -> Result<(), Failure> {
        progress::note(
            &self.bar,
            "Stopping",
            &format!(
                "run {run}, the batch in progress: what the batches before it read is kept. \
                 Ctrl-C again quits without waiting"
            ),
        );
        self.cancelled = true;
        // A run that ended meanwhile cannot be cancelled, and what it read
        // is kept.
        let _ = remote.post(&format!("/runs/{run}/cancel"), &json!({}));
        Ok(())
    }

    /// Moves on once a batch has been kept: says how long the first took,
    /// when that was long enough to be worth saying, and moves the bar.
    fn kept(&mut self, found: &Found, left: u64) {
        self.batches += 1;
        if self.batches == 1 && self.began.elapsed() >= Duration::from_millis(QUIET_MS) {
            progress::done(
                &self.bar,
                "Prepared",
                &format!(
                    "{} in {}",
                    self.extractor.file,
                    progress::took(self.began.elapsed())
                ),
            );
        }
        if self.batches == 1 && left == 0 {
            return;
        }
        let read = found.documents + found.failed.len() as u64;
        let records = format!(", {}", count(found.mentions, "record"));
        if self.counted {
            self.bar.set_position(read);
            self.bar.set_message(records);
            return;
        }
        self.bar.finish_and_clear();
        self.bar = progress::bar("Extracting", read + left, " documents", read, records);
        self.counted = true;
    }

    /// Takes the bar away, and says what the batches came to when there
    /// was more than one.
    fn finish(&self, found: &Found) {
        self.bar.finish_and_clear();
        if self.batches > 1 {
            progress::done(
                &self.bar,
                "Extracted",
                &format!(
                    "{} in {}: {}, {} failed",
                    count(found.documents, "document"),
                    progress::took(self.began.elapsed()),
                    count(found.mentions, "record"),
                    found.failed.len()
                ),
            );
        }
    }

    /// Says what the batches before a failed one kept: the run that failed,
    /// when the server started one.
    fn stopped(&self, kept: u64, again: bool, failed: Option<RunId>) {
        self.bar.finish_and_clear();
        if kept == 0 {
            return;
        }
        let read = count(kept, "document");
        let before = match failed {
            Some(run) => format!("run {run}"),
            None => "the batch that failed".to_owned(),
        };
        match again {
            true => progress::done(
                &self.bar,
                "Kept",
                &format!("{read} read again before {before}"),
            ),
            false => progress::done(
                &self.bar,
                "Kept",
                &format!("{read} read before {before}: the same command again reads the rest"),
            ),
        }
    }
}

/// Whether a line is the helpers' count of how far a run is, such as
/// `read 400 of 3,053 documents`.
fn counts_documents(line: &str) -> bool {
    line.strip_prefix("read ")
        .and_then(|rest| rest.strip_suffix(" documents"))
        .and_then(|rest| rest.split_once(" of "))
        .is_some_and(|(read, of)| {
            [read, of].iter().all(|number| {
                !number.is_empty() && number.chars().all(|c| c.is_ascii_digit() || c == ',')
            })
        })
}

/// What a command stopped with Ctrl-C says it left: the documents read
/// before it stopped are kept, and those of the run it cancelled are not.
fn stopped(found: &Found, again: bool, cancelled: Option<RunId>) -> String {
    let mut said = String::from("stopped");
    if let Some(run) = cancelled {
        said.push_str(&format!(
            ": run {run} was cancelled, and nothing it read is kept"
        ));
    }
    let kept = count(found.documents, "document");
    let are = if found.documents == 1 { "is" } else { "are" };
    match (again, found.documents) {
        (false, 0) => said.push_str(". The same command again reads the documents."),
        (false, _) => said.push_str(&format!(
            ". The {kept} read before it {are} kept, and the same command again reads the rest."
        )),
        (true, 0) => said.push('.'),
        (true, _) => said.push_str(&format!(
            ". The {kept} read again before it {are} kept. `--again` starts over."
        )),
    }
    said
}

/// Waits for the run a call started.
fn ended(remote: &Remote, answer: Value) -> Result<(Value, RunEntry), Failure> {
    remote.ended(answer)?.ok_or_else(|| {
        Failure::failed(
            "the server started no run; is it a Spool server of this version?".to_owned(),
        )
    })
}

/// Prints the records of the files' documents, in the order of the files
/// and of the text. They are what the extractor found there, and what any
/// other extractor found there before it.
fn list(remote: &Remote, source: &str, files: &[File], mentions: u64) -> Result<(), Failure> {
    let mut lines = Vec::new();
    for file in files {
        let document = DocumentId::derive(source, &file.name, &file.text);
        let mut after = None;
        let mut found = Vec::new();
        loop {
            let mut query = vec![("document", document.to_string())];
            if let Some(after) = after {
                query.push(("after", after));
            }
            let page = remote.list("/records", &query)?;
            let records: Vec<RecordEntry> = typed(&page["records"])?;
            found.extend(
                records
                    .into_iter()
                    .filter(|record| record.superseded_by.is_none()),
            );
            match page["next"].as_str() {
                Some(next) => after = Some(next.to_owned()),
                None => break,
            }
        }
        for record in &found {
            let text = record
                .evidence
                .as_ref()
                .and_then(|evidence| evidence["text"].as_str())
                .unwrap_or_default();
            lines.push((record.entity_type.to_string(), shown(text), record.name()));
        }
    }
    if lines.is_empty() {
        return Ok(());
    }
    let rows: Vec<Vec<String>> = lines
        .iter()
        .take(LISTED)
        .map(|(entity_type, text, place)| vec![entity_type.clone(), text.clone(), place.clone()])
        .collect();
    println!();
    aligned(&rows, 2);
    let listed = lines.len().min(LISTED) as u64;
    // The extractor read whatever the source held that it had not read, and
    // that may be more than the files.
    if mentions > listed || lines.len() > LISTED {
        hint(format!(
            "and more: `spool show records --source {source}` lists every record of the source"
        ));
    }
    Ok(())
}

/// A found text on one line, cut short when it is long.
fn shown(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= SHOWN {
        return line;
    }
    let mut cut: String = line.chars().take(SHOWN - 1).collect();
    cut.push('…');
    cut
}

fn unreported(run: &RunEntry, what: &str) -> Failure {
    Failure::failed(format!(
        "run {} succeeded and did not report {what}; is the server of this version?",
        run.run
    ))
}

#[cfg(test)]
mod tests {
    use spool::DeclaredSource;

    use std::time::Duration;

    use super::{File, next_batch, read, sending, source_of};

    /// The files the paths name, which have to be read.
    fn read_all(paths: &[String]) -> Vec<File> {
        read(paths).unwrap_or_else(|failure| panic!("{}", failure.message()))
    }

    /// What `source_of` says of files called `files` in a project with
    /// `sources`: the source, or why there is none.
    fn said(sources: &[&str], named: Option<&str>, files: &[&str]) -> String {
        let sources: Vec<DeclaredSource> = sources
            .iter()
            .map(|name| DeclaredSource {
                name: (*name).to_owned(),
                key: None,
                evidence: None,
                columns: vec!["name".to_owned()],
                entity_types: Vec::new(),
            })
            .collect();
        let files: Vec<File> = files
            .iter()
            .map(|name| File {
                path: format!("samples/{name}"),
                name: (*name).to_owned(),
                text: "text".to_owned(),
            })
            .collect();
        match source_of(&sources, named, &files) {
            Ok(source) => source.unwrap_or_else(|| "every source".to_owned()),
            Err(failure) => failure.message(),
        }
    }

    #[test]
    fn the_source_is_the_one_named_the_only_one_or_the_one_the_files_are_named_after() {
        assert_eq!(said(&["notes"], None, &["call.txt"]), "notes");
        assert_eq!(said(&["crm", "notes"], None, &["notes-1.txt"]), "notes");
        assert_eq!(
            said(&["crm", "notes"], None, &["notes-1.txt", "notes.md"]),
            "notes"
        );
        assert_eq!(said(&["crm", "notes"], Some("crm"), &["call.txt"]), "crm");
        // With no file, the extractor reads what the server holds.
        assert_eq!(said(&["crm", "notes"], None, &[]), "every source");
        assert_eq!(said(&["crm", "notes"], Some("notes"), &[]), "notes");
    }

    #[test]
    fn a_source_that_cannot_be_told_is_asked_for() {
        assert_eq!(
            said(&["crm", "notes"], None, &["call.txt"]),
            "the project has the sources `crm` and `notes`, and call.txt is named after \
             neither: say which its documents belong to with `--source`"
        );
        assert_eq!(
            said(
                &["crm", "notes", "wire"],
                None,
                &["notes-1.txt", "call.txt"]
            ),
            "the project has the sources `crm`, `notes` and `wire`, and call.txt is named after \
             none of them: say which its documents belong to with `--source`"
        );
        assert_eq!(
            said(&["crm", "notes"], None, &["notes-1.txt", "crm-1.txt"]),
            "the files are named after different sources, `notes` and `crm`: extract them one \
             source at a time, or say which they all belong to with `--source`"
        );
        assert_eq!(
            said(&["crm", "notes"], Some("news"), &["call.txt"]),
            "the project has no source `news`: there is no sources/news.yaml. It has `crm` and \
             `notes`."
        );
        assert_eq!(
            said(&[], None, &["call.txt"]),
            "the project declares no source, so there is nothing a document can belong to. A \
             source is a file sources/<name>.yaml that lists what a record found in its \
             documents may say."
        );
    }

    #[test]
    fn a_batch_is_made_to_take_the_time_it_is_given_and_grows_by_steps() {
        let target = Duration::from_secs(120);
        // Five documents in ten seconds: sixty fit, and twenty are allowed.
        assert_eq!(next_batch(5, Duration::from_secs(10), target), 20);
        // Twenty in a minute: forty fit.
        assert_eq!(next_batch(20, Duration::from_secs(60), target), 40);
        // A batch that took longer than the time gets one smaller.
        assert_eq!(next_batch(40, Duration::from_secs(240), target), 20);
        // However slow, one document; however quick, a thousand.
        assert_eq!(next_batch(5, Duration::from_secs(3600), target), 1);
        assert_eq!(next_batch(800, Duration::from_millis(1), target), 1_000);
        // A server that lets code run twenty-five seconds gives five.
        assert_eq!(
            next_batch(5, Duration::from_secs(5), Duration::from_secs(5)),
            5
        );
    }

    #[test]
    fn files_are_sent_a_thousand_or_sixteen_megabytes_at_a_time() {
        let file = |bytes: usize| File {
            path: String::new(),
            name: String::new(),
            text: "x".repeat(bytes),
        };
        let small: Vec<File> = (0..2_500).map(|_| file(10)).collect();
        let sizes: Vec<usize> = sending(&small).iter().map(|group| group.len()).collect();
        assert_eq!(sizes, vec![1_000, 1_000, 500]);
        let large: Vec<File> = (0..3).map(|_| file(10 * 1024 * 1024)).collect();
        let sizes: Vec<usize> = sending(&large).iter().map(|group| group.len()).collect();
        assert_eq!(sizes, vec![1, 1, 1]);
        assert!(sending(&[]).is_empty());
    }

    #[test]
    fn a_folder_is_its_files_by_their_paths_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("documents");
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("train/b.txt", "B");
        write("dev/a.txt", "A");
        write("dev/deeper/c.md", "C");
        write(".git/HEAD", "ref");
        write("dev/.DS_Store", "\0");
        let elsewhere = dir.path().join("elsewhere.txt");
        std::fs::write(&elsewhere, "E").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&elsewhere, root.join("linked.txt")).unwrap();
            // A link to a folder above would lead round and round.
            std::os::unix::fs::symlink(dir.path(), root.join("dev/up")).unwrap();
        }

        let files = read_all(&[root.display().to_string()]);
        let keys: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        #[cfg(unix)]
        assert_eq!(
            keys,
            ["dev/a.txt", "dev/deeper/c.md", "linked.txt", "train/b.txt"]
        );
        assert_eq!(files[0].text, "A");
        assert_eq!(files[0].path, root.join("dev/a.txt").display().to_string());

        // A file named on its own is stored under its name.
        let files = read_all(&[root.join("dev/a.txt").display().to_string()]);
        assert_eq!(files[0].name, "a.txt");
        // A hidden file that is named is read.
        let files = read_all(&[root.join(".git/HEAD").display().to_string()]);
        assert_eq!(files[0].name, "HEAD");
    }
}
