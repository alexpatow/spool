//! The commands a project goes through: `init`, `info`, `build`, `apply`,
//! `serve` and `dev`, run as a user runs them.

mod support;

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};
use spool::Rows;
use support::*;

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn serve_arguments<'a>(manifest: &'a Path, store: &'a Path, seed: &'a str) -> Vec<&'a str> {
    vec![
        "serve",
        "--manifest",
        path(manifest),
        "--store",
        path(store),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        seed,
    ]
}

fn ingest(server: &Server, table: &[&[&str]]) {
    let response = server.post(
        "/v1/ingest",
        json!({
            "source": "shop",
            "entity_type": "thing",
            "key": "id",
            "rows": json_rows(rows(table)),
        }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
}

fn resolve(server: &Server) -> Value {
    server.call("/v1/runs", json!({ "entity_type": "thing" }))
}

#[test]
fn the_binary_starts_with_no_library_path_set() {
    let dir = tempfile::tempdir().unwrap();
    for arguments in [&["--version"][..], &["init", path(dir.path())]] {
        let output = Command::new(env!("CARGO_BIN_EXE_spool"))
            .env_clear()
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
    }
    // `init` opened DuckDB to check what it wrote.
    assert!(dir.path().join("entities/record/features.sql").is_file());
}

#[test]
fn init_scaffolds_a_project_that_info_lists_and_build_compiles() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("crm");

    let created = succeed(&["init", path(&project), "--entity", "customer"]);
    assert!(created.contains("entity type `customer`"), "{created}");
    for file in [
        "entities/customer/features.sql",
        "entities/customer/rules/block/same_name.sql",
        "entities/customer/rules/match/same_name.sql",
    ] {
        assert!(project.join(file).is_file(), "{file}");
    }
    assert_eq!(read(&project.join(".gitignore")), ".spool/\n");

    // A project is not scaffolded over another.
    let again = run(&["init", path(&project)]);
    assert_eq!(again.status.code(), Some(1));
    assert!(text(&again.stderr).contains("already exists"));

    let listing = succeed(&["info", path(&project)]);
    assert!(listing.contains("entity type `customer`"), "{listing}");
    assert!(
        listing.contains("features  observation_id UUID, name VARCHAR"),
        "{listing}"
    );
    assert!(listing.contains("block     same_name"), "{listing}");
    assert!(listing.contains("no_match  (none)"), "{listing}");
    let listing: Value =
        serde_json::from_str(&succeed(&["info", path(&project), "--json"])).unwrap();
    assert_eq!(listing["entity_types"][0]["name"], "customer");
    assert_eq!(
        listing["entity_types"][0]["rules"],
        json!({ "block": ["same_name"], "no_match": [], "match": ["same_name"] })
    );
    assert_eq!(listing["warnings"], json!([]));

    let built = succeed(&["build", path(&project)]);
    let manifest: Value =
        serde_json::from_str(&read(&project.join(".spool/manifest.json"))).unwrap();
    assert_eq!(manifest["hash"], listing["manifest_hash"]);
    assert!(
        built.contains(manifest["hash"].as_str().unwrap()),
        "{built}"
    );

    // The first build writes a Dockerfile, which then belongs to the project.
    let dockerfile = read(&project.join("Dockerfile"));
    assert!(
        dockerfile.contains(&format!(
            "FROM ghcr.io/alexpatow/spool:{}",
            env!("CARGO_PKG_VERSION")
        )),
        "{dockerfile}"
    );
    assert!(dockerfile.contains("RUN spool build"), "{dockerfile}");
    assert_eq!(read(&project.join(".dockerignore")), ".git\n.spool\n");
    std::fs::write(project.join("Dockerfile"), "FROM mine\n").unwrap();
    let rebuilt = succeed(&["build", path(&project)]);
    assert!(!rebuilt.contains("Dockerfile"), "{rebuilt}");
    assert_eq!(read(&project.join("Dockerfile")), "FROM mine\n");
    let refused = run(&["build", path(&project), "--base-image", "other"]);
    assert_eq!(refused.status.code(), Some(1));

    let other = dir.path().join("other");
    succeed(&["init", path(&other)]);
    succeed(&[
        "build",
        path(&other),
        "--base-image",
        "registry.example/spool:1",
    ]);
    assert!(read(&other.join("Dockerfile")).contains("FROM registry.example/spool:1 AS build"));
}

#[test]
fn a_project_that_does_not_compile_is_reported_with_its_file() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let rule = dir.path().join("entities/thing/rules/match/same_code.sql");
    std::fs::write(&rule, "l.code = r.no_such_column\n").unwrap();

    for command in ["info", "build"] {
        let output = run(&[command, path(dir.path())]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        let stderr = text(&output.stderr);
        assert!(stderr.starts_with("spool: "), "{stderr}");
        assert!(stderr.contains("same_code.sql"), "{stderr}");
        assert!(stderr.contains("no_such_column"), "{stderr}");
    }
    assert!(!dir.path().join(".spool/manifest.json").exists());
}

#[test]
fn apply_declares_types_and_rebuilds_the_features_of_a_changed_declaration() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = build(&project);
    let store = dir.path().join("store.duckdb");

    let applied = succeed(&["apply", path(&project), "--store", path(&store)]);
    assert!(applied.contains("declared  thing"), "{applied}");
    let again = succeed(&[
        "apply",
        "--manifest",
        path(&manifest),
        "--store",
        path(&store),
    ]);
    assert!(again.contains("unchanged thing"), "{again}");
    assert!(again.contains("already at this manifest"), "{again}");

    // The store path can come from the environment, as it does in an image.
    let output = spool()
        .args(["apply", path(&project)])
        .env("SPOOL_STORE", &store)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));

    // Put two observations in, then change what a thing's code is.
    let server = Server::start(&serve_arguments(&manifest, &store, "1"));
    ingest(&server, &[&["id", "code"], &["1", "anvil"]]);
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );
    server.stop();

    std::fs::write(
        project.join("entities/thing/features.sql"),
        "SELECT observation_id, lower(payload->>'code') AS code, payload->>'colour' AS colour\nFROM observation\n",
    )
    .unwrap();
    build(&project);
    let applied = succeed(&["apply", path(&project), "--store", path(&store)]);
    assert!(applied.contains("rebuilt   thing"), "{applied}");

    // Identities are untouched, and the new features are what match now.
    let server = Server::start(&serve_arguments(&manifest, &store, "2"));
    let info = server.read("/v1/info");
    assert_eq!(info["store"]["entity_types"][0]["declaration_version"], 2);
    assert_eq!(info["store"]["entity_types"][0]["identities"], 1);
    ingest(&server, &[&["id", "code"], &["2", "ANVIL"]]);
    let resolution = resolve(&server)["run"]["resolution"].clone();
    assert_eq!(resolution["identities_created"], 0, "{resolution}");
    assert_eq!(resolution["observations_assigned"], 1, "{resolution}");

    // A manifest that was edited by hand is not applied.
    let edited = read(&manifest).replace("lower(", "upper(");
    std::fs::write(&manifest, edited).unwrap();
    let output = run(&["apply", path(&project), "--store", path(&store)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("manifest.json"));
}

#[test]
fn serve_sets_up_an_empty_store_and_refuses_one_applied_with_another_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = build(&project);
    let store = dir.path().join("store.duckdb");

    // No token, no server.
    let stopped = Server::start_with(&serve_arguments(&manifest, &store, "1"), None).unwrap_err();
    assert_eq!(stopped.code, Some(1));
    assert!(stopped.output.contains("SPOOL_TOKEN"), "{}", stopped.output);
    let stopped =
        Server::start_with(&serve_arguments(&manifest, &store, "1"), Some(" ")).unwrap_err();
    assert_eq!(stopped.code, Some(1));
    assert!(!store.exists(), "nothing was opened");

    // An empty store is set up from the manifest.
    let server = Server::start(&serve_arguments(&manifest, &store, "1"));
    let hash = server.read("/v1/info")["store"]["manifest_hash"].clone();
    ingest(
        &server,
        &[&["id", "code"], &["1", "anvil"], &["2", "anvil"]],
    );
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );
    server.stop();

    // The same manifest starts again on the same store, which kept its state.
    let server = Server::start(&serve_arguments(&manifest, &store, "2"));
    let info = server.read("/v1/info");
    assert_eq!(info["store"]["manifest_hash"], hash);
    assert_eq!(info["store"]["head"], 1);
    assert_eq!(info["store"]["entity_types"][0]["identities"], 1);
    server.stop();

    // A changed project is refused until it is applied.
    std::fs::remove_file(project.join("entities/thing/rules/no_match/other_colour.sql")).unwrap();
    build(&project);
    let stopped =
        Server::start_with(&serve_arguments(&manifest, &store, "3"), Some(TOKEN)).unwrap_err();
    assert_eq!(stopped.code, Some(1));
    assert!(
        stopped.output.contains(hash.as_str().unwrap()),
        "{}",
        stopped.output
    );
    assert!(stopped.output.contains("spool apply"), "{}", stopped.output);

    succeed(&["apply", path(&project), "--store", path(&store)]);
    let server = Server::start(&serve_arguments(&manifest, &store, "3"));
    let info = server.read("/v1/info");
    assert_ne!(info["store"]["manifest_hash"], hash);
    assert_eq!(info["store"]["entity_types"][0]["identities"], 1);
    server.stop();

    // No manifest, no server.
    std::fs::remove_file(&manifest).unwrap();
    let stopped =
        Server::start_with(&serve_arguments(&manifest, &store, "4"), Some(TOKEN)).unwrap_err();
    assert_eq!(stopped.code, Some(1));
    assert!(
        stopped.output.contains("manifest.json"),
        "{}",
        stopped.output
    );
}

#[test]
fn a_run_cut_short_by_a_killed_server_is_recorded_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = build(&project);
    let store = dir.path().join("store.duckdb");

    let server = Server::start(&serve_arguments(&manifest, &store, "1"));
    ingest(&server, &[&["id", "code"], &["1", "anvil"]]);
    let ids: Vec<String> = (0..300_000).map(|index| format!("bulk-{index}")).collect();
    let bulk: Vec<Vec<Option<&str>>> = ids
        .iter()
        .map(|id| vec![Some(id.as_str()), Some(id.as_str())])
        .collect();
    let response = server.post_arrow(
        "/v1/ingest?source=bulk&entity_type=thing&key=id&wait_ms=0",
        Rows::from_text(&["id", "code"], &bulk).unwrap(),
    );
    let accepted = response.expect(202);
    assert_eq!(accepted["run"]["status"], "running");
    let run = accepted["run"]["run"].as_u64().unwrap();
    server.kill();

    let server = Server::start(&serve_arguments(&manifest, &store, "2"));
    let record = server.read(&format!("/v1/runs/{run}"));
    assert_eq!(record["run"]["status"], "failed", "{record}");
    let error = record["run"]["error"].as_str().unwrap();
    assert!(error.starts_with("interrupted"), "{error}");
    assert!(record["run"]["finished_at"].is_string());
    assert_eq!(server.read("/v1/runs?status=running")["runs"], json!([]));

    // Nothing of the batch was kept, and the store works as before.
    let info = server.read("/v1/info");
    assert_eq!(info["store"]["entity_types"][0]["observations"], 1);
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );
}

#[test]
fn dev_serves_a_project_directory_and_applies_changes_as_they_are_saved() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let features = dir.path().join("entities/thing/features.sql");
    let original = read(&features);

    let server = Server::start_with(
        &[
            "dev",
            path(dir.path()),
            "--listen",
            "127.0.0.1:0",
            "--seed",
            "1",
        ],
        None,
    )
    .unwrap();
    assert_eq!(server.token, "dev");
    assert!(dir.path().join(".spool/dev.duckdb").is_file());
    let first = server.read("/v1/info")["store"]["manifest_hash"].clone();

    ingest(&server, &[&["id", "code"], &["1", "anvil"]]);
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );

    // A saved change to the declaration is applied without a restart.
    std::fs::write(
        &features,
        original.replace("payload->>'code'", "lower(payload->>'code')"),
    )
    .unwrap();
    let applied = server.printed("applied");
    assert!(applied.contains("rebuilt thing"), "{applied}");
    let second = server.read("/v1/info")["store"]["manifest_hash"].clone();
    assert_ne!(second, first);
    assert!(applied.contains(second.as_str().unwrap()), "{applied}");
    ingest(&server, &[&["id", "code"], &["2", "ANVIL"]]);
    let resolution = resolve(&server)["run"]["resolution"].clone();
    assert_eq!(resolution["identities_created"], 0, "{resolution}");
    assert_eq!(resolution["observations_assigned"], 1, "{resolution}");

    // A change that does not compile is reported, and the server carries on
    // with what it had.
    std::fs::write(&features, "SELEC nothing\n").unwrap();
    let complaint = server.complained("features.sql");
    assert!(complaint.starts_with("spool: "), "{complaint}");
    server.complained("still running the last version that compiled");
    assert_eq!(server.read("/v1/info")["store"]["manifest_hash"], second);

    // A rule is only a rule: adding one rebuilds nothing.
    std::fs::write(
        &features,
        original.replace("payload->>'code'", "lower(payload->>'code')"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("entities/thing/rules/match/never.sql"),
        "false\n",
    )
    .unwrap();
    let applied = server.printed("applied");
    assert!(applied.contains("rules only"), "{applied}");
    let third = server.read("/v1/info")["store"]["manifest_hash"].clone();
    assert_ne!(third, second);

    server.stop();
}
