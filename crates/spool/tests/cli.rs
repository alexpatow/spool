//! The `spool` command's arguments and exit codes.

mod common;

use std::path::Path;
use std::process::{Command, Output};

use common::benchmarks;

fn spool(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spool"))
        .args(arguments)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            if entry.file_name() != ".spool" {
                copy_dir(&entry.path(), &target);
            }
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn usage_is_printed_on_request_and_on_a_mistake() {
    for arguments in [&[][..], &["--help"], &["bench", "--help"]] {
        let output = spool(arguments);
        assert!(output.status.success());
        assert!(text(&output.stdout).starts_with("Usage: spool bench <scenario>"));
    }

    for (arguments, expected) in [
        (&["frobnicate"][..], "unknown command `frobnicate`"),
        (&["bench"], "`bench` needs a scenario"),
        (&["bench", "a", "b"], "unexpected argument `b`"),
        (&["bench", "a", "--fast"], "unknown option `--fast`"),
        (&["bench", "a", "--seed"], "`--seed` needs a value"),
        (&["bench", "a", "--seed", "lucky"], "needs a whole number"),
    ] {
        let output = spool(arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let stderr = text(&output.stderr);
        assert!(stderr.contains(expected), "{arguments:?}: {stderr}");
        assert!(stderr.contains("Usage:"));
    }
}

#[test]
fn a_failed_run_says_why_and_exits_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-scenario");
    let output = spool(&["bench", missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(stderr.starts_with("spool: "), "{stderr}");
    assert!(stderr.contains("no-such-scenario"), "{stderr}");
}

#[test]
fn the_database_defaults_to_the_project_directory_and_is_replaced_each_run() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&benchmarks().join("resolution/people"), dir.path());
    let scenario = dir.path().join("sample.toml");

    for _ in 0..2 {
        let output = spool(&["bench", scenario.to_str().unwrap(), "--list", "2"]);
        assert!(output.status.success(), "{}", text(&output.stderr));
        let stdout = text(&output.stdout);
        assert!(
            stdout.contains("ingested  10 rows of `person` from `sample`: 10 new"),
            "{stdout}"
        );
        assert!(stdout.contains("identities (6):"), "{stdout}");
        assert!(stdout.contains("... and 4 more"), "{stdout}");
    }
    assert!(dir.path().join(".spool/bench.duckdb").is_file());
}
