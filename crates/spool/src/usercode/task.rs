//! One run of a strategy or an extractor: its task directory, what is
//! written to it and read from it, and the command that works in it, which
//! is started, waited for and stopped here.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::json;
use spool_core::{EntityType, RunId};

use super::{COUNTER, Code, SAID, UserCode, last_line};
use crate::error::{Error, Result};
use crate::project::{Extractor, FeatureColumn, Strategy};

/// The version of the task directory's layout. A helper refuses a task of
/// another version.
pub(crate) const PROTOCOL: u32 = 1;

pub(crate) const CANDIDATES: &str = "candidates.parquet";
pub(crate) const FEATURES: &str = "features.parquet";
pub(crate) const JUDGEMENTS: &str = "judgements.parquet";
pub(crate) const DOCUMENTS: &str = "documents.parquet";
pub(crate) const MENTIONS: &str = "mentions.parquet";
/// The documents an extractor could not read, with why. It need not write
/// the file.
pub(crate) const FAILED: &str = "failed.parquet";

/// The most lines of what user code wrote to standard error that a run
/// keeps, from the end.
const KEPT_LINES: usize = 100;

/// What a task tells the strategy about its run.
pub(crate) struct TaskInfo<'a> {
    pub run: RunId,
    pub entity_type: &'a EntityType,
    pub declaration_version: u32,
    pub rematch: bool,
    pub features: &'a [FeatureColumn],
}

/// One entity type as an extraction task describes it.
pub(crate) struct ExtractType<'a> {
    pub name: &'a str,
    /// The text of the type's `extract.md`, if it has one.
    pub instructions: Option<&'a str>,
    pub features: &'a [FeatureColumn],
}

/// One run of a strategy or an extractor: its directory and the command
/// that works in it.
pub(crate) struct Task {
    pub dir: PathBuf,
    run: RunId,
    code: Code,
    command: Command,
    user_code: UserCode,
    /// The file the code is to leave in the directory.
    output: &'static str,
}

impl Task {
    /// Makes the directory of a strategy's task and writes `task.json`. The
    /// Parquet files are the engine's to write, since only it reads the
    /// store.
    pub(crate) fn create(
        user_code: &UserCode,
        strategy: &Strategy,
        info: &TaskInfo,
    ) -> Result<Self> {
        let description = json!({
            "protocol": PROTOCOL,
            "kind": "match",
            "run": info.run,
            "entity_type": info.entity_type,
            "strategy": { "name": strategy.name, "version": strategy.hash },
            "declaration_version": info.declaration_version,
            "rematch": info.rematch,
            "features": info.features,
            "inputs": { "candidates": CANDIDATES, "features": FEATURES },
            "output": JUDGEMENTS,
        });
        Self::write(
            user_code,
            strategy.into(),
            info.run,
            description,
            JUDGEMENTS,
        )
    }

    /// Makes the directory of an extractor's task and writes `task.json`.
    pub(crate) fn extract(
        user_code: &UserCode,
        extractor: &Extractor,
        run: RunId,
        entity_types: &[ExtractType],
    ) -> Result<Self> {
        let entity_types: Vec<_> = entity_types
            .iter()
            .map(|entity_type| {
                json!({
                    "name": entity_type.name,
                    "instructions": entity_type.instructions,
                    "features": entity_type.features,
                })
            })
            .collect();
        let description = json!({
            "protocol": PROTOCOL,
            "kind": "extract",
            "run": run,
            "extractor": { "name": extractor.name, "version": extractor.hash },
            "entity_types": entity_types,
            "inputs": { "documents": DOCUMENTS },
            "output": MENTIONS,
            "failed": FAILED,
        });
        Self::write(user_code, extractor.into(), run, description, MENTIONS)
    }

    fn write(
        user_code: &UserCode,
        code: Code,
        run: RunId,
        description: serde_json::Value,
        output: &'static str,
    ) -> Result<Self> {
        let dir = user_code.work_dir.join(format!(
            "run-{}-{}-{}",
            run,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(Error::io(&dir))?;
        // The directory is named in an environment variable and in SQL, so
        // it is made absolute once, here.
        let dir = std::fs::canonicalize(&dir).map_err(Error::io(&dir))?;
        let path = dir.join("task.json");
        let mut text =
            serde_json::to_string_pretty(&description).expect("a task serialises to JSON");
        text.push('\n');
        std::fs::write(&path, text).map_err(Error::io(&path))?;
        Ok(Self {
            command: user_code.command(code.clone(), Some(&dir))?,
            dir,
            run,
            code,
            user_code: user_code.clone(),
            output,
        })
    }

    pub(crate) fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    fn fail(&self, message: String) -> Error {
        self.code.fail(format!(
            "{message}. Its task is kept in {}",
            self.dir.display()
        ))
    }

    /// Runs the code and waits for it. On success the file it was to write
    /// is there to be read.
    ///
    /// The file is checked against the manifest once more, here, where it
    /// is started. A resolve checks every strategy before it starts the
    /// first, and the last may start long after: the run records the hash
    /// in the manifest, so that must be the file that runs.
    pub(crate) fn execute(&mut self) -> Result<()> {
        if let Err(error) = self.user_code.check(&self.code) {
            // Nothing ran, so there is nothing in the task to look at.
            let _ = std::fs::remove_dir_all(&self.dir);
            return Err(error);
        }
        let timeout = self.user_code.timeout;
        let log = |name: &str| -> Result<Stdio> {
            let path = self.dir.join(name);
            Ok(std::fs::File::create(&path)
                .map_err(Error::io(&path))?
                .into())
        };
        self.command.stdin(Stdio::null());
        self.command.stdout(log("stdout.log")?);
        self.command.stderr(log("stderr.log")?);

        let program = self.command.get_program().to_string_lossy().into_owned();
        // While the code runs, what it says can be asked for.
        let running = self.user_code.running.clone();
        running
            .lock_said()
            .insert(self.run, self.dir.join("stderr.log"));
        let this = self.run;
        let finished = run_command(&mut self.command, timeout, &|| running.is_cancelled(this));
        running.lock_said().remove(&self.run);
        let finished = finished.map_err(|error| {
            self.fail(format!(
                "`{program}`, which runs {} {}, could not be started: {error}",
                self.code.runtime,
                self.code.kind.plural()
            ))
        })?;
        match finished {
            Finished::Cancelled => {
                let _ = std::fs::remove_dir_all(&self.dir);
                Err(self.cancelled())
            }
            Finished::TimedOut => {
                let said = end_of(&self.dir.join("stderr.log"))
                    .ok()
                    .and_then(|end| last_line(&String::from_utf8_lossy(&end)))
                    .map(|line| format!(". The last it said was `{line}`"))
                    .unwrap_or_default();
                Err(self.fail(format!(
                    "it was stopped after {} without finishing. That is the longest a strategy \
                     or an extractor may run here, and `--task-timeout <seconds>` sets it, on \
                     `spool dev`, `spool serve` or `spool bench`{said}",
                    format_seconds(timeout)
                )))
            }
            Finished::Exited(status) if !status.success() => {
                let said = read_log_tail(&self.dir.join("stderr.log"));
                let said = match said.is_empty() {
                    true => String::new(),
                    false => format!(":\n{said}\n"),
                };
                Err(self.fail(format!("it failed with {status}{said}")))
            }
            Finished::Exited(_) if !self.path(self.output).is_file() => {
                Err(self.fail(format!("it finished without writing {}", self.output)))
            }
            Finished::Exited(_) => Ok(()),
        }
    }

    /// What the code wrote to standard error, as a terminal would show it:
    /// one entry for each line that is not blank, what a progress bar left
    /// last, and only the last lines. Read once the code has ended.
    pub(crate) fn said(&self) -> Vec<String> {
        const BYTES: u64 = 64 * 1024;
        let path = self.dir.join("stderr.log");
        let Ok(end) = last_bytes(&path, BYTES) else {
            return Vec::new();
        };
        let text = String::from_utf8_lossy(&end);
        // What follows the last line feed is a bar that never ended its
        // line, and what precedes the first is part of a line, when only
        // the end of the file was read.
        let Some((whole, _)) = text.rsplit_once('\n') else {
            return Vec::new();
        };
        let cut = std::fs::metadata(&path).is_ok_and(|file| file.len() > BYTES);
        let whole = match cut {
            true => whole.split_once('\n').map_or("", |(_, rest)| rest),
            false => whole,
        };
        let lines: Vec<String> = whole
            .lines()
            .filter_map(|line| {
                line.rsplit('\r')
                    .map(str::trim_end)
                    .find(|part| !part.trim().is_empty())
                    .map(|part| part.chars().take(SAID).collect())
            })
            .collect();
        lines[lines.len().saturating_sub(KEPT_LINES)..].to_vec()
    }

    /// The error of a run that was cancelled.
    pub(crate) fn cancelled(&self) -> Error {
        self.code
            .fail("it was cancelled, and nothing it found is kept".to_owned())
    }

    /// What went wrong with what the code wrote, as an error that says
    /// where the task is.
    pub(crate) fn rejected(&self, error: Error) -> Error {
        match error {
            Error::Judgements(message) => {
                self.fail(format!("its judgements were refused: {message}"))
            }
            Error::Mentions(message) => self.fail(format!("its mentions were refused: {message}")),
            Error::Duckdb(error) => {
                self.fail(format!("{} could not be read: {error}", self.output))
            }
            other => other,
        }
    }

    /// Removes the task's directory, once what it wrote is in the store.
    pub(crate) fn remove(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn format_seconds(duration: Duration) -> String {
    match duration.as_secs_f64() {
        whole if whole.fract() == 0.0 => format!("{whole} seconds"),
        other => format!("{other:.1} seconds"),
    }
}

/// The end of a file: its last 4,096 bytes, or all of it if it is shorter.
pub(super) fn end_of(path: &Path) -> std::io::Result<Vec<u8>> {
    last_bytes(path, 4096)
}

/// The last `bytes` of a file, or all of it if it is shorter.
fn last_bytes(path: &Path, bytes: u64) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(bytes)))?;
    let mut end = Vec::new();
    file.take(bytes).read_to_end(&mut end)?;
    Ok(end)
}

/// The last lines of a log, for an error message. Only the end of the file
/// is read: user code can write as much as the disk holds before it fails.
fn read_log_tail(path: &Path) -> String {
    const LINES: usize = 20;
    let Ok(end) = end_of(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&end);
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(LINES)..].join("\n")
}

pub(crate) enum Finished {
    Exited(ExitStatus),
    TimedOut,
    Cancelled,
}

/// Starts a command and waits for it, for at most `timeout` and until
/// `cancelled` says to stop. A command that runs out of time or is
/// cancelled is killed together with every process it started.
pub(crate) fn run_command(
    command: &mut Command,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> std::io::Result<Finished> {
    if cancelled() {
        return Ok(Finished::Cancelled);
    }
    // A group of its own, so that the processes it starts can be stopped
    // with it: `uv run` is the parent of the interpreter, not the interpreter.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    let mut child = command.spawn()?;
    let deadline = Instant::now() + timeout;
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Finished::Exited(status));
        }
        let stop = match (Instant::now() >= deadline, cancelled()) {
            (true, _) => Some(Finished::TimedOut),
            (false, true) => Some(Finished::Cancelled),
            (false, false) => None,
        };
        if let Some(stop) = stop {
            #[cfg(unix)]
            #[allow(unsafe_code)]
            {
                // SAFETY: `kill` takes no pointers. The group is the child's
                // own, and still exists: the child has not been waited for.
                unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
            }
            let _ = child.kill();
            let _ = child.wait();
            return Ok(stop);
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(50));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn a_command_that_finishes_reports_its_exit_status() {
        let Finished::Exited(status) =
            run_command(&mut shell("exit 0"), Duration::from_secs(30), &|| false).unwrap()
        else {
            panic!("the command had time to finish");
        };
        assert!(status.success());

        let Finished::Exited(status) =
            run_command(&mut shell("exit 3"), Duration::from_secs(30), &|| false).unwrap()
        else {
            panic!("the command had time to finish");
        };
        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn a_command_that_runs_out_of_time_is_stopped_with_what_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("still-running");
        // The shell starts a second process, which would write the marker
        // long after the timeout if it were left running.
        let script = format!("(sleep 2; touch '{}') & sleep 60", marker.display());
        let started = Instant::now();
        let finished =
            run_command(&mut shell(&script), Duration::from_millis(200), &|| false).unwrap();
        assert!(matches!(finished, Finished::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(2));

        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "the process it started outlived it");
    }

    #[test]
    fn a_command_that_is_cancelled_is_stopped_with_what_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("still-running");
        let script = format!("(sleep 2; touch '{}') & sleep 60", marker.display());
        let started = Instant::now();
        let asked = || started.elapsed() >= Duration::from_millis(200);
        let finished = run_command(&mut shell(&script), Duration::from_secs(30), &asked).unwrap();
        assert!(matches!(finished, Finished::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2));

        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "the process it started outlived it");

        // A command that was cancelled before it started is not started.
        let finished = run_command(
            &mut shell(&format!("touch '{}'", marker.display())),
            Duration::from_secs(30),
            &|| true,
        )
        .unwrap();
        assert!(matches!(finished, Finished::Cancelled));
        assert!(!marker.exists());
    }

    #[test]
    fn a_command_that_does_not_exist_is_an_error() {
        let mut command = Command::new("spool-no-such-command");
        assert!(run_command(&mut command, Duration::from_secs(1), &|| false).is_err());
    }

    #[test]
    fn the_tail_of_a_log_is_its_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stderr.log");
        let lines: Vec<String> = (1..=50).map(|line| format!("line {line}")).collect();
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let tail = read_log_tail(&path);
        assert!(tail.starts_with("line 31\n"));
        assert!(tail.ends_with("line 50"));
        assert_eq!(super::read_log_tail(&dir.path().join("missing")), "");

        // A long log is read from its end, and cut where the read begins.
        let lines: Vec<String> = (1..=100_000).map(|line| format!("line {line}")).collect();
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let tail = super::read_log_tail(&path);
        assert_eq!(tail.lines().count(), 20);
        assert!(tail.starts_with("line 99981\n"));
        assert!(tail.ends_with("line 100000"));
        std::fs::write(&path, "é".repeat(5000)).unwrap();
        assert!(super::read_log_tail(&path).len() <= 4096 + 4);
    }
}
