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
//!    run from `target/`.
//!
//! With the `bundled` feature DuckDB is linked statically and none of this
//! matters.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_BUNDLED").is_some() {
        return;
    }
    let (origin, library) = match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => ("@executable_path", "libduckdb.dylib"),
        Ok("linux") => ("$ORIGIN", "libduckdb.so"),
        // Windows finds a DLL beside the executable without being told.
        _ => return,
    };
    let mut search = vec![origin.to_owned(), format!("{origin}/../lib")];
    if let Some(downloaded) = downloaded(library) {
        search.push(downloaded.display().to_string());
    }
    for dir in search {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{dir}");
    }
}

/// The directory the `duckdb` crate's build downloaded the library to:
/// `duckdb-download/<target>/<version>` beside the profile directory.
fn downloaded(library: &str) -> Option<PathBuf> {
    // OUT_DIR is <profile>/build/<package>-<hash>/out.
    let out = PathBuf::from(std::env::var_os("OUT_DIR")?);
    let profile = out.ancestors().nth(3)?;
    find(&profile.parent()?.join("duckdb-download"), library, 3)
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
