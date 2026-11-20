//! The API's schema and the TypeScript client that is generated from it.

use std::path::PathBuf;
use std::process::Command;

use crate::support::*;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_committed_schema_is_the_one_the_server_speaks() {
    let path = repository().join("schema/spool-api.v1.schema.json");
    let committed = std::fs::read_to_string(&path).unwrap();
    let printed = succeed(&["schema"]);
    assert!(
        committed == printed,
        "{} is out of date: write the output of `spool schema` to it, \
         then run `bun run generate` in packages/client",
        path.display()
    );
    let schema: serde_json::Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(schema, spool_cli::api::schema());
    for name in [
        "RunEntry",
        "Changeset",
        "DraftEntry",
        "ReviewEntry",
        "ErrorResponse",
    ] {
        assert!(schema["$defs"][name].is_object(), "{name}");
    }
}

/// Runs a `bun` command in the client package, against this build's binary.
fn bun(arguments: &[&str]) {
    let output = Command::new("bun")
        .args(arguments)
        .current_dir(repository().join("packages/client"))
        .env("SPOOL_BIN", env!("CARGO_BIN_EXE_spool"))
        .output()
        .expect("bun is installed");
    assert!(
        output.status.success(),
        "bun {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
}

#[test]
#[ignore = "needs bun, and the network the first time"]
fn the_typescript_client_passes_its_tests_against_this_binary() {
    bun(&["install", "--frozen-lockfile"]);
    // The generated types are those of the committed schema, and they compile.
    bun(&["run", "check-types"]);
    bun(&["run", "typecheck"]);
    // The client calls a real server, and gets the same changesets from the
    // company scenario as the library does.
    bun(&["test"]);
}
