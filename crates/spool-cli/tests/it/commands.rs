//! The commands a project goes through: `init`, `info`, `build`, `apply`,
//! `serve` and `dev`, run as a user runs them.

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};
use spool::Rows;

use crate::support::*;

mod init;
mod scaffold;

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

fn load(server: &Server, table: &[&[&str]]) {
    let response = server.post(
        "/v1/load",
        json!({
            "source": "shop",
            "entity_type": "thing",
            "rows": json_rows(rows(table)),
        }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
}

fn resolve(server: &Server) -> Value {
    server.call("/v1/match", json!({ "entity_type": "thing" }))
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
    assert!(dir.path().join("entities/.gitkeep").is_file());
}

/// Every file under `dir` outside Git metadata, as paths below it with `/`
/// between their parts.
fn files_of(dir: &Path) -> Vec<String> {
    fn visit(root: &Path, dir: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if dir == root && entry.file_name() == ".git" {
                continue;
            }
            let entry = entry.path();
            if entry.is_dir() {
                visit(root, &entry, found);
            } else {
                let below: Vec<_> = entry
                    .strip_prefix(root)
                    .unwrap()
                    .iter()
                    .map(|part| part.to_string_lossy())
                    .collect();
                found.push(below.join("/"));
            }
        }
    }
    let mut found = Vec::new();
    visit(dir, dir, &mut found);
    found.sort();
    found
}

fn git_init(dir: &Path) {
    let output = Command::new("git")
        .args(["init", "--quiet", "--"])
        .arg(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
}

/// `spool init <arguments>`, run in `dir`, on a machine that has `uv`: the
/// command it is told to run Python files with is one that is there.
fn init_in(dir: &Path, arguments: &[&str]) -> std::process::Output {
    spool()
        .current_dir(dir)
        .arg("init")
        .args(arguments)
        .env("SPOOL_UV", std::env::current_exe().unwrap())
        .output()
        .unwrap()
}

/// What `spool init` writes into every project, beside what an example
/// adds.
const GITIGNORE: &str = ".spool/\n.env\n";

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
        assert!(stderr.starts_with("error: "), "{stderr}");
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
    assert!(again.contains("unchanged  thing"), "{again}");
    assert!(again.contains("already at this manifest"), "{again}");

    // The store path can come from the environment, as it does in an image.
    let output = spool()
        .args(["apply", path(&project)])
        .env("SPOOL_STORE", &store)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));

    // Put two records in, then change what a thing's code is.
    let server = Server::start(&serve_arguments(&manifest, &store, "1"));
    load(&server, &[&["id", "code"], &["1", "anvil"]]);
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );
    server.stop();

    std::fs::write(
        project.join("entities/thing/entity.yaml"),
        thing_entity("{ from: code, transform: lower }"),
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
    load(&server, &[&["id", "code"], &["2", "ANVIL"]]);
    let resolution = resolve(&server)["run"]["resolution"].clone();
    assert_eq!(resolution["identities_created"], 0, "{resolution}");
    assert_eq!(resolution["records_assigned"], 1, "{resolution}");

    // A manifest that was edited by hand is not applied.
    let edited = read(&manifest).replace("lower(", "upper(");
    std::fs::write(&manifest, edited).unwrap();
    let output = run(&["apply", path(&project), "--store", path(&store)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("manifest.json"));
}

#[test]
fn serve_sets_up_a_store_applies_what_its_tables_fit_and_refuses_a_changed_declaration() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = build(&project);
    let store = dir.path().join("store.duckdb");

    // An empty store is set up from the manifest.
    let server = Server::start(&serve_arguments(&manifest, &store, "1"));
    let hash = server.read("/v1/info")["store"]["manifest_hash"].clone();
    load(
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

    // A changed rule leaves the feature tables as they are, so the server
    // applies the new manifest itself and starts.
    std::fs::remove_file(project.join("entities/thing/rules/no_match/other_colour.sql")).unwrap();
    build(&project);
    let server = Server::start(&serve_arguments(&manifest, &store, "3"));
    let info = server.read("/v1/info");
    let ruled = info["store"]["manifest_hash"].clone();
    assert_ne!(ruled, hash);
    assert_eq!(info["store"]["head"], 1);
    assert_eq!(info["store"]["entity_types"][0]["declaration_version"], 1);
    server.stop();

    // A changed declaration computes the type's feature table again, which
    // is refused until it is applied. The refusal changes nothing.
    std::fs::write(
        project.join("entities/thing/entity.yaml"),
        thing_entity("{ from: code, transform: lower }"),
    )
    .unwrap();
    build(&project);
    let stopped = Server::start_with(&serve_arguments(&manifest, &store, "4")).unwrap_err();
    assert_eq!(stopped.code, Some(1));
    assert!(
        stopped.output.contains(ruled.as_str().unwrap()),
        "{}",
        stopped.output
    );
    assert!(stopped.output.contains("`thing`"), "{}", stopped.output);
    assert!(stopped.output.contains("spool apply"), "{}", stopped.output);

    succeed(&["apply", path(&project), "--store", path(&store)]);
    let server = Server::start(&serve_arguments(&manifest, &store, "4"));
    let info = server.read("/v1/info");
    assert_ne!(info["store"]["manifest_hash"], ruled);
    assert_eq!(info["store"]["entity_types"][0]["identities"], 1);
    server.stop();

    // No manifest, no server.
    std::fs::remove_file(&manifest).unwrap();
    let stopped = Server::start_with(&serve_arguments(&manifest, &store, "5")).unwrap_err();
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
    load(&server, &[&["id", "code"], &["1", "anvil"]]);
    let ids: Vec<String> = (0..300_000).map(|index| format!("bulk-{index}")).collect();
    let bulk: Vec<Vec<Option<&str>>> = ids
        .iter()
        .map(|id| vec![Some(id.as_str()), Some(id.as_str())])
        .collect();
    let response = server.post_arrow(
        "/v1/load?source=bulk&entity_type=thing&wait_ms=0",
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
    assert_eq!(info["store"]["entity_types"][0]["records"], 1);
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );
}

#[test]
fn dev_serves_a_project_directory_and_applies_changes_as_they_are_saved() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let entity = dir.path().join("entities/thing/entity.yaml");
    // A transform that cannot work on a code that is a word. Nothing uses it yet.
    std::fs::create_dir_all(dir.path().join("transforms")).unwrap();
    std::fs::write(
        dir.path().join("transforms/number.sql"),
        "lambda code: code::INTEGER\n",
    )
    .unwrap();
    let folded = thing_entity("{ from: code, transform: lower }");

    let server = Server::start_with(&[
        "dev",
        path(dir.path()),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ])
    .unwrap();
    assert!(dir.path().join(".spool/dev.duckdb").is_file());
    let first = server.read("/v1/info")["store"]["manifest_hash"].clone();

    load(&server, &[&["id", "code"], &["1", "anvil"]]);
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );

    // A saved change to the declaration is applied without a restart.
    std::fs::write(&entity, &folded).unwrap();
    let applied = server.printed("applied");
    assert!(applied.contains("rebuilt thing"), "{applied}");
    let second = server.read("/v1/info")["store"]["manifest_hash"].clone();
    assert_ne!(second, first);
    assert!(applied.contains(second.as_str().unwrap()), "{applied}");
    load(&server, &[&["id", "code"], &["2", "ANVIL"]]);
    let resolution = resolve(&server)["run"]["resolution"].clone();
    assert_eq!(resolution["identities_created"], 0, "{resolution}");
    assert_eq!(resolution["records_assigned"], 1, "{resolution}");

    // A change that does not compile is reported, and the server carries on
    // with what it had.
    std::fs::write(&entity, "features: [code\n").unwrap();
    let complaint = server.complained("entity.yaml");
    assert!(complaint.starts_with("error: "), "{complaint}");
    server.complained("still running the last version that compiled");
    assert_eq!(server.read("/v1/info")["store"]["manifest_hash"], second);

    // So is a change that compiles and that the stored records do not
    // fit: here, a code that has to be a number.
    std::fs::write(&entity, thing_entity("{ from: code, transform: number }")).unwrap();
    server.complained("still running the last version that compiled");
    assert_eq!(server.read("/v1/info")["store"]["manifest_hash"], second);
    load(&server, &[&["id", "code"], &["3", "Anvil"]]);
    let resolution = resolve(&server)["run"]["resolution"].clone();
    assert_eq!(resolution["records_assigned"], 1, "{resolution}");
    assert_eq!(resolution["identities_created"], 0, "{resolution}");

    // A rule is only a rule: adding one rebuilds nothing.
    std::fs::write(&entity, &folded).unwrap();
    std::fs::write(
        dir.path().join("entities/thing/rules/match/never.sql"),
        "false\n",
    )
    .unwrap();
    let applied = server.printed("applied");
    assert!(applied.contains("no table changed"), "{applied}");
    let third = server.read("/v1/info")["store"]["manifest_hash"].clone();
    assert_ne!(third, second);

    server.stop();
}

/// The records of the source `crm`, by key: each one's record ID, the name
/// it is called by, and its identity.
fn companies(server: &Server) -> std::collections::BTreeMap<String, (String, String, String)> {
    let text = |value: &Value| value.as_str().unwrap().to_owned();
    server.read("/v1/records?source=crm")["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            (
                text(&record["key"]),
                (
                    text(&record["id"]),
                    text(&record["label"]),
                    text(&record["identity"]),
                ),
            )
        })
        .collect()
}

#[test]
fn a_second_server_on_a_store_or_a_port_says_what_is_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("companies");
    succeed(&["init", path(&project)]);
    let store = project.join(".spool/dev.duckdb");
    let dev = |project: &Path, listen: &str| {
        Server::start_with(&["dev", path(project), "--listen", listen])
    };
    let running = dev(&project, "127.0.0.1:0").unwrap();

    // The same project again. DuckDB refuses the second process the store,
    // in words about a lock and a page of its own documentation.
    let stopped = dev(&project, "127.0.0.1:0").unwrap_err();
    assert_eq!(stopped.code, Some(1));
    let said = stopped.output;
    assert!(
        said.starts_with(&format!(
            "error: the store {} is open in another process",
            store.display()
        )),
        "{said}"
    );
    assert!(
        said.contains("most likely a `spool dev` that is already running on this project"),
        "{said}"
    );
    assert!(said.contains("stop it with Ctrl-C"), "{said}");
    for raw in ["IO Error", "lock", "duckdb.org"] {
        assert!(!said.contains(raw), "{said}");
    }

    // Any other command that opens the store is told the same thing.
    build(&project);
    let applied = run(&["apply", path(&project), "--store", path(&store)]);
    assert_eq!(applied.status.code(), Some(1));
    let said = text(&applied.stderr);
    assert!(
        said.starts_with(&format!(
            "error: the store at {} is open in another process",
            store.display()
        )),
        "{said}"
    );
    assert!(said.contains("stop that one first"), "{said}");
    assert!(!said.contains("duckdb.org"), "{said}");

    // Another project on the port the first one has.
    let other = dir.path().join("other");
    succeed(&["init", path(&other)]);
    let stopped = dev(&other, &running.address).unwrap_err();
    assert_eq!(stopped.code, Some(1));
    let said = stopped.output;
    let (host, port) = running.address.rsplit_once(':').unwrap();
    let next = match port.parse::<u16>().unwrap() {
        u16::MAX => u16::MAX - 1,
        port => port + 1,
    };
    let next = format!("{host}:{next}");
    assert!(
        said.contains(&format!(
            "error: cannot listen on {}: something else is listening there",
            running.address
        )),
        "{said}"
    );
    assert!(said.contains(&format!("`--listen {next}`")), "{said}");
    assert!(said.contains(&format!("SPOOL_URL=http://{next}")), "{said}");
    assert!(!said.contains("os error"), "{said}");

    // The first server is as it was, and the second project starts on a
    // port of its own.
    assert_eq!(running.read("/v1/info")["store"]["head"], 0);
    let second = dev(&other, "127.0.0.1:0").unwrap();
    assert_eq!(second.read("/v1/info")["store"]["head"], 0);
    second.stop();
    running.stop();
}
