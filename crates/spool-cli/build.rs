//! Tells the `spool` binary where to find the DuckDB shared library, so that
//! it starts without `cargo run` setting a library path for it.
//!
//! The `duckdb` crate links a prebuilt `libduckdb` by default and downloads
//! it under the target directory. The binary looks for it, in this order:
//!
//! 1. beside itself,
//! 2. in `../lib` relative to itself, which is where an image or an install
//!    puts it,
//! 3. where the build downloaded it, which is what makes a development build
//!    run from `target/`,
//! 4. in the copy that build keeps in its own output directory. The download
//!    can be gone while the copy is still there: a cache of `target/` that
//!    keeps only what Cargo wrote restores it that way.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let (origin, library) = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => ("@executable_path", "libduckdb.dylib"),
        Ok("linux") => ("$ORIGIN", "libduckdb.so"),
        // Windows finds a DLL beside the executable without being told.
        _ => return,
    };
    let mut search = vec![origin.to_owned(), format!("{origin}/../lib")];
    for dir in [downloaded(library), copied(library)].into_iter().flatten() {
        search.push(dir.display().to_string());
    }
    for dir in search {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{dir}");
    }
}

/// The directory the `duckdb` crate's build downloaded the library to:
/// `duckdb-download/<target>/<version>` beside the profile directory.
fn downloaded(library: &str) -> Option<PathBuf> {
    // The `duckdb` crate's build names the directory for what depends on it.
    if let Some(named) = std::env::var_os("DEP_DUCKDB_INCLUDE").map(PathBuf::from)
        && named.join(library).is_file()
    {
        return Some(named);
    }
    find(&profile()?.parent()?.join("duckdb-download"), library, 3)
}

/// The output directory of the `duckdb` crate's build, where it copies the
/// library: `<profile>/build/libduckdb-sys-<hash>/out`. Builds with other
/// features leave directories of their own, so this is the newest copy.
fn copied(library: &str) -> Option<PathBuf> {
    std::fs::read_dir(profile()?.join("build"))
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("libduckdb-sys-"))
        })
        .map(|path| path.join("out"))
        .filter_map(|out| {
            let modified = out.join(library).metadata().ok()?.modified().ok()?;
            Some((modified, out))
        })
        .max()
        .map(|(_, out)| out)
}

/// The profile directory, `target/debug` for instance.
fn profile() -> Option<PathBuf> {
    // OUT_DIR is <profile>/build/<package>-<hash>/out.
    let out = PathBuf::from(std::env::var_os("OUT_DIR")?);
    Some(out.ancestors().nth(3)?.to_owned())
}

fn find(dir: &Path, library: &str, depth: usize) -> Option<PathBuf> {
    if dir.join(library).is_file() {
        return Some(dir.to_owned());
    }
    if depth == 0 {
        return None;
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.is_dir())
        .collect();
    entries.sort();
    entries
        .into_iter()
        .find_map(|entry| find(&entry, library, depth - 1))
}
