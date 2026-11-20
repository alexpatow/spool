//! What the command shows while an extraction runs, and following a
//! batch until it ends.

use std::time::{Duration, Instant};

use indicatif::ProgressBar;
use serde_json::{Value, json};
use spool::core::RunId;
use spool::{Extractor, RunEntry, RunStatus};
use spool_cli::text::count;

use super::{ASK_MS, Found, QUIET_MS, was_interrupted};
use crate::Failure;
use crate::commands::remote::{Remote, typed};
use crate::progress;

/// What the command shows on standard error while an extraction runs: a
/// spinner with what the extractor says while its first batch starts, then
/// a bar of the documents, and what the extractor wrote above them. What it
/// prints on standard output is the same however long it took.
pub(super) struct Progress<'a> {
    extractor: &'a Extractor,
    /// The batches that have ended.
    batches: u64,
    /// The spinner, then the bar once the documents are counted.
    bar: ProgressBar,
    counted: bool,
    began: Instant,
    /// Whether the batch in progress was cancelled after Ctrl-C.
    pub(super) cancelled: bool,
}

impl<'a> Progress<'a> {
    pub(super) fn new(extractor: &'a Extractor) -> Self {
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
    pub(super) fn follow(&mut self, remote: &Remote, mut answer: Value) -> Result<Value, Failure> {
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
    pub(super) fn relay(&self, stderr: &[String]) {
        for line in stderr.iter().filter(|line| !counts_documents(line)) {
            self.say(&format!("{:>12} {line}", ""));
        }
    }

    pub(super) fn say(&self, line: &str) {
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
    pub(super) fn kept(&mut self, found: &Found, left: u64) {
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
    pub(super) fn finish(&self, found: &Found) {
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
    pub(super) fn stopped(&self, kept: u64, again: bool, failed: Option<RunId>) {
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
