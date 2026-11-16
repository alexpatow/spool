//! User code: strategies that are files in the project, run as child
//! processes.
//!
//! The engine embeds no Python or JavaScript runtime. It writes a directory
//! of files, starts a command with `SPOOL_TASK_DIR` set to it, waits, and
//! reads the judgements the command left there:
//!
//! ```text
//! $SPOOL_TASK_DIR/
//! ├── task.json             the run, the entity type, the strategy, the feature columns
//! ├── candidates.parquet    left_id, right_id, and both sides' features as left_* and right_*
//! ├── features.parquet      observation_id, is_new, identity_id, and the feature columns
//! ├── judgements.parquet    written by the user code: left_id, right_id, verdict, score, detail
//! ├── stdout.log
//! └── stderr.log
//! ```
//!
//! The exit status is the result and standard error is the log. A task that
//! fails leaves its directory behind to be looked at; one that succeeds is
//! removed.
//!
//! A `.py` file runs with `uv run` and a `.ts` file with `bun run`. Each
//! finds a module that hides the files from the strategy's author: `spool`
//! in Python and `spool-strategy` in TypeScript. This crate carries both
//! and writes them out beside the tasks. The TypeScript one is built from
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
use crate::project::{FeatureColumn, Runtime, Strategy, strategy_hash};

/// The version of the task directory's layout. A helper refuses a task of
/// another version.
pub(crate) const PROTOCOL: u32 = 1;

const PYTHON_HELPER: &str = include_str!("usercode/spool.py");
const TYPESCRIPT_HELPER: &str = include_str!("usercode/spool.js");

pub(crate) const CANDIDATES: &str = "candidates.parquet";
pub(crate) const FEATURES: &str = "features.parquet";
pub(crate) const JUDGEMENTS: &str = "judgements.parquet";

/// Where the engine finds a project's user code, and how it runs it.
///
/// An engine that was not given this runs no user code: a manifest names
/// strategy files by their path from the project directory, and the engine
/// reads nothing but the manifest unless it is told where that directory is.
#[derive(Clone, Debug)]
pub struct UserCode {
    /// The project directory. Strategy files are found under it, and it is
    /// the working directory of the processes that run them.
    pub project_dir: PathBuf,
    /// Task directories are made here.
    pub work_dir: PathBuf,
    /// How long a strategy may run. One that takes longer is stopped, with
    /// everything it started, and its run fails.
    pub timeout: Duration,
    /// The `uv` command, which runs `.py` strategies.
    pub uv: OsString,
    /// The `bun` command, which runs `.ts` strategies.
    pub bun: OsString,
}

impl UserCode {
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

    /// User code of the project at `project_dir`, with tasks under the
    /// system's temporary directory, `uv` and `bun` as the search path finds
    /// them, and ten minutes for a strategy to finish.
    pub fn new(project_dir: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
            work_dir: std::env::temp_dir().join("spool-tasks"),
            timeout: Self::DEFAULT_TIMEOUT,
            uv: "uv".into(),
            bun: "bun".into(),
        }
    }

    /// The command that runs a strategy file. With `task` unset the file is
    /// run with nothing to do, which is how its dependencies are installed
    /// ahead of time.
    pub fn command(&self, strategy: &Strategy, task: Option<&Path>) -> Result<Command> {
        // The command runs in the project directory, so a path it is given
        // must not depend on where the engine was started.
        let absolute = |path: &Path| std::path::absolute(path).map_err(Error::io(path));
        let helpers = absolute(&self.helpers()?)?;
        let file = absolute(&self.file(strategy))?;
        let mut command = match strategy.runtime {
            Runtime::Python => {
                let mut command = Command::new(&self.uv);
                // The helper reads and writes Parquet with pyarrow, so it is
                // the launcher that asks for it, not every strategy file.
                command.args(["run", "--quiet", "--with", "pyarrow", "--script"]);
                command.arg(&file);
                command.env(
                    "PYTHONPATH",
                    search_path(&helpers.join("python"), "PYTHONPATH"),
                );
                // The project directory may be read-only, and compiled files
                // beside a strategy are not part of it.
                command.env("PYTHONDONTWRITEBYTECODE", "1");
                // Python puts the directory of the file it runs first among
                // the places it imports from, so `splink.py` would be what
                // `import splink` finds. This leaves that directory out.
                command.env("PYTHONSAFEPATH", "1");
                command.env("PYTHONUNBUFFERED", "1");
                command
            }
            Runtime::Typescript => {
                let mut command = Command::new(&self.bun);
                command.arg("run");
                command.arg(&file);
                command.env("NODE_PATH", search_path(&helpers.join("node"), "NODE_PATH"));
                command
            }
        };
        command.current_dir(&self.project_dir);
        // User code runs with the service's permissions, and that is no
        // reason to hand it the token that callers of the API present.
        command.env_remove("SPOOL_TOKEN");
        match task {
            Some(task) => command.env("SPOOL_TASK_DIR", absolute(task)?),
            None => command.env_remove("SPOOL_TASK_DIR"),
        };
        Ok(command)
    }

    /// Runs a strategy file once with nothing to do, so that `uv` or `bun`
    /// fetches what it depends on, and a file that cannot start fails now
    /// and not during a run.
    pub fn install(&self, strategy: &Strategy) -> Result<()> {
        self.check(strategy)?;
        let mut command = self.command(strategy, None)?;
        let program = command.get_program().to_string_lossy().into_owned();
        let fail = |message: String| Error::Strategy {
            strategy: strategy.name.clone(),
            message,
        };
        let output = command.stdin(Stdio::null()).output().map_err(|error| {
            fail(format!(
                "`{program}`, which runs {} strategies, could not be started: {error}",
                strategy.runtime
            ))
        })?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            return Err(fail(format!(
                "it does not start: {}:\n{}",
                output.status,
                said.trim_end()
            )));
        }
        Ok(())
    }

    fn file(&self, strategy: &Strategy) -> PathBuf {
        let mut path = self.project_dir.clone();
        path.extend(strategy.file.split('/'));
        path
    }

    /// Checks that a strategy's file is the one the manifest was compiled
    /// from. Only code that was built into the manifest runs: a file that
    /// changed since is refused until the project is built again.
    pub(crate) fn check(&self, strategy: &Strategy) -> Result<()> {
        let file = self.file(strategy);
        let fail = |message: String| Error::Strategy {
            strategy: strategy.name.clone(),
            message,
        };
        if !file.is_file() {
            return Err(fail(format!(
                "its file {} is not in the project directory {}",
                strategy.file,
                self.project_dir.display()
            )));
        }
        if strategy_hash(&file)? != strategy.hash {
            return Err(fail(format!(
                "{} or a file that belongs to it has changed since the manifest was compiled; \
                 build the project again and apply it",
                strategy.file
            )));
        }
        Ok(())
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
                    "the strategy helpers could not be written",
                )));
            }
        }
        Ok(dir)
    }
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

/// One run of a strategy: its directory and the command that works in it.
pub(crate) struct Task {
    pub dir: PathBuf,
    strategy: Strategy,
    command: Command,
    user_code: UserCode,
}

impl Task {
    /// Makes the task's directory and writes `task.json`. The Parquet files
    /// are the engine's to write, since only it reads the store.
    pub(crate) fn create(
        user_code: &UserCode,
        strategy: &Strategy,
        info: &TaskInfo,
    ) -> Result<Self> {
        let dir = user_code.work_dir.join(format!(
            "run-{}-{}-{}",
            info.run,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(Error::io(&dir))?;
        // The directory is named in an environment variable and in SQL, so
        // it is made absolute once, here.
        let dir = std::fs::canonicalize(&dir).map_err(Error::io(&dir))?;
        let description = json!({
            "protocol": PROTOCOL,
            "run": info.run,
            "entity_type": info.entity_type,
            "strategy": { "name": strategy.name, "version": strategy.hash },
            "declaration_version": info.declaration_version,
            "rematch": info.rematch,
            "features": info.features,
            "inputs": { "candidates": CANDIDATES, "features": FEATURES },
            "output": JUDGEMENTS,
        });
        let path = dir.join("task.json");
        let mut text =
            serde_json::to_string_pretty(&description).expect("a task serialises to JSON");
        text.push('\n');
        std::fs::write(&path, text).map_err(Error::io(&path))?;
        Ok(Self {
            command: user_code.command(strategy, Some(&dir))?,
            dir,
            strategy: strategy.clone(),
            user_code: user_code.clone(),
        })
    }

    pub(crate) fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    fn fail(&self, message: String) -> Error {
        Error::Strategy {
            strategy: self.strategy.name.clone(),
            message: format!("{message}. Its task is kept in {}", self.dir.display()),
        }
    }

    /// Runs the strategy and waits for it. On success `judgements.parquet`
    /// is there to be read.
    ///
    /// The file is checked against the manifest once more, here, where it
    /// is started. A resolve checks every strategy before it starts the
    /// first, and the last may start long after: the run records the hash
    /// in the manifest, so that must be the file that runs.
    pub(crate) fn execute(&mut self) -> Result<()> {
        if let Err(error) = self.user_code.check(&self.strategy) {
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
                "`{program}`, which runs {} strategies, could not be started: {error}",
                self.strategy.runtime
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
            Finished::Exited(_) if !self.path(JUDGEMENTS).is_file() => {
                Err(self.fail(format!("it finished without writing {JUDGEMENTS}")))
            }
            Finished::Exited(_) => Ok(()),
        }
    }

    /// What went wrong with the judgements the strategy wrote, as an error
    /// that says where the task is.
    pub(crate) fn rejected(&self, error: Error) -> Error {
        match error {
            Error::Judgements(message) => {
                self.fail(format!("its judgements were refused: {message}"))
            }
            Error::Duckdb(error) => self.fail(format!("{JUDGEMENTS} could not be read: {error}")),
            other => other,
        }
    }

    /// Removes the task's directory, once its judgements are in the store.
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
/// is read: a strategy can write as much as the disk holds before it fails.
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
        assert_eq!(command.get_program(), "uv");
        let file = dir
            .path()
            .join("project/entities/thing/strategies/model.py");
        assert_eq!(
            arguments(&command),
            [
                "run",
                "--quiet",
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
        assert!(set.contains(&("SPOOL_TOKEN".to_owned(), None)), "{set:?}");
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
        assert_eq!(command.get_program(), "bun");
        assert_eq!(arguments(&command)[0], "run");
        let set = environment(&command);
        // With nothing to do, it is told of no task.
        assert!(
            set.contains(&("SPOOL_TASK_DIR".to_owned(), None)),
            "{set:?}"
        );
        assert!(set.contains(&("SPOOL_TOKEN".to_owned(), None)));
        let (_, path) = set.iter().find(|(name, _)| name == "NODE_PATH").unwrap();
        let first = std::env::split_paths(path.as_deref().unwrap())
            .next()
            .unwrap();
        assert!(first.join("spool-strategy/index.js").is_file());
        assert!(first.join("spool-strategy/package.json").is_file());
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
