//! `spool reset`: starts the development store of `spool dev` over.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anstream::{eprint, eprintln, println};
use clap::Args;

use super::accounts_path;
use crate::Failure;
use crate::output::{Output, OutputArgs, hint, render};

/// Start the development store of `spool dev` over
///
/// Starts the development store of `spool dev` over: removes the store under
/// <DIR>/.spool/, with every row, document, identity, run and changeset it
/// holds, so that the next `spool dev` begins with an empty one. It says what
/// it removes and what it keeps, and asks first.
///
/// It keeps the project's files, the accounts and tokens of `spool dev` (the
/// .auth.sqlite file beside the store), the manifest `spool build` wrote, and
/// everything else under .spool/, such as what an extractor keeps there.
///
/// It removes nothing else than these, and each only if it is what DuckDB
/// makes: <store>.wal, a file; <store>.tmp, a directory, with what is in it,
/// where a link in it is removed and not what the link points to; and
/// <store>, a file, last. <store> is a .duckdb file directly under
/// <DIR>/.spool/. If any of them, or .spool itself, is a link or of another
/// kind, it removes nothing.
///
/// It removes nothing while a `spool dev` has the store open: stop that one
/// with Ctrl-C first. It starts over only a store under <DIR>/.spool/, never
/// one a `spool serve` runs on.
#[derive(Args)]
pub struct ResetArgs {
    /// The project directory [default: the current directory]
    dir: Option<PathBuf>,
    /// The development store, if `spool dev` was given one with `--store` [default: <DIR>/.spool/dev.duckdb]
    ///
    /// It must be directly under <DIR>/.spool/.
    #[arg(long, value_name = "PATH")]
    store: Option<PathBuf>,
    /// Remove it without asking, which is needed where nobody can answer, as in a script
    #[arg(long)]
    yes: bool,
    #[command(flatten)]
    output: OutputArgs,
}

pub fn reset(arguments: ResetArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool reset`")?;
    // What it removes and keeps is the report, or with JSON what the
    // question is about, which goes to standard error.
    let say = |line: &str| match out.is_table() {
        true => println!("{line}"),
        false => eprintln!("{line}"),
    };
    let nothing = || {
        if !out.is_table() {
            out.json(&serde_json::json!({ "removed": [], "kept": [] }));
        }
    };
    let dir = super::project_dir(&arguments.dir);
    let spool_dir = dir.join(".spool");
    let store = arguments
        .store
        .clone()
        .unwrap_or_else(|| spool_dir.join("dev.duckdb"));
    let shown = |path: &Path| -> String {
        path.strip_prefix(&dir)
            .unwrap_or(path)
            .display()
            .to_string()
    };

    let spool_kind = std::fs::symlink_metadata(&spool_dir).map(|found| found.file_type());
    if spool_kind.as_ref().is_ok_and(|kind| kind.is_symlink()) {
        return Err(Failure::failed(format!(
            "{} is a link to another place, and `spool reset` removes nothing outside the \
             project. Nothing was removed.",
            spool_dir.display()
        )));
    }
    if !spool_kind.is_ok_and(|kind| kind.is_dir()) {
        say(&format!(
            "{} does not exist, so there is no development store to remove",
            spool_dir.display()
        ));
        nothing();
        return Ok(());
    }
    let inside = store
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .zip(spool_dir.canonicalize().ok())
        .is_some_and(|(parent, spool_dir)| parent == spool_dir);
    if !inside
        || store
            .extension()
            .is_none_or(|extension| extension != "duckdb")
    {
        return Err(Failure::usage(format!(
            "`spool reset` removes only a development store: a .duckdb file directly under {}. \
             {} is not one, and may be the store of a `spool serve`, which no command removes.",
            spool_dir.display(),
            store.display()
        )));
    }

    // DuckDB's log of writes not yet in the file, and where it spills what
    // does not fit in memory, then the store itself: the store goes last,
    // so that the lock on it is held while the others are removed. Each
    // must be what DuckDB makes, and none a link, or nothing is removed.
    let sidecar = |suffix: &str| {
        let mut path = store.clone().into_os_string();
        path.push(suffix);
        PathBuf::from(path)
    };
    let mut removed: Vec<(PathBuf, Kind)> = Vec::new();
    for (path, kind) in [
        (sidecar(".wal"), Kind::File),
        (sidecar(".tmp"), Kind::Directory),
        (store.clone(), Kind::File),
    ] {
        match kind.check(&path) {
            Ok(true) => removed.push((path, kind)),
            Ok(false) => {}
            Err(is) => {
                return Err(Failure::failed(format!(
                    "{} is {is}, where `spool reset` expects {}. It removes only what DuckDB \
                     makes, so nothing was removed.",
                    path.display(),
                    kind.expected()
                )));
            }
        }
    }
    if removed.is_empty() {
        say(&format!(
            "there is no development store at {}: nothing to remove",
            shown(&store)
        ));
        nothing();
        return Ok(());
    }

    // Held until the files are gone, so that no `spool dev` opens the
    // store half way. A file DuckDB cannot read is removed all the same.
    let _held = match spool::hold_store(&store) {
        Ok(held) => Some(held),
        Err(spool::Error::StoreInUse { holder, .. }) => {
            return Err(Failure::failed(format!(
                "the store {} is open in another process{}, most likely a `spool dev` on this \
                 project. Nothing was removed.",
                shown(&store),
                holder.map_or_else(String::new, |pid| format!(" (PID {pid})")),
            ))
            .hint("stop it with Ctrl-C, then run `spool reset` again"));
        }
        Err(_) => None,
    };

    let mut kept: Vec<(String, String)> = Vec::new();
    let accounts = accounts_path(&store);
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&spool_dir)
        .map_err(|error| Failure::failed(format!("{}: {error}", spool_dir.display())))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| !removed.iter().any(|(gone, _)| gone == path))
        .collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let accounts_file = path
            .to_string_lossy()
            .starts_with(accounts.to_string_lossy().as_ref());
        let what = if accounts_file {
            "the accounts and tokens of `spool dev`"
        } else if name == "manifest.json" {
            "the manifest `spool build` wrote"
        } else if path
            .extension()
            .is_some_and(|extension| extension == "duckdb")
        {
            "another store"
        } else {
            "not the store's"
        };
        let shown = match path.is_dir() {
            true => format!("{}/", shown(&path)),
            false => shown(&path),
        };
        kept.push((shown, what.to_owned()));
    }

    kept.push((
        "the project's files".to_owned(),
        "everything outside .spool/".to_owned(),
    ));
    if out.is_table() || !arguments.yes {
        say(&format!(
            "This starts the development store of {} over.",
            match &arguments.dir {
                Some(given) => given.display().to_string(),
                None => "this project".to_owned(),
            }
        ));
        say("\nremoves");
        for (path, _) in &removed {
            say(&format!("  {}", shown(path)));
        }
        say("\nkeeps");
        let rows: Vec<Vec<String>> = kept
            .iter()
            .map(|(path, what)| vec![path.clone(), what.clone()])
            .collect();
        for line in render(None, &rows, 2) {
            say(&line);
        }
        say("");
    }

    if !arguments.yes {
        if !std::io::stdin().is_terminal() {
            return Err(Failure::failed(
                "nothing was removed: nobody can be asked here, so add `--yes` to remove it"
                    .to_owned(),
            ));
        }
        eprint!("Remove the store? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .map_err(|error| Failure::failed(format!("reading the answer: {error}")))?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            say("nothing was removed");
            nothing();
            return Ok(());
        }
    }

    for (path, kind) in &removed {
        // Checked again: it may have changed while the question was asked.
        if let Err(is) = kind.check(path) {
            return Err(Failure::failed(format!(
                "{} is {is} now, where `spool reset` expects {}; it stopped there",
                path.display(),
                kind.expected()
            )));
        }
        // `remove_dir_all` removes a link inside the directory, not what
        // it points to.
        let gone = match kind {
            Kind::Directory => std::fs::remove_dir_all(path),
            Kind::File => std::fs::remove_file(path),
        };
        gone.map_err(|error| Failure::failed(format!("{}: {error}", path.display())))?;
        if out.is_table() {
            println!("removed  {}", shown(path));
        }
    }
    if !out.is_table() {
        out.json(&serde_json::json!({
            "store": store,
            "removed": removed.iter().map(|(path, _)| path).collect::<Vec<_>>(),
            "kept": kept
                .iter()
                .map(|(path, what)| serde_json::json!({ "path": path, "what": what }))
                .collect::<Vec<_>>(),
        }));
    }
    hint(
        "\nThe next `spool dev` starts with an empty store. `spool load` and `spool extract` \
         fill it again.",
    );
    Ok(())
}

/// What a path `spool reset` removes must be.
#[derive(Clone, Copy)]
enum Kind {
    File,
    Directory,
}

impl Kind {
    /// Whether there is such a thing at `path`, without following a link.
    /// `Err` with what is there instead, if it is anything else.
    fn check(self, path: &Path) -> Result<bool, &'static str> {
        let found = match std::fs::symlink_metadata(path) {
            Ok(found) => found.file_type(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err("something that cannot be read"),
        };
        match (self, found) {
            (_, found) if found.is_symlink() => Err("a link"),
            (Kind::File, found) if found.is_file() => Ok(true),
            (Kind::Directory, found) if found.is_dir() => Ok(true),
            (_, found) if found.is_dir() => Err("a directory"),
            (_, found) if found.is_file() => Err("a file"),
            _ => Err("neither a file nor a directory"),
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Kind::File => "a file",
            Kind::Directory => "a directory",
        }
    }
}
