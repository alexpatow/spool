//! What a project's code is started with: the variables its `.env` sets,
//! and the commands that run it, as the search path or their installers
//! have them.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{Code, UserCode};
use crate::error::{Error, Result};
use crate::project::Runtime;

impl UserCode {
    /// The variables the project's `.env` sets, in its order: none if it
    /// has no `.env`. It is read each time code is started, so a change to
    /// it needs no restart. A line that is wrong is named by its number
    /// alone, since what it holds may be a secret.
    pub fn project_variables(&self) -> Result<Vec<(String, String)>> {
        let path = self.project_dir.join(ENV_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(Error::io(&path)(error)),
        };
        parse_env(&text).map_err(|line| Error::Project {
            file: path,
            message: match line {
                0 => "a line is not `NAME=value`, a comment that starts with `#`, or blank"
                    .to_owned(),
                line => format!(
                    "line {line} is not `NAME=value`, a comment that starts with `#`, or blank"
                ),
            },
        })
    }

    /// The command that runs files of a runtime: the one this was given if
    /// the search path has it, and otherwise where its installer puts it.
    /// Someone who installs `uv` because a run said so has a server whose
    /// search path is as it was, and the next run finds it all the same.
    pub(super) fn program(&self, runtime: Runtime) -> OsString {
        let (named, installed): (&OsString, &[&str]) = match runtime {
            Runtime::Python => (&self.uv, &[".local/bin", ".cargo/bin"]),
            Runtime::Typescript => (&self.bun, &[".bun/bin"]),
        };
        if command_exists(named) {
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
                    .find(|path| is_executable(path))
            })
            .map_or_else(|| named.clone(), PathBuf::into_os_string)
    }

    /// Whether what runs files of a runtime is installed here.
    pub fn installed(&self, runtime: Runtime) -> bool {
        command_exists(&self.program(runtime))
    }

    /// Why nothing here can run the code, if nothing can: what runs its
    /// language is not installed. `None` for code that can be started.
    pub(super) fn cannot_run(&self, code: &Code) -> Option<String> {
        if self.installed(code.runtime) {
            return None;
        }
        let (language, program, variable) = match code.runtime {
            Runtime::Python => ("Python", "uv", "SPOOL_UV"),
            Runtime::Typescript => ("TypeScript", "bun", "SPOOL_BUN"),
        };
        let named = self.program(code.runtime);
        let looked = match named == *program {
            true => String::new(),
            false => format!(" It was looked for as `{}`.", named.to_string_lossy()),
        };
        Some(format!(
            "it is a {language} file, and Spool runs those with `{program}`, which is not \
             installed where Spool runs.{looked} Install it there and run this again:\n\n  \
             {}\n\nSpool finds it where that puts it, with no restart. Other ways to install \
             it: {}. If it is installed where the search path does not reach, set {variable} \
             to it where Spool is started",
            code.runtime.install_command(),
            code.runtime.install_page(),
        ))
    }
}

/// Whether a file is there and may be executed.
fn is_executable(path: &Path) -> bool {
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
fn command_exists(program: &OsString) -> bool {
    let path = Path::new(program);
    if path.components().count() != 1 {
        return is_executable(path);
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(path)))
    })
}

/// The file of a project that sets variables for its code.
const ENV_FILE: &str = ".env";

/// The variables a `.env` sets, as `dotenvy` reads one, or the number of its
/// first line that is wrong. A value may name a variable the file set
/// earlier, as `$NAME` or `${NAME}`, unless it is between single quotes.
///
/// `dotenvy` says which line is wrong by its text, which may be a secret, so
/// the line is found again here and only its number is handed on.
fn parse_env(text: &str) -> std::result::Result<Vec<(String, String)>, usize> {
    dotenvy::from_read_iter(text.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| {
            let dotenvy::Error::LineParse(wrong, _) = error else {
                // Nothing is read from a file or from the environment here.
                return 0;
            };
            // A value that opens a quote and never closes it runs on to the
            // end of the text: the line it began on is the wrong one.
            // Of a line that is wrong from its value on, only the value is
            // said.
            let began = wrong.lines().next().unwrap_or_default();
            text.lines()
                .position(|line| line.contains(began))
                .map_or(0, |index| index + 1)
        })
}

/// `dir`, followed by what the variable already holds.
pub(super) fn search_path(dir: &Path, variable: &str) -> OsString {
    let mut paths = vec![dir.to_owned()];
    if let Some(existing) = std::env::var_os(variable) {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).unwrap_or_else(|_| dir.as_os_str().to_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::project::{Extractor, Strategy, strategy_hash};

    #[test]
    fn an_env_file_is_read_as_dotenv_files_are_written() {
        assert_eq!(parse_env(""), Ok(Vec::new()));
        let text = "\
# a comment

PLAIN=one
export EXPORTED=2
SPACED = three   # said of it
DOUBLE=\"four # five\"
SINGLE='$six'  # quoted
EMPTY=
WITH_EQUALS=a=b
AFTER_PLAIN=$PLAIN/more
AFTER_BRACES=\"${EXPORTED} of them\"
";
        let pairs = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect()
        };
        assert_eq!(
            parse_env(text),
            Ok(pairs(&[
                ("PLAIN", "one"),
                ("EXPORTED", "2"),
                ("SPACED", "three"),
                ("DOUBLE", "four # five"),
                ("SINGLE", "$six"),
                ("EMPTY", ""),
                ("WITH_EQUALS", "a=b"),
                ("AFTER_PLAIN", "one/more"),
                ("AFTER_BRACES", "2 of them"),
            ]))
        );
        // A wrong line is named by its number.
        for (text, line) in [
            ("A=1\nno equals sign\n", 2),
            ("1ST=1\n", 1),
            ("A B=1\n", 1),
            ("\n\nOPEN=\"no end\n", 3),
            ("AFTER='quoted' more\n", 1),
            // A value of more than one word is between quotes.
            ("A=1\nWORDS=one two\n", 2),
        ] {
            assert_eq!(parse_env(text), Err(line), "{text:?}");
        }
    }

    #[test]
    fn code_is_given_what_the_projects_env_file_sets_under_what_is_set_already() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let user_code = UserCode {
            work_dir: dir.path().join("work"),
            ..UserCode::new(&project)
        };
        let strategy = Strategy {
            name: "model".to_owned(),
            runtime: Runtime::Python,
            file: "entities/thing/strategies/model.py".to_owned(),
            hash: String::new(),
        };
        let given = |name: &str| -> Option<Option<String>> {
            let command = user_code.command(&strategy, None).unwrap();
            command
                .get_envs()
                .find(|(set, _)| set.to_str() == Some(name))
                .map(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
        };
        // With no `.env`, nothing is added.
        assert_eq!(given("SPOOL_TEST_FROM_DOT_ENV"), None);

        assert!(std::env::var_os("PATH").is_some());
        std::fs::write(
            project.join(".env"),
            "SPOOL_TEST_FROM_DOT_ENV=secret\nPATH=/nowhere\nPYTHONPATH=/nowhere\n\
             SPOOL_TOKEN=withheld\n",
        )
        .unwrap();
        assert_eq!(
            given("SPOOL_TEST_FROM_DOT_ENV"),
            Some(Some("secret".to_owned()))
        );
        // What this process was started with wins, and so does what Spool
        // sets for the code, and what is withheld from it stays withheld.
        assert_eq!(given("PATH"), None);
        let python_path = given("PYTHONPATH").unwrap().unwrap();
        assert!(!python_path.starts_with("/nowhere"), "{python_path}");
        assert_eq!(given("SPOOL_TOKEN"), Some(None));

        // A wrong line is reported by its number, and not with what it holds.
        std::fs::write(project.join(".env"), "TOKEN=fine\nsecret-value\n").unwrap();
        let refused = user_code.command(&strategy, None).unwrap_err().to_string();
        assert!(
            refused.contains(".env: line 2 is not `NAME=value`"),
            "{refused}"
        );
        assert!(!refused.contains("secret-value"), "{refused}");
        assert!(!refused.contains("fine"), "{refused}");
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
        assert!(!nowhere.installed(Runtime::Python));
        let looked = dir.path().join("bin/uv");
        assert_eq!(
            nowhere.check(&python).unwrap_err().to_string(),
            format!(
                "extractor `names`: it is a Python file, and Spool runs those with `uv`, which \
                 is not installed where Spool runs. It was looked for as `{}`. Install it there \
                 and run this again:\n\n  curl -LsSf https://astral.sh/uv/install.sh | sh\n\n\
                 Spool finds it where that puts it, with no restart. Other ways to install it: \
                 https://docs.astral.sh/uv/getting-started/installation/. If it is installed \
                 where the search path does not reach, set SPOOL_UV to it where Spool is started",
                looked.display()
            )
        );
        let message = nowhere.check(&typescript).unwrap_err().to_string();
        for part in [
            "it is a TypeScript file, and Spool runs those with `bun`, which is not installed \
             where Spool runs.",
            "\n\n  curl -fsSL https://bun.sh/install | bash\n\n",
            "https://bun.sh/docs/installation",
            "SPOOL_BUN",
        ] {
            assert!(message.contains(part), "no `{part}` in: {message}");
        }
        // The same when it is asked to fetch what the code depends on.
        let message = nowhere.install(python.clone()).unwrap_err().to_string();
        assert!(message.contains("astral.sh/uv/install.sh"), "{message}");

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
        assert!(nowhere.installed(Runtime::Python));
    }
}
