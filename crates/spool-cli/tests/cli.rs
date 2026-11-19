//! The `spool` command's arguments and exit codes.

mod support;

use std::path::Path;

use support::{benchmarks, run as spool, text};

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
    for arguments in [&[][..], &["--help"], &["help"]] {
        let output = spool(arguments);
        assert!(output.status.success());
        let stdout = text(&output.stdout);
        assert!(stdout.starts_with("Usage: spool <command>"), "{stdout}");
        for command in [
            "init", "info", "build", "apply", "serve", "dev", "show", "retract", "explain",
            "token", "bench", "schema",
        ] {
            assert!(stdout.contains(&format!("\n  {command} ")), "{command}");
        }
    }
    for command in [
        "init", "info", "build", "apply", "serve", "dev", "show", "retract", "explain", "token",
        "bench", "schema",
    ] {
        for arguments in [&[command, "--help"][..], &[command, "x", "-h"]] {
            let output = spool(arguments);
            assert!(output.status.success());
            let stdout = text(&output.stdout);
            assert!(
                stdout.starts_with(&format!("Usage: spool {command}")),
                "{stdout}"
            );
        }
    }
    assert_eq!(
        text(&spool(&["--version"]).stdout),
        format!("spool {}\n", env!("CARGO_PKG_VERSION"))
    );

    for (arguments, expected) in [
        (&["frobnicate"][..], "unknown command `frobnicate`"),
        (&["bench"], "`bench` needs a scenario"),
        (&["bench", "a", "b"], "unexpected argument `b`"),
        (&["bench", "a", "--fast"], "unknown option `--fast`"),
        (&["bench", "a", "--seed"], "`--seed` needs a value"),
        (&["bench", "a", "--seed", "lucky"], "needs a whole number"),
        (
            &["init", "a", "--example", "Not An Example"],
            "there is no example `Not An Example`: the examples are `companies`",
        ),
        (&["init", "a", "--example"], "`--example` needs a value"),
        (&["info", "a", "b"], "unexpected argument `b`"),
        (&["apply", "a"], "`--store` or `SPOOL_STORE`"),
        (&["serve", "extra"], "unexpected argument `extra`"),
        (
            &["serve", "--store", "s", "--listen", "nowhere"],
            "`--listen` needs an address",
        ),
        (&["dev", "--port", "1"], "unknown option `--port`"),
        // The server's own token is gone: a caller presents one that was made.
        (&["dev", "--token", "dev"], "unknown option `--token`"),
        // Nobody signs in to it, so it is not for anyone else to reach.
        (
            &["dev", "--listen", "0.0.0.0:7765"],
            "listens on this machine only",
        ),
        (
            &["dev", "--listen", "192.168.1.20:7765"],
            "listens on this machine only",
        ),
        (
            &["show", "weather"],
            "`weather` is not something `show` shows",
        ),
        (
            &["explain", "a:1", "b:2", "c:3"],
            "unexpected argument `c:3`",
        ),
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

#[test]
fn output_piped_to_a_reader_that_has_gone_ends_quietly() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;

    // As in `spool schema | head -n 1`, where `head` has already left.
    let mut child = support::spool()
        .arg("schema")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        text(&output.stderr),
        "",
        "nothing is said about the closed pipe"
    );
    assert_eq!(output.status.signal(), Some(13), "ended by SIGPIPE");
}
