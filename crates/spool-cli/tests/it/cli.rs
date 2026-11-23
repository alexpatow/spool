//! The `spool` command's arguments and exit codes. The help of each command
//! is checked against its snapshot by a test inside the binary.

use std::path::Path;

use crate::support::{self, benchmarks, run as spool, text};

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
fn help_is_printed_on_request() {
    for arguments in [&[][..], &["--help"], &["help"], &["-h"]] {
        let output = spool(arguments);
        assert!(output.status.success(), "{arguments:?}");
        let stdout = text(&output.stdout);
        assert!(
            stdout.starts_with("Extract, resolve and track entities\n\nUsage: spool"),
            "{stdout}"
        );
        for command in [
            "init", "info", "build", "apply", "serve", "dev", "show", "retract", "explain",
            "token", "bench", "schema",
        ] {
            assert!(stdout.contains(&format!("\n  {command} ")), "{command}");
        }
    }
    for command in ["init", "show", "token"] {
        for arguments in [
            &[command, "--help"][..],
            &["help", command],
            &[command, "-h"],
        ] {
            let output = spool(arguments);
            assert!(output.status.success(), "{arguments:?}");
            let stdout = text(&output.stdout);
            assert!(
                stdout.contains(&format!("\nUsage: spool {command} ")),
                "{stdout}"
            );
        }
    }
    // `-h` is a line for each option, and `--help` says more.
    let short = text(&spool(&["show", "records", "-h"]).stdout);
    let long = text(&spool(&["show", "records", "--help"]).stdout);
    assert!(short.lines().count() < long.lines().count());
    assert!(long.contains("They have none, and their source has not taken them back."));
    assert_eq!(
        text(&spool(&["--version"]).stdout),
        format!("spool {}\n", env!("CARGO_PKG_VERSION"))
    );
}

/// A mistake in the arguments is said with the usage of the command it was
/// made in, and exits with 2.
#[test]
fn a_mistake_is_said_with_the_usage_and_exits_with_two() {
    for (arguments, expected) in [
        (
            &["recompte"][..],
            "error: unrecognized subcommand 'recompte'\n\n  tip: a similar subcommand exists: 'recompute'\n\nUsage: spool [OPTIONS] [COMMAND]\n\nFor more information, try '--help'.\n",
        ),
        (
            &["show", "identitis"],
            "error: unrecognized subcommand 'identitis'\n\n  tip: some similar subcommands exist: 'documents', 'identities'\n\nUsage: spool show [OPTIONS] [COMMAND]\n\nFor more information, try '--help'.\n",
        ),
        (
            &["show", "identities", "--sort", "size"],
            "error: invalid value 'size' for '--sort <SORT>'\n  [possible values: records, names, sources, label, entity_type, created, id]\n\nFor more information, try '--help'.\n",
        ),
        (
            &["load", "rows.csv", "--sauce", "crm"],
            "error: unexpected argument '--sauce' found\n\n  tip: a similar argument exists: '--source'\n\nUsage: spool load --source <NAME> <FILE>\n\nFor more information, try '--help'.\n",
        ),
        (
            &["bench"],
            "error: the following required arguments were not provided:\n  <SCENARIO>\n\nUsage: spool bench <SCENARIO>\n\nFor more information, try '--help'.\n",
        ),
        (
            &["show", "records", "crm:c-1", "--ids"],
            "error: the argument '[RECORD]' cannot be used with '--ids'\n\nUsage: spool show records <RECORD>\n\nFor more information, try '--help'.\n",
        ),
        (
            &["init", "a", "--example", "people"],
            "error: invalid value 'people' for '--example <NAME>'\n  [possible values: companies]\n\nFor more information, try '--help'.\n",
        ),
        // Said by the command, not by the parser, with the same usage.
        (
            &["dev", "--listen", "0.0.0.0:7765"],
            "error: `spool dev` has no sign-in, so it listens on this machine only: `--listen` needs an address such as 127.0.0.1:7765, not `0.0.0.0:7765`. `spool serve` runs a server that others reach, and they sign in to it.\n\nUsage: spool dev [OPTIONS] [DIR]\n\nFor more information, try '--help'.\n",
        ),
    ] {
        let output = spool(arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert_eq!(text(&output.stderr), expected, "{arguments:?}");
        assert_eq!(text(&output.stdout), "", "{arguments:?}");
    }
}

/// Every mistake in the arguments exits with 2, and says what it is.
#[test]
fn every_mistake_in_the_arguments_exits_with_two_and_says_what_it_is() {
    for (arguments, expected) in [
        (&["bench", "a", "b"][..], "unexpected argument 'b'"),
        (
            &["bench", "a", "--seed"],
            "a value is required for '--seed <SEED>'",
        ),
        (
            &["bench", "a", "--seed", "lucky"],
            "invalid value 'lucky' for '--seed <SEED>'",
        ),
        (&["info", "a", "b"], "unexpected argument 'b'"),
        (&["apply", "a"], "`--store` or `SPOOL_STORE`"),
        (&["serve", "extra"], "unexpected argument 'extra'"),
        (
            &["serve", "--store", "s", "--listen", "nowhere"],
            "invalid value 'nowhere' for '--listen <ADDR>'",
        ),
        (&["dev", "--port", "1"], "unexpected argument '--port'"),
        // The server's own token is gone: a caller presents one that was made.
        (&["dev", "--token", "dev"], "unexpected argument '--token'"),
        (
            &["dev", "--listen", "192.168.1.20:7765"],
            "listens on this machine only",
        ),
        (&["dev", "--task-timeout", "0"], "0 is not in 1.."),
        (
            &["explain", "a:1", "b:2", "c:3"],
            "unexpected argument 'c:3'",
        ),
        (&["explain"], "<RECORD>"),
        (
            &["show", "runs", "first"],
            "invalid value 'first' for '[RUN]'",
        ),
        (
            &["show", "runs", "--sort", "kind"],
            "unexpected argument '--sort'",
        ),
        (
            &["show", "runs", "1", "--kind", "load"],
            "cannot be used with",
        ),
        (&["show", "log", "1"], "unexpected argument '1'"),
        (
            &["show", "--type", "company"],
            "unexpected argument '--type'",
        ),
        (
            &["show", "drafts", "anvil"],
            "invalid value 'anvil' for '[DRAFT]'",
        ),
        (
            &["show", "identities", "anvil"],
            "invalid value 'anvil' for '[IDENTITY]'",
        ),
        (
            &["recompute", "--identity", "x"],
            "invalid value 'x' for '--identity <ID>'",
        ),
        (&["recompute", "--reason", "x"], "--apply <PLAN>"),
        (
            &["recompute", "--apply", "p", "--limit", "1"],
            "cannot be used with",
        ),
        (&["load"], "<FILE>"),
        (&["load", "rows.csv", "--batch", "0"], "0 is not in 1.."),
        (&["retract", "--type", "thing", "1"], "--source <NAME>"),
        (
            &["retract", "--source", "shop", "--type", "thing"],
            "<KEYS>...",
        ),
        (&["token", "list", "ci"], "unexpected argument 'ci'"),
        (&["token", "add"], "<NAME>"),
        (&["token", "make", "ci"], "unrecognized subcommand 'make'"),
        (&["match", "rows.csv"], "unexpected argument 'rows.csv'"),
    ] {
        let output = spool(arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let stderr = text(&output.stderr);
        assert!(stderr.contains(expected), "{arguments:?}: {stderr}");
        assert!(stderr.starts_with("error: "), "{arguments:?}: {stderr}");
    }
    // A command that is only its subcommands lists them.
    let output = spool(&["token"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("\nUsage: spool token [OPTIONS] <COMMAND>\n"));
}

#[test]
fn a_failed_run_says_why_and_exits_with_one() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-scenario");
    let output = spool(&["bench", missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(stderr.starts_with("error: "), "{stderr}");
    assert!(stderr.contains("no-such-scenario"), "{stderr}");
}

/// A variable that is set and empty is as good as one that is not set.
#[test]
fn an_empty_variable_says_nothing() {
    let output = support::spool()
        .args(["apply", "nowhere"])
        .env("SPOOL_STORE", "")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("`--store` or `SPOOL_STORE`"));
    // And one named on the command line is the one.
    let output = support::spool()
        .args(["token", "list", "--store", "nowhere.duckdb"])
        .env("SPOOL_STORE", "")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("there is no store at nowhere.duckdb"));
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
