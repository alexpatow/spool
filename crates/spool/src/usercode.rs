//! User code: strategies and extractors that are files in the project, run
//! as child processes.
//!
//! The engine embeds no Python or JavaScript runtime. It writes a directory
//! of files, starts a command with `SPOOL_TASK_DIR` set to it, waits, and
//! reads what the command left there. A strategy is given candidate pairs
//! and writes judgements:
//!
//! ```text
//! $SPOOL_TASK_DIR/
//! ├── task.json             the run, the entity type, the strategy, the feature columns
//! ├── candidates.parquet    left_id, right_id, and both sides' features as left_* and right_*
//! ├── features.parquet      record_id, is_new, identity_id, and the feature columns
//! ├── judgements.parquet    written by the user code: left_id, right_id, verdict, score, detail
//! ├── stdout.log
//! └── stderr.log
//! ```
//!
//! An extractor is given documents and writes the mentions it found in them:
//!
//! ```text
//! $SPOOL_TASK_DIR/
//! ├── task.json             the run, the extractor, the entity types and what to look for
//! ├── documents.parquet     document_id, source, key, uri, text
//! ├── mentions.parquet      written by the user code: document_id, entity_type, start, end, attributes
//! ├── stdout.log
//! └── stderr.log
//! ```
//!
//! The exit status is the result and standard error is the log. A task that
//! fails leaves its directory behind to be looked at; one that succeeds is
//! removed.
//!
//! An extractor that is a pattern is not user code, and nothing here runs
//! it: the engine reads the documents itself.
//!
//! A `.py` file runs with `uv run` and a `.ts` file with `bun run`. Each
//! finds a module that hides the files from the code's author: `spool` in
//! Python and `spool-strategy` in TypeScript. This crate carries both and
//! writes them out beside the tasks. The TypeScript one is built from
//! `packages/strategy`.

use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use spool_core::{EntityType, RunId};

use crate::error::{Error, Result};
use crate::project::{Extractor, FeatureColumn, Runtime, Strategy, strategy_hash};

/// The version of the task directory's layout. A helper refuses a task of
/// another version.
pub(crate) const PROTOCOL: u32 = 1;

/// What the server's environment holds that user code is not handed: a
/// token of the API, and the secret of the server's client at Google.
const WITHHELD: [&str; 2] = ["SPOOL_TOKEN", "SPOOL_GOOGLE_CLIENT_SECRET"];

const PYTHON_HELPER: &str = include_str!("usercode/spool.py");
const TYPESCRIPT_HELPER: &str = include_str!("usercode/spool.js");

pub(crate) const CANDIDATES: &str = "candidates.parquet";
pub(crate) const FEATURES: &str = "features.parquet";
pub(crate) const JUDGEMENTS: &str = "judgements.parquet";
pub(crate) const DOCUMENTS: &str = "documents.parquet";
pub(crate) const MENTIONS: &str = "mentions.parquet";

/// One file of user code as the manifest names it: a strategy or an
/// extractor. Both are found, checked and started the same way.
#[derive(Clone, Debug)]
pub struct Code {
    kind: Kind,
    name: String,
    runtime: Runtime,
    file: String,
    hash: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Strategy,
    Extractor,
}

impl Kind {
    fn plural(self) -> &'static str {
        match self {
            Kind::Strategy => "strategies",
            Kind::Extractor => "extractors",
        }
    }
}

impl From<&Strategy> for Code {
    fn from(strategy: &Strategy) -> Self {
        Self {
            kind: Kind::Strategy,
            name: strategy.name.clone(),
            runtime: strategy.runtime,
            file: strategy.file.clone(),
            hash: strategy.hash.clone(),
        }
    }
}

impl From<&Extractor> for Code {
    fn from(extractor: &Extractor) -> Self {
        Self {
            kind: Kind::Extractor,
            name: extractor.name.clone(),
            runtime: extractor.runtime,
            file: extractor.file.clone(),
            hash: extractor.hash.clone(),
        }
    }
}

impl Code {
    fn fail(&self, message: String) -> Error {
        match self.kind {
            Kind::Strategy => Error::Strategy {
                strategy: self.name.clone(),
                message,
            },
            Kind::Extractor => Error::Extractor {
                extractor: self.name.clone(),
                message,
            },
        }
    }
}

/// Where the engine finds a project's user code, and how it runs it.
///
/// An engine that was not given this runs no user code: a manifest names
/// the files of strategies and extractors by their path from the project
/// directory, and the engine reads nothing but the manifest unless it is
/// told where that directory is.
#[derive(Clone, Debug)]
pub struct UserCode {
    /// The project directory. The files of user code are found under it,
    /// and it is the working directory of the processes that run them.
    pub project_dir: PathBuf,
    /// Task directories are made here.
    pub work_dir: PathBuf,
    /// How long a strategy or an extractor may run. One that takes longer
    /// is stopped, with everything it started, and its run fails.
    pub timeout: Duration,
    /// The `uv` command, which runs `.py` files.
    pub uv: OsString,
    /// The `bun` command, which runs `.ts` files.
    pub bun: OsString,
}

impl UserCode {
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

    /// User code of the project at `project_dir`, with tasks under the
    /// system's temporary directory, `uv` and `bun` as the search path finds
    /// them, and ten minutes for a strategy or an extractor to finish.
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
            work_dir: std::env::temp_dir().join("spool-tasks"),
            timeout: Self::DEFAULT_TIMEOUT,
            uv: "uv".into(),
            bun: "bun".into(),
        }
    }

    /// The command that runs a strategy's or an extractor's file. With
    /// `task` unset the file is run with nothing to do, which is how its
    /// dependencies are installed ahead of time.
    pub fn command(&self, code: impl Into<Code>, task: Option<&Path>) -> Result<Command> {
        let code = code.into();
        // The command runs in the project directory, so a path it is given
        // must not depend on where the engine was started.
        let absolute = |path: &Path| std::path::absolute(path).map_err(Error::io(path));
        let helpers = absolute(&self.helpers()?)?;
        let file = absolute(&self.file(&code))?;
        let mut command = match code.runtime {
            Runtime::Python => {
                let mut command = Command::new(self.program(code.runtime));
                // The helper reads and writes Parquet with pyarrow, so it is
                // the launcher that asks for it, not every file of user code.
                // `--isolated` keeps the packages of the system's Python out:
                // without it `uv` runs a file that names no dependencies in
                // that Python as it is, and pyarrow fails beside a NumPy
                // older than it was built for.
                command.args([
                    "run",
                    "--quiet",
                    "--isolated",
                    "--with",
                    "pyarrow",
                    "--script",
                ]);
                command.arg(&file);
                command.env(
                    "PYTHONPATH",
                    search_path(&helpers.join("python"), "PYTHONPATH"),
                );
                // The project directory may be read-only, and compiled files
                // beside the code are not part of it.
                command.env("PYTHONDONTWRITEBYTECODE", "1");
                // Python puts the directory of the file it runs first among
                // the places it imports from, so `splink.py` would be what
                // `import splink` finds. This leaves that directory out.
                command.env("PYTHONSAFEPATH", "1");
                command.env("PYTHONUNBUFFERED", "1");
                command
            }
            Runtime::Typescript => {
                let mut command = Command::new(self.program(code.runtime));
                command.arg("run");
                command.arg(&file);
                command.env("NODE_PATH", search_path(&helpers.join("node"), "NODE_PATH"));
                command
            }
            Runtime::Pattern => {
                return Err(code.fail(
                    "it is a pattern, which the engine runs itself: no command runs it".to_owned(),
                ));
            }
        };
        command.current_dir(&self.project_dir);
        // User code runs with the service's permissions, and that is no
        // reason to hand it a token this process was started with, or what
        // makes the server a client of Google's.
        for secret in WITHHELD {
            command.env_remove(secret);
        }
        match task {
            Some(task) => command.env("SPOOL_TASK_DIR", absolute(task)?),
            None => command.env_remove("SPOOL_TASK_DIR"),
        };
        Ok(command)
    }

    /// Runs a strategy's or an extractor's file once with nothing to do, so
    /// that `uv` or `bun` fetches what it depends on, and a file that cannot
    /// start fails now and not during a run.
    pub fn install(&self, code: impl Into<Code>) -> Result<()> {
        let code = code.into();
        self.check(&code)?;
        let mut command = self.command(code.clone(), None)?;
        let program = command.get_program().to_string_lossy().into_owned();
        let output = command.stdin(Stdio::null()).output().map_err(|error| {
            code.fail(format!(
                "`{program}`, which runs {} {}, could not be started: {error}",
                code.runtime,
                code.kind.plural()
            ))
        })?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            return Err(code.fail(format!(
                "it does not start: {}:\n{}",
                output.status,
                said.trim_end()
            )));
        }
        Ok(())
    }

    fn file(&self, code: &Code) -> PathBuf {
        let mut path = self.project_dir.clone();
        path.extend(code.file.split('/'));
        path
    }

    /// The command that runs files of a runtime: the one this was given if
    /// the search path has it, and otherwise where its installer puts it.
    /// Someone who installs `uv` because a run said so has a server whose
    /// search path is as it was, and the next run finds it all the same.
    fn program(&self, runtime: Runtime) -> OsString {
        let (named, installed): (&OsString, &[&str]) = match runtime {
            Runtime::Python => (&self.uv, &[".local/bin", ".cargo/bin"]),
            Runtime::Typescript => (&self.bun, &[".bun/bin"]),
            Runtime::Pattern => return OsString::new(),
        };
        if found(named) {
            return named.clone();
        }
        let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
        // Only the command's own name is looked for elsewhere: a path that
        // was given names one file.
        let plain = Path::new(named).components().count() == 1;
        home.filter(|_| plain)
            .and_then(|home| {
                installed
                    .iter()
                    .map(|dir| Path::new(&home).join(dir).join(named))
                    .find(|path| executable(path))
            })
            .map_or_else(|| named.clone(), PathBuf::into_os_string)
    }

    /// Why nothing here can run the code, if nothing can: what runs its
    /// language is not installed. `None` for code that can be started.
    fn cannot_run(&self, code: &Code) -> Option<String> {
        let (language, program, install, variable) = match code.runtime {
            Runtime::Python => (
                "Python",
                "uv",
                "https://docs.astral.sh/uv/getting-started/installation/",
                "SPOOL_UV",
            ),
            Runtime::Typescript => (
                "TypeScript",
                "bun",
                "https://bun.sh/docs/installation",
                "SPOOL_BUN",
            ),
            Runtime::Pattern => return None,
        };
        let named = self.program(code.runtime);
        if found(&named) {
            return None;
        }
        let looked = match named == *program {
            true => String::new(),
            false => format!(" It was looked for as `{}`.", named.to_string_lossy()),
        };
        Some(format!(
            "it is a {language} file, and Spool runs those with `{program}`, which is not \
             installed where Spool runs.{looked} Install {program} there ({install}) and try \
             again. If it is installed where the search path does not reach, set {variable} to \
             it where Spool is started"
        ))
    }

    /// Checks that the file is the one the manifest was compiled from, and
    /// that what runs it is installed. Only code that was built into the
    /// manifest runs: a file that changed since is refused until the
    /// project is built again.
    pub(crate) fn check(&self, code: &Code) -> Result<()> {
        let file = self.file(code);
        if !file.is_file() {
            return Err(code.fail(format!(
                "its file {} is not in the project directory {}",
                code.file,
                self.project_dir.display()
            )));
        }
        if strategy_hash(&file)? != code.hash {
            return Err(code.fail(format!(
                "{} or a file that belongs to it has changed since the manifest was compiled; \
                 build the project again and apply it",
                code.file
            )));
        }
        match self.cannot_run(code) {
            Some(why) => Err(code.fail(why)),
            None => Ok(()),
        }
    }

    /// Writes the helper modules beside the tasks, once per version of them,
    /// and returns their directory.
    fn helpers(&self) -> Result<PathBuf> {
        let version = crate::project::hash_text(&[PYTHON_HELPER, TYPESCRIPT_HELPER]);
        let dir = self.work_dir.join(format!("helpers-{}", &version[..12]));
        if dir.is_dir() {
            return Ok(dir);
        }
        // Written under another name and then renamed, so that two engines
        // starting at once never see half of it.
        let partial = self.work_dir.join(format!(
            "helpers-{}.{}-{}",
            &version[..12],
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let write = |path: PathBuf, contents: &str| -> Result<()> {
            let parent = path.parent().expect("a helper file has a directory");
            std::fs::create_dir_all(parent).map_err(Error::io(parent))?;
            std::fs::write(&path, contents).map_err(Error::io(&path))
        };
        write(partial.join("python/spool/__init__.py"), PYTHON_HELPER)?;
        write(
            partial.join("node/spool-strategy/index.js"),
            TYPESCRIPT_HELPER,
        )?;
        write(
            partial.join("node/spool-strategy/package.json"),
            "{ \"name\": \"spool-strategy\", \"type\": \"module\", \"main\": \"index.js\" }\n",
        )?;
        if std::fs::rename(&partial, &dir).is_err() {
            // Someone else got there first, with the same files.
            let _ = std::fs::remove_dir_all(&partial);
            if !dir.is_dir() {
                return Err(Error::io(&dir)(std::io::Error::other(
                    "the user-code helpers could not be written",
                )));
            }
        }
        Ok(dir)
    }
}

/// Whether a file is there and may be executed.
fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    let may = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o111 != 0;
    #[cfg(not(unix))]
    let may = true;
    metadata.is_file() && may
}

/// Whether a command can be started: a path names an executable file, and a
/// plain name is one that a directory of the search path has.
fn found(program: &OsString) -> bool {
    let path = Path::new(program);
    if path.components().count() != 1 {
        return executable(path);
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| executable(&dir.join(path))))
}

/// `dir`, followed by what the variable already holds.
fn search_path(dir: &Path, variable: &str) -> OsString {
    let mut paths = vec![dir.to_owned()];
    if let Some(existing) = std::env::var_os(variable) {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).unwrap_or_else(|_| dir.as_os_str().to_owned())
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

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
        let finished = run(&mut self.command, timeout).map_err(|error| {
            self.fail(format!(
                "`{program}`, which runs {} {}, could not be started: {error}",
                self.code.runtime,
                self.code.kind.plural()
            ))
        })?;
        match finished {
            Finished::TimedOut => Err(self.fail(format!(
                "it was stopped after {} without finishing",
                seconds(timeout)
            ))),
            Finished::Exited(status) if !status.success() => {
                let said = tail(&self.dir.join("stderr.log"));
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

fn seconds(duration: Duration) -> String {
    match duration.as_secs_f64() {
        whole if whole.fract() == 0.0 => format!("{whole} seconds"),
        other => format!("{other:.1} seconds"),
    }
}

/// The last lines of a log, for an error message. Only the end of the file
/// is read: user code can write as much as the disk holds before it fails.
fn tail(path: &Path) -> String {
    const BYTES: u64 = 4096;
    const LINES: usize = 20;
    let read = || -> std::io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        file.seek(SeekFrom::Start(length.saturating_sub(BYTES)))?;
        let mut end = Vec::new();
        file.take(BYTES).read_to_end(&mut end)?;
        Ok(end)
    };
    let Ok(end) = read() else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&end);
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(LINES)..].join("\n")
}

pub(crate) enum Finished {
    Exited(ExitStatus),
    TimedOut,
}

/// Starts a command and waits for it, for at most `timeout`. A command that
/// runs out of time is killed together with every process it started.
pub(crate) fn run(command: &mut Command, timeout: Duration) -> std::io::Result<Finished> {
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
        if Instant::now() >= deadline {
            #[cfg(unix)]
            {
                // SAFETY: `kill` takes no pointers. The group is the child's
                // own, and still exists: the child has not been waited for.
                unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
            }
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Finished::TimedOut);
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(50));
    }
}

/// A path as a SQL string literal.
pub(crate) fn sql_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
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
        let Finished::Exited(status) = run(&mut shell("exit 0"), Duration::from_secs(30)).unwrap()
        else {
            panic!("the command had time to finish");
        };
        assert!(status.success());

        let Finished::Exited(status) = run(&mut shell("exit 3"), Duration::from_secs(30)).unwrap()
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
        let finished = run(&mut shell(&script), Duration::from_millis(200)).unwrap();
        assert!(matches!(finished, Finished::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(2));

        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "the process it started outlived it");
    }

    #[test]
    fn a_strategy_is_started_with_its_task_and_without_the_servers_token() {
        let dir = tempfile::tempdir().unwrap();
        let user_code = UserCode {
            work_dir: dir.path().join("work"),
            ..UserCode::new(dir.path().join("project"))
        };
        let strategy = |runtime, file: &str| Strategy {
            name: "model".to_owned(),
            runtime,
            file: file.to_owned(),
            hash: String::new(),
        };
        let environment = |command: &Command| -> Vec<(String, Option<String>)> {
            command
                .get_envs()
                .map(|(name, value)| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.map(|value| value.to_string_lossy().into_owned()),
                    )
                })
                .collect()
        };
        let arguments = |command: &Command| -> Vec<String> {
            command
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect()
        };
        let task = dir.path().join("work/run-1");

        let python = strategy(Runtime::Python, "entities/thing/strategies/model.py");
        let command = user_code.command(&python, Some(&task)).unwrap();
        // `uv` as the search path has it, or as its installer left it.
        assert_eq!(Path::new(command.get_program()).file_name().unwrap(), "uv");
        let file = dir
            .path()
            .join("project/entities/thing/strategies/model.py");
        assert_eq!(
            arguments(&command),
            [
                "run",
                "--quiet",
                "--isolated",
                "--with",
                "pyarrow",
                "--script",
                file.to_str().unwrap()
            ]
        );
        assert_eq!(
            command.get_current_dir(),
            Some(dir.path().join("project").as_path())
        );
        let set = environment(&command);
        for secret in WITHHELD {
            assert!(set.contains(&(secret.to_owned(), None)), "{set:?}");
        }
        assert!(set.contains(&(
            "SPOOL_TASK_DIR".to_owned(),
            Some(task.to_string_lossy().into_owned())
        )));
        // The helper module is where Python looks first.
        let (_, path) = set.iter().find(|(name, _)| name == "PYTHONPATH").unwrap();
        let first = std::env::split_paths(path.as_deref().unwrap())
            .next()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(first.join("spool/__init__.py")).unwrap(),
            PYTHON_HELPER
        );

        // Paths that are relative to where the engine runs still name the
        // same files from the project directory, where the command runs.
        let here = tempfile::tempdir_in(".").unwrap();
        let here = here
            .path()
            .strip_prefix(std::env::current_dir().unwrap())
            .unwrap();
        assert!(here.is_relative());
        let relative = UserCode {
            work_dir: here.join("work"),
            ..UserCode::new(here.join("project"))
        };
        let command = relative
            .command(&python, Some(&here.join("work/run-1")))
            .unwrap();
        let given = arguments(&command);
        assert_eq!(
            Path::new(given.last().unwrap()),
            std::path::absolute(here.join("project/entities/thing/strategies/model.py")).unwrap()
        );
        let set = environment(&command);
        for name in ["SPOOL_TASK_DIR", "PYTHONPATH"] {
            let (_, value) = set.iter().find(|(set, _)| set == name).unwrap();
            let first = std::env::split_paths(value.as_deref().unwrap())
                .next()
                .unwrap();
            assert!(first.is_absolute(), "{name}: {first:?}");
        }
        // A strategy may be named after the library it wraps: the directory
        // of the file is not where Python looks for modules.
        assert!(set.contains(&("PYTHONSAFEPATH".to_owned(), Some("1".to_owned()))));

        let typescript = strategy(Runtime::Typescript, "entities/thing/strategies/model.ts");
        let command = user_code.command(&typescript, None).unwrap();
        assert_eq!(Path::new(command.get_program()).file_name().unwrap(), "bun");
        assert_eq!(arguments(&command)[0], "run");
        let set = environment(&command);
        // With nothing to do, it is told of no task.
        assert!(
            set.contains(&("SPOOL_TASK_DIR".to_owned(), None)),
            "{set:?}"
        );
        for secret in WITHHELD {
            assert!(set.contains(&(secret.to_owned(), None)), "{set:?}");
        }
        let (_, path) = set.iter().find(|(name, _)| name == "NODE_PATH").unwrap();
        let first = std::env::split_paths(path.as_deref().unwrap())
            .next()
            .unwrap();
        assert!(first.join("spool-strategy/index.js").is_file());
        assert!(first.join("spool-strategy/package.json").is_file());
    }

    #[test]
    fn code_whose_language_nothing_here_runs_says_what_to_install() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join("extractors")).unwrap();
        let code = |runtime, file: &str| {
            std::fs::write(project.join(file), "").unwrap();
            Code::from(&Extractor {
                name: "names".to_owned(),
                runtime,
                file: file.to_owned(),
                hash: strategy_hash(&project.join(file)).unwrap(),
                pattern: None,
            })
        };
        let python = code(Runtime::Python, "extractors/names.py");
        let typescript = code(Runtime::Typescript, "extractors/names.ts");
        // Neither command is anywhere: a path names one file, and is looked
        // for nowhere else.
        let nowhere = UserCode {
            work_dir: dir.path().join("work"),
            uv: dir.path().join("bin/uv").into(),
            bun: dir.path().join("bin/bun").into(),
            ..UserCode::new(&project)
        };
        let message = nowhere.check(&python).unwrap_err().to_string();
        for part in [
            "extractor `names`: it is a Python file, and Spool runs those with `uv`, which is \
             not installed where Spool runs.",
            "Install uv there (https://docs.astral.sh/uv/getting-started/installation/) and try \
             again.",
            "set SPOOL_UV to it where Spool is started",
        ] {
            assert!(message.contains(part), "no `{part}` in: {message}");
        }
        let message = nowhere.check(&typescript).unwrap_err().to_string();
        for part in [
            "it is a TypeScript file, and Spool runs those with `bun`, which is not installed \
             where Spool runs.",
            "Install bun there (https://bun.sh/docs/installation) and try again.",
            "SPOOL_BUN",
        ] {
            assert!(message.contains(part), "no `{part}` in: {message}");
        }
        // The same when it is asked to fetch what the code depends on.
        let message = nowhere.install(python.clone()).unwrap_err().to_string();
        assert!(message.contains("Install uv"), "{message}");

        // A file that is there and cannot be executed is not the command.
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin/uv"), "#!/bin/sh\n").unwrap();
        assert!(nowhere.check(&python).is_err());
        let mut permissions = std::fs::metadata(dir.path().join("bin/uv"))
            .unwrap()
            .permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(dir.path().join("bin/uv"), permissions).unwrap();
        nowhere.check(&python).unwrap();

        // A pattern is run by the engine, and needs neither.
        let pattern = code(Runtime::Pattern, "extractors/codes.yaml");
        nowhere.check(&pattern).unwrap();
        assert!(nowhere.command(pattern, None).is_err());
    }

    #[test]
    fn a_command_that_does_not_exist_is_an_error() {
        let mut command = Command::new("spool-no-such-command");
        assert!(run(&mut command, Duration::from_secs(1)).is_err());
    }

    #[test]
    fn the_tail_of_a_log_is_its_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stderr.log");
        let lines: Vec<String> = (1..=50).map(|line| format!("line {line}")).collect();
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let tail = tail(&path);
        assert!(tail.starts_with("line 31\n"));
        assert!(tail.ends_with("line 50"));
        assert_eq!(super::tail(&dir.path().join("missing")), "");

        // A long log is read from its end, and cut where the read begins.
        let lines: Vec<String> = (1..=100_000).map(|line| format!("line {line}")).collect();
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let tail = super::tail(&path);
        assert_eq!(tail.lines().count(), 20);
        assert!(tail.starts_with("line 99981\n"));
        assert!(tail.ends_with("line 100000"));
        std::fs::write(&path, "é".repeat(5000)).unwrap();
        assert!(super::tail(&path).len() <= 4096 + 4);
    }
}
