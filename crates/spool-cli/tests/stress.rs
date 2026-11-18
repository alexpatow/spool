//! The stress tests of `packages/stress`, in their small form.

use std::path::PathBuf;
use std::process::Command;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs a `bun` command in the stress package, against this build's binary.
fn bun(arguments: &[&str]) {
    let output = Command::new("bun")
        .args(arguments)
        .current_dir(repository().join("packages/stress"))
        .env("SPOOL_BIN", env!("CARGO_BIN_EXE_spool"))
        .output()
        .expect("bun is installed");
    assert!(
        output.status.success(),
        "bun {arguments:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "needs bun, and the network the first time"]
fn a_server_stands_up_to_scale_crashes_a_random_reviewer_and_hostile_requests() {
    bun(&["install", "--frozen-lockfile"]);
    bun(&["run", "typecheck"]);
    // Small runs: a store that grows in batches, a server killed in the
    // middle of its work, reviewers who do anything in any order, and
    // requests nobody should send. Each ends with the store verified. The
    // test files run side by side, each with servers of its own.
    bun(&["test", "--parallel"]);
}
