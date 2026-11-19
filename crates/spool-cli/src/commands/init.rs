//! `spool init`: an empty project, or one filled with an example.

use std::path::{Path, PathBuf};
use std::process::Command;

use anstream::println;
use clap::Args;
use clap::builder::PossibleValue;
use spool::{Manifest, Runtime};

use super::project::{failed, write};
use crate::Failure;
use crate::output::{Output, OutputArgs, hint};

/// Create a project, empty or from an example
///
/// Creates a project: the standard folders, a .gitignore and a .env.example.
/// With --example, it's a complete project with sample rows and documents
/// to load.
///
/// Runs git init if the folder has no repository. This requires Git on PATH
/// and creates an empty repository with no commits.
///
/// Either way, it writes what coding agents read: AGENTS.md (imported by
/// CLAUDE.md) and skills for using Spool, in .agents/skills/ with a symlink
/// to each from .claude/skills/. `spool update-skills` updates them to a
/// later version's.
///
/// The folder has to be empty apart from `.git` and `.DS_Store`. Otherwise
/// it's refused and nothing is written.
///
/// Spool passes the variables in the project's .env to the extractors and
/// strategies it runs, and never prints their values. Variables already set
/// in Spool's environment win. Git ignores .env, and .env.example shows the
/// format.
#[derive(Args)]
#[command(
    after_help = "Examples:\n  spool init companies --example companies\n  spool init my-data"
)]
pub struct InitArgs {
    /// The folder to make the project in, made if it is not there [default: the current folder]
    dir: Option<PathBuf>,
    /// Fill the project with an example
    #[arg(long, value_name = "NAME", value_parser = examples())]
    example: Option<String>,
    #[command(flatten)]
    output: OutputArgs,
}

/// The names of the examples, each with what it holds.
fn examples() -> Vec<PossibleValue> {
    EXAMPLES
        .iter()
        .map(|example| PossibleValue::new(example.name).help(example.about))
        .collect()
}

/// What may be in a folder that `spool init` takes for empty: a repository
/// made with `git init`, and what the Finder leaves.
const MAY_HOLD: [&str; 2] = [".git", ".DS_Store"];

/// The folders the project reader looks in, with what each holds. Each is
/// written with a `.gitkeep` while it is empty, so that Git keeps it: the
/// reader needs `entities/`, and the others say where a file goes.
const FOLDERS: [(&str, &str); 4] = [
    (
        "sources",
        "a YAML file for each source of rows or documents",
    ),
    (
        "entities",
        "a folder for each entity type, with its features and its rules",
    ),
    ("transforms", "SQL functions that features are made with"),
    (
        "extractors",
        "Python or TypeScript files that find records in text",
    ),
];

const GITIGNORE: &str = ".spool/\n.env\n";

const ENV_EXAMPLE: &str = "\
# Secrets and settings for this project's own code: its extractors and its
# strategies. Copy this file to .env, which Git does not keep, and set each
# one there as NAME=value. Spool hands them to that code each time it runs
# it, and never prints them. A variable that is set already where Spool runs
# keeps its value.
#
# OPENAI_API_KEY=sk-...
";

/// A project to start from, which `--example <name>` writes.
struct Example {
    name: &'static str,
    /// What the example is, in the help.
    about: &'static str,
    /// Each file, by its path in the project.
    files: &'static [(&'static str, &'static str)],
    /// What `spool init` lists of it, with what each holds.
    listed: &'static [(&'static str, &'static str)],
    /// The commands that come next, in the project, with what each does.
    next: &'static [(&'static str, &'static str)],
    /// What is said after the commands.
    note: &'static str,
    /// What runs the example's code. Where one is not installed, it is said
    /// how to install it.
    runtimes: &'static [Runtime],
}

macro_rules! example_files {
    ($example:literal: $($file:literal),+ $(,)?) => {
        &[$(($file, include_str!(concat!("../examples/", $example, "/", $file)))),+]
    };
}

const EXAMPLES: &[Example] = &[Example {
    name: "companies",
    about: "a CRM's rows and a call note, matched into companies by three rules of SQL. Its \
            extractor finds company names with a small model: a Python file that Spool runs \
            with `uv`, whose first run fetches about 330 MB",
    files: example_files!("companies":
        "sources/crm.yaml",
        "sources/notes.yaml",
        "entities/company/entity.yaml",
        "entities/company/rules/block/name_prefix.sql",
        "entities/company/rules/match/similar_name.sql",
        "entities/company/rules/no_match/different_country.sql",
        "transforms/name_key.sql",
        "extractors/companies.py",
        "samples/crm-1.csv",
        "samples/crm-2.csv",
        "samples/notes-1.txt",
    ),
    listed: &[
        ("sources/crm.yaml", "the columns of a CRM export"),
        (
            "sources/notes.yaml",
            "what a record found in a note may say",
        ),
        (
            "entities/company/entity.yaml",
            "what companies are compared by",
        ),
        (
            "entities/company/rules/",
            "three rules, one line of SQL each",
        ),
        (
            "transforms/name_key.sql",
            "the lower-case name the rules compare",
        ),
        (
            "extractors/companies.py",
            "finds company names in text, with a small model",
        ),
        ("samples/crm-1.csv", "six rows to load first"),
        ("samples/crm-2.csv", "three more for later"),
        (
            "samples/notes-1.txt",
            "a call note that names three companies",
        ),
    ],
    next: &[
        ("spool dev", "runs the server on this machine"),
        (
            "spool load samples/crm-1.csv",
            "loads the first rows, from a second terminal",
        ),
        (
            "spool extract samples/notes-1.txt",
            "finds the companies in the note",
        ),
    ],
    // Said before the step that needs it, and not by a run that fails: the
    // rows load and match without `uv`.
    note: "The extractor is a Python file, which Spool runs with `uv`. Its first run fetches\n\
           about 330 MB, most of it the model, and later runs fetch nothing.",
    runtimes: &[Runtime::Python],
}];

/// What is listed of every project, after what is in it.
const ALWAYS: [(&str, &str); 4] = [
    (
        ".gitignore",
        "keeps the server's data, .spool/, and .env out of Git",
    ),
    (
        ".env.example",
        "the form of .env, where your code's secrets go",
    ),
    (
        "AGENTS.md",
        "what a coding agent should know here; CLAUDE.md imports it",
    ),
    (
        ".agents/skills/",
        "skills that teach coding agents Spool, linked from .claude/skills/",
    ),
];

pub fn init(arguments: InitArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool init`")?;
    let example = arguments.example.as_deref().map(|name| {
        EXAMPLES
            .iter()
            .find(|example| example.name == name)
            .expect("the parser takes only the names of examples")
    });
    let named = arguments.dir.is_some();
    let dir = super::project_dir(&arguments.dir);
    refuse_unless_empty(&dir, named)?;
    init_git(&dir)?;

    let mut files: Vec<(String, &str)> = example
        .map(|example| {
            example
                .files
                .iter()
                .map(|(file, contents)| ((*file).to_owned(), *contents))
                .collect()
        })
        .unwrap_or_default();
    for (folder, _) in FOLDERS {
        let prefix = format!("{folder}/");
        if !files.iter().any(|(file, _)| file.starts_with(&prefix)) {
            files.push((format!("{folder}/.gitkeep"), ""));
        }
    }
    files.push((".gitignore".to_owned(), GITIGNORE));
    files.push((".env.example".to_owned(), ENV_EXAMPLE));
    for (file, contents) in &files {
        write(&dir.join(file), contents)?;
    }
    super::update_skills::write_agent_files(&dir, false)?;

    // What was written has to be a project that builds.
    Manifest::load(&dir)?;

    let folders: Vec<(String, &str)> = FOLDERS
        .iter()
        .map(|(folder, holds)| (format!("{folder}/"), *holds))
        .collect();
    let mut written: Vec<(String, &str)> = match example {
        Some(example) => example
            .listed
            .iter()
            .map(|(file, holds)| ((*file).to_owned(), *holds))
            .collect(),
        None => folders,
    };
    written.extend(ALWAYS.map(|(file, holds)| (file.to_owned(), holds)));
    let commands: &[(&str, &str)] = match example {
        Some(example) => example.next,
        None => &[(
            "spool dev",
            "runs the server, and applies each file as you save it",
        )],
    };
    let user_code = super::user_code(None, &dir);
    let missing: Vec<Runtime> = example
        .map(|example| example.runtimes)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|runtime| !user_code.installed(*runtime))
        .collect();
    if !out.is_table() {
        let line =
            |(said, about): &(&str, &str)| serde_json::json!({ "path": said, "holds": about });
        out.json(&serde_json::json!({
            "created": dir,
            "files": written.iter().map(|(file, holds)| line(&(file.as_str(), *holds))).collect::<Vec<_>>(),
            "next": commands
                .iter()
                .map(|(command, does)| serde_json::json!({ "command": command, "does": does }))
                .collect::<Vec<_>>(),
            "not_installed": missing.iter().map(|runtime| program(*runtime)).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    // What each line is for begins in one column: six characters after the
    // longest file, and at least two after the longest command. What to do
    // next goes to standard error.
    let longest = |lines: &mut dyn Iterator<Item = &str>| {
        lines.map(|line| line.chars().count()).max().unwrap_or(0)
    };
    let width = (longest(&mut written.iter().map(|(file, _)| file.as_str())) + 6)
        .max(longest(&mut commands.iter().map(|(command, _)| *command)) + 2);
    println!("created   {}\n", dir.display());
    for (file, holds) in &written {
        println!("  {file:<width$}{holds}");
    }
    hint("");
    if !is_current_dir(&dir) {
        hint(format!("  cd {}", shell_word(&dir)));
    }
    for (command, does) in commands {
        hint(format!("  {command:<width$}{does}"));
    }
    match example {
        Some(example) => {
            hint(format!("\n{}", example.note));
            for runtime in missing {
                hint(format!(
                    "`{}` is not installed here. Before `spool extract`, install it:\n\n  {}",
                    program(runtime),
                    runtime.install_command()
                ));
            }
        }
        None => hint(
            "\nThe project is empty. Declare a source under sources/ and an entity type\n\
             under entities/, then load your rows with `spool load`. To see a project that\n\
             works first, with sample rows: spool init companies --example companies",
        ),
    }
    Ok(())
}

/// What runs code of a runtime.
fn program(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Python => "uv",
        Runtime::Typescript => "bun",
    }
}

fn init_git(dir: &Path) -> Result<(), Failure> {
    let git = dir.join(".git");
    // Worktrees use a .git file. A failed git init can leave a .git
    // directory without a HEAD or an objects directory.
    if git.is_file() || (git.join("HEAD").is_file() && git.join("objects").is_dir()) {
        return Ok(());
    }

    let output = Command::new("git")
        .args(["init", "--quiet", "--"])
        .arg(dir)
        // A command called from Git can inherit paths for another repository.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .output()
        .map_err(|error| {
            let failure = failed(format!(
                "could not run git init in {}: {error}",
                dir.display()
            ));
            if error.kind() == std::io::ErrorKind::NotFound {
                failure.hint("install Git and make sure git is on PATH")
            } else {
                failure
            }
        })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(failed(format!(
        "git init in {} failed ({}): {}",
        dir.display(),
        output.status,
        detail.trim()
    )))
}

/// Fails unless `dir` is not there or holds nothing but what [`MAY_HOLD`]
/// names. A refusal names a few of what it holds.
fn refuse_unless_empty(dir: &Path, named: bool) -> Result<(), Failure> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) if dir.exists() && !dir.is_dir() => {
            return Err(failed(format!(
                "{} is a file, and a project is a folder",
                dir.display()
            )));
        }
        Err(error) => return Err(failed(format!("{}: {error}", dir.display()))),
    };
    let mut held = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| failed(format!("{}: {error}", dir.display())))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !MAY_HOLD.contains(&name.as_str()) {
            held.push(name);
        }
    }
    if held.is_empty() {
        return Ok(());
    }
    if dir.join("entities").is_dir() {
        let which = match named {
            true => dir.display().to_string(),
            false => "this folder".to_owned(),
        };
        return Err(failed(format!(
            "{which} is a project already: it has an entities/ folder"
        )));
    }
    // What a reader sees in a listing first, and hidden files after.
    held.sort_by_key(|name| (name.starts_with('.'), name.clone()));
    const SHOWN: usize = 3;
    let mut listed = held
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if held.len() > SHOWN {
        listed.push_str(&format!(" and {} more", held.len() - SHOWN));
    }
    Err(failed(if named {
        format!(
            "{} already holds files ({listed}). `spool init` makes a project only in a new \
             or empty folder: give it another name",
            dir.display()
        )
    } else {
        format!(
            "this folder already holds files ({listed}), and `spool init` makes a project \
             only in an empty one. Nothing was written. `spool init <name>` makes the \
             project in a new folder of that name"
        )
    }))
}

/// A path as a word of a shell command: as it is if it is plain, and
/// between single quotes if not.
fn shell_word(path: &Path) -> String {
    let path = path.display().to_string();
    let plain = !path.is_empty()
        && !path.starts_with('-')
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    if plain {
        path
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

fn is_current_dir(dir: &Path) -> bool {
    match (std::fs::canonicalize(dir), std::env::current_dir()) {
        (Ok(dir), Ok(current)) => {
            std::fs::canonicalize(current).is_ok_and(|current| current == dir)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_an_example_lists_is_what_it_writes() {
        for example in EXAMPLES {
            for (listed, _) in example.listed {
                assert!(
                    example
                        .files
                        .iter()
                        .any(|(file, _)| file == listed || file.starts_with(listed)),
                    "{}: {listed}",
                    example.name
                );
            }
            for (file, _) in example.files {
                assert!(
                    example
                        .listed
                        .iter()
                        .any(|(listed, _)| file == listed || file.starts_with(listed)),
                    "{}: {file} is not listed",
                    example.name
                );
            }
        }
    }
}
