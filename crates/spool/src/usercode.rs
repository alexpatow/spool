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
//! A `.py` file runs with `uv run` and a `.ts` file with `bun run`. Each
//! finds a module that hides the files from the code's author: `spool` in
//! Python and `spool-strategy` in TypeScript. This crate carries both and
//! writes them out beside the tasks. The TypeScript one is built from
//! `packages/strategy`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use spool_core::RunId;

use crate::error::{Error, Result};
use crate::project::{Extractor, Runtime, Strategy, strategy_hash};

mod env;
mod task;

use env::search_path;
use task::end_of;
pub(crate) use task::{
    CANDIDATES, DOCUMENTS, ExtractType, FAILED, FEATURES, JUDGEMENTS, MENTIONS, Task, TaskInfo,
};

/// What the server's environment holds that user code is not handed: a
/// token of the API, and the secret of the server's client at Google.
const WITHHELD: [&str; 2] = ["SPOOL_TOKEN", "SPOOL_GOOGLE_CLIENT_SECRET"];

const PYTHON_HELPER: &str = include_str!("../assets/usercode/spool.py");
const TYPESCRIPT_HELPER: &str = include_str!("../assets/usercode/spool.js");

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
    /// The runs whose code is running now. Copies of this share it.
    pub running: Running,
}

/// The runs whose code is running now, where each writes what it says, and
/// the runs whose code is to be stopped.
#[derive(Clone, Debug, Default)]
pub struct Running {
    said: Arc<Mutex<HashMap<RunId, PathBuf>>>,
    control: Arc<Mutex<Control>>,
}

/// Which runs were cancelled, and which are storing what their code found
/// and can no longer be. One lock holds both, so that a cancellation and
/// the start of storing cannot both succeed.
#[derive(Debug, Default)]
struct Control {
    cancelled: HashSet<RunId>,
    storing: HashSet<RunId>,
}

impl Running {
    fn lock_said(&self) -> std::sync::MutexGuard<'_, HashMap<RunId, PathBuf>> {
        self.said.lock().unwrap_or_else(|held| held.into_inner())
    }

    fn lock_control(&self) -> std::sync::MutexGuard<'_, Control> {
        self.control.lock().unwrap_or_else(|held| held.into_inner())
    }

    fn is_cancelled(&self, run: RunId) -> bool {
        self.lock_control().cancelled.contains(&run)
    }

    /// Asks for the code of a run to be stopped, with every process it
    /// started, as one that runs out of time is. Code that has not started
    /// yet is not started, and what code that has finished found is not
    /// stored. The run then fails. `false` when the run has begun to store
    /// what its code found, which is then too late.
    pub fn cancel(&self, run: RunId) -> bool {
        let mut control = self.lock_control();
        if control.storing.contains(&run) {
            return false;
        }
        control.cancelled.insert(run);
        true
    }

    /// Marks a run as storing what its code found, after which it can no
    /// longer be cancelled. `false` when it was cancelled first, and is to
    /// store nothing.
    pub fn begin_storing(&self, run: RunId) -> bool {
        let mut control = self.lock_control();
        if control.cancelled.contains(&run) {
            return false;
        }
        control.storing.insert(run);
        true
    }

    /// Forgets a run's cancellation and storing, once it has ended.
    pub fn forget(&self, run: RunId) {
        let mut control = self.lock_control();
        control.cancelled.remove(&run);
        control.storing.remove(&run);
    }

    /// The last whole line the code of a run has written to its standard
    /// error, while it runs: `None` for a run whose code is not running,
    /// and for code that has finished no line yet.
    ///
    /// A progress bar writes over itself with carriage returns and ends its
    /// line when it is done, so one that is still moving is not a line yet,
    /// and what was said before it stands. Only the end of what was written
    /// is read.
    pub fn said(&self, run: RunId) -> Option<String> {
        let log = self.lock_said().get(&run).cloned()?;
        last_line(&String::from_utf8_lossy(&end_of(&log).ok()?))
    }
}

/// The last line of a text that is ended by a line feed and is not blank,
/// as a terminal would show it: what follows its last carriage return.
fn last_line(text: &str) -> Option<String> {
    let (whole, _) = text.rsplit_once('\n')?;
    let line = whole
        .lines()
        .filter_map(|line| {
            line.rsplit('\r')
                .map(str::trim)
                .find(|part| !part.is_empty())
        })
        .next_back()?;
    // A line this long is cut to what a terminal has room for.
    Some(line.chars().take(SAID).collect())
}

/// The most that is passed on of a line that user code wrote.
const SAID: usize = 200;

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
            running: Running::default(),
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
                // `uv` is left to say what it downloads: the first run of a
                // file can take minutes, and that is all there is to see.
                command.arg("run");
                // `--isolated` keeps the packages of the system's Python out:
                // without it `uv` runs a file that names no dependencies in
                // that Python as it is, and pyarrow fails beside a NumPy
                // older than it was built for. A file that names its
                // dependencies runs apart in any case, and `uv` warns that
                // the option does nothing.
                if !names_dependencies(&file) {
                    command.arg("--isolated");
                }
                // The helper reads and writes Parquet with pyarrow, so it is
                // the launcher that asks for it, not every file of user code.
                command.args(["--with", "pyarrow", "--script"]);
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
        };
        // What the project's `.env` sets, under what this process was
        // started with and what is set above for the code.
        let set: Vec<OsString> = command
            .get_envs()
            .map(|(name, _)| name.to_owned())
            .collect();
        for (name, value) in self.project_variables()? {
            let named = |set: &OsString| set.as_os_str() == std::ffi::OsStr::new(&name);
            if std::env::var_os(&name).is_none() && !set.iter().any(named) {
                command.env(name, value);
            }
        }
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

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Whether a Python file names what it depends on, in a block of comments
/// that begins `# /// script`, which is how `uv` reads it.
fn names_dependencies(file: &Path) -> bool {
    std::fs::read_to_string(file)
        .is_ok_and(|text| text.lines().any(|line| line.trim_end() == "# /// script"))
}

/// A path as a SQL string literal.
pub(crate) fn sql_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_cancelled_or_stores_what_it_found_and_never_both() {
        let running = Running::default();
        // Cancelled first: what the code found is not stored.
        assert!(running.cancel(RunId(1)));
        assert!(!running.begin_storing(RunId(1)));
        // Storing first: it is too late to cancel.
        assert!(running.begin_storing(RunId(2)));
        assert!(!running.cancel(RunId(2)));
        // A run that ended is forgotten.
        running.forget(RunId(1));
        running.forget(RunId(2));
        assert!(running.begin_storing(RunId(1)));
        assert!(running.cancel(RunId(2)));
    }

    fn strategy(runtime: Runtime, file: &str) -> Strategy {
        Strategy {
            name: "model".to_owned(),
            runtime,
            file: file.to_owned(),
            hash: String::new(),
        }
    }

    fn environment(command: &Command) -> Vec<(String, Option<String>)> {
        command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    fn arguments(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    /// User code for a project in `dir`, with its tasks under `dir/work`.
    fn user_code_in(dir: &Path) -> UserCode {
        UserCode {
            work_dir: dir.join("work"),
            ..UserCode::new(dir.join("project"))
        }
    }

    #[test]
    fn a_python_strategy_is_started_with_its_task_and_without_the_servers_token() {
        let dir = tempfile::tempdir().unwrap();
        let user_code = user_code_in(dir.path());
        let task = dir.path().join("work/run-1");

        let python = strategy(Runtime::Python, "entities/thing/strategies/model.py");
        let command = user_code.command(&python, Some(&task)).unwrap();
        // `uv` as the search path has it, or as its installer left it.
        assert_eq!(Path::new(command.get_program()).file_name().unwrap(), "uv");
        let file = dir
            .path()
            .join("project/entities/thing/strategies/model.py");
        // `uv` is not told to be quiet: what it downloads is all there is
        // to see of a first run.
        assert_eq!(
            arguments(&command),
            [
                "run",
                "--isolated",
                "--with",
                "pyarrow",
                "--script",
                file.to_str().unwrap()
            ]
        );
        // A file that names what it depends on runs apart in any case, and
        // `uv` warns about an option that does nothing.
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "# /// script\n# dependencies = [\"numpy\"]\n# ///\nimport numpy\n",
        )
        .unwrap();
        let command = user_code.command(&python, Some(&task)).unwrap();
        assert_eq!(
            arguments(&command),
            [
                "run",
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
    }

    #[test]
    fn a_strategy_started_from_relative_paths_is_given_absolute_ones() {
        let python = strategy(Runtime::Python, "entities/thing/strategies/model.py");
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
    }

    #[test]
    fn a_typescript_strategy_is_started_without_the_servers_token() {
        let dir = tempfile::tempdir().unwrap();
        let user_code = user_code_in(dir.path());
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
    fn what_running_code_said_last_is_its_last_whole_line() {
        assert_eq!(last_line(""), None);
        assert_eq!(last_line("not a line yet"), None);
        assert_eq!(last_line("one\n").as_deref(), Some("one"));
        assert_eq!(last_line("one\ntwo\n\n  \n").as_deref(), Some("two"));
        // A line that is not ended yet is not said yet.
        assert_eq!(last_line("one\ntwo").as_deref(), Some("one"));
        // A progress bar writes over itself until it is done. While it
        // moves, what was said before it stands, and when it is done it is
        // a line as a terminal shows it.
        let bar = "Fetching the model, once.\n\rFetching:   0%\rFetching:  40%";
        assert_eq!(last_line(bar).as_deref(), Some("Fetching the model, once."));
        let done = format!("{bar}\rFetching: 100%\n");
        assert_eq!(last_line(&done).as_deref(), Some("Fetching: 100%"));
        // A line that no terminal has room for is cut.
        let long = "é".repeat(1000) + "\n";
        assert_eq!(last_line(&long).unwrap().chars().count(), SAID);

        // Only a run whose code is running says anything.
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("stderr.log");
        let running = Running::default();
        assert_eq!(running.said(RunId(7)), None);
        running.lock_said().insert(RunId(7), log.clone());
        // Its log may not be there yet, or be empty.
        assert_eq!(running.said(RunId(7)), None);
        std::fs::write(&log, "Downloading pyarrow (52.5MiB)\n Downloaded pyarrow\n").unwrap();
        assert_eq!(
            running.said(RunId(7)).as_deref(),
            Some("Downloaded pyarrow")
        );
        assert_eq!(running.said(RunId(8)), None);
        // A copy of the user code shares what is running.
        let user_code = UserCode {
            running: running.clone(),
            ..UserCode::new(dir.path())
        };
        assert_eq!(
            user_code.clone().running.said(RunId(7)).as_deref(),
            Some("Downloaded pyarrow")
        );
        running.lock_said().remove(&RunId(7));
        assert_eq!(user_code.running.said(RunId(7)), None);
    }
}
