//! `spool reset`: starts the development store of `spool dev` over.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use super::accounts_path;
use crate::Failure;
use crate::args::Spec;

pub const USAGE: &str = "\
Usage: spool reset [<dir>] [options]

Starts the development store of `spool dev` over: removes the store under
<dir>/.spool/, with every row, document, identity, run and changeset it
holds, so that the next `spool dev` begins with an empty one. It says what it
removes and what it keeps, and asks first.

It keeps the project's files, the accounts and tokens of `spool dev` (the
.auth.sqlite file beside the store), the manifest `spool build` wrote, and
everything else under .spool/, such as what an extractor keeps there.

It removes nothing while a `spool dev` has the store open: stop that one with
Ctrl-C first. It starts over only a store under <dir>/.spool/, never one a
`spool serve` runs on.

  <dir>           the project directory (default: the current directory)

Options:
  --store <path>  the development store, if `spool dev` was given one with
                  `--store`; it must be under <dir>/.spool/
                  (default: <dir>/.spool/dev.duckdb)
  --yes           remove it without asking, which is needed where nobody
                  can answer, as in a script
";

pub fn reset(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &["--store"],
            flags: &["--yes"],
            positional: 1,
        },
    )?;
    let dir = arguments.project_dir();
    let spool_dir = dir.join(".spool");
    let store = arguments
        .path("--store")
        .unwrap_or_else(|| spool_dir.join("dev.duckdb"));
    let shown = |path: &Path| -> String {
        path.strip_prefix(&dir)
            .unwrap_or(path)
            .display()
            .to_string()
    };

    if !spool_dir.is_dir() {
        println!(
            "{} does not exist, so there is no development store to remove",
            spool_dir.display()
        );
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
        return Err(Failure::Usage(format!(
            "`spool reset` removes only a development store: a .duckdb file directly under {}. \
             {} is not one, and may be the store of a `spool serve`, which no command removes.",
            spool_dir.display(),
            store.display()
        )));
    }

    // DuckDB's log of writes not yet in the file, and where it spills
    // what does not fit in memory.
    let mut removed: Vec<PathBuf> = vec![store.clone()];
    for suffix in ["wal", "tmp"] {
        let mut path = store.clone().into_os_string();
        path.push(format!(".{suffix}"));
        removed.push(PathBuf::from(path));
    }
    removed.retain(|path| path.exists());
    if removed.is_empty() {
        println!(
            "there is no development store at {}: nothing to remove",
            shown(&store)
        );
        return Ok(());
    }

    // Held until the files are gone, so that no `spool dev` opens the
    // store half way. A file DuckDB cannot read is removed all the same.
    let _held = match spool::hold_store(&store) {
        Ok(held) => Some(held),
        Err(spool::Error::StoreInUse { holder, .. }) => {
            return Err(Failure::Failed(format!(
                "the store {} is open in another process{}, most likely a `spool dev` on this \
                 project.\nStop it with Ctrl-C, then run `spool reset` again. Nothing was removed.",
                shown(&store),
                holder.map_or_else(String::new, |pid| format!(" (PID {pid})")),
            )));
        }
        Err(_) => None,
    };

    let mut kept: Vec<(String, String)> = Vec::new();
    let accounts = accounts_path(&store);
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&spool_dir)
        .map_err(|error| Failure::Failed(format!("{}: {error}", spool_dir.display())))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| !removed.contains(path))
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

    println!(
        "This starts the development store of {} over.",
        match arguments.positional.first() {
            Some(given) => given.clone(),
            None => "this project".to_owned(),
        }
    );
    println!("\nremoves");
    let rows: Vec<(String, String)> = removed
        .iter()
        .map(|path| (shown(path), size(path)))
        .collect();
    columns(&rows);
    println!("\nkeeps");
    kept.push((
        "the project's files".to_owned(),
        "everything outside .spool/".to_owned(),
    ));
    columns(&kept);
    println!();

    if !arguments.flag("--yes") {
        if !std::io::stdin().is_terminal() {
            return Err(Failure::Failed(
                "nothing was removed: nobody can be asked here, so add `--yes` to remove it"
                    .to_owned(),
            ));
        }
        print!("Remove the store? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .map_err(|error| Failure::Failed(format!("reading the answer: {error}")))?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("nothing was removed");
            return Ok(());
        }
    }

    for path in &removed {
        let gone = match path.is_dir() {
            true => std::fs::remove_dir_all(path),
            false => std::fs::remove_file(path),
        };
        gone.map_err(|error| Failure::Failed(format!("{}: {error}", path.display())))?;
        println!("removed  {}", shown(path));
    }
    println!(
        "\nThe next `spool dev` starts with an empty store. `spool load` and `spool extract` \
         fill it again."
    );
    Ok(())
}

/// Two columns, indented.
fn columns(rows: &[(String, String)]) {
    let width = rows
        .iter()
        .map(|(left, _)| left.chars().count())
        .max()
        .unwrap_or(0);
    for (left, right) in rows {
        println!("  {left:<width$}  {right}");
    }
}

/// How much a file or a directory holds, for a person to read.
fn size(path: &Path) -> String {
    fn bytes(path: &Path) -> u64 {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => std::fs::read_dir(path)
                .map(|entries| {
                    entries
                        .filter_map(|entry| Some(bytes(&entry.ok()?.path())))
                        .sum()
                })
                .unwrap_or(0),
            Ok(metadata) => metadata.len(),
            Err(_) => 0,
        }
    }
    let bytes = bytes(path) as f64;
    match bytes {
        b if b < 1024.0 => format!("{b} bytes"),
        b if b < 1024.0 * 1024.0 => format!("{:.1} KB", b / 1024.0),
        b if b < 1024.0 * 1024.0 * 1024.0 => format!("{:.1} MB", b / 1024.0 / 1024.0),
        b => format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0),
    }
}
