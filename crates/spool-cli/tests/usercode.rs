//! A project's user-code strategies through the commands and the server.
//!
//! The tests that start `uv` are ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool-cli --test usercode -- --include-ignored
//! ```

mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::*;

/// Judges two things the same if their code is the same, whatever their
/// colour: the project's rules keep two colours apart, so the two disagree.
const BY_CODE: &str = r#"
from spool import strategy


@strategy
def judge(candidates):
    return [
        {"left_id": pair["left_id"], "right_id": pair["right_id"], "verdict": "match", "score": 1.0}
        for pair in candidates.to_pylist()
        if pair["left_code"] == pair["right_code"]
    ]
"#;

const BROKEN: &str = r#"
from spool import strategy


@strategy
def judge(candidates):
    raise RuntimeError("the model is missing")
"#;

fn write_strategy(project: &Path, file: &str, source: &str) {
    let path = project.join("entities/thing/strategies").join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, source).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
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

fn thing(server: &Server) -> Value {
    server.read("/v1/info")["store"]["entity_types"][0].clone()
}

#[test]
fn info_lists_strategies_and_build_writes_a_dockerfile_that_can_run_them() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_things(project);
    write_strategy(project, "by_code.py", BY_CODE);

    let listing = succeed(&["info", path(project)]);
    assert!(
        listing.contains("strategy  by_code (python, entities/thing/strategies/by_code.py)"),
        "{listing}"
    );
    let listing: Value =
        serde_json::from_str(&succeed(&["info", path(project), "--json"])).unwrap();
    let strategies = listing["entity_types"][0]["strategies"].as_array().unwrap();
    assert_eq!(strategies.len(), 1);
    assert_eq!(strategies[0]["name"], "by_code");
    assert_eq!(strategies[0]["runtime"], "python");
    assert_eq!(
        strategies[0]["file"],
        "entities/thing/strategies/by_code.py"
    );

    succeed(&["build", path(project)]);
    let manifest: Value =
        serde_json::from_str(&read(&project.join(".spool/manifest.json"))).unwrap();
    assert_eq!(
        manifest["entity_types"][0]["strategies"][0]["hash"],
        strategies[0]["hash"]
    );

    // The image of a project with user code holds the project's files and
    // what runs them, and only the runtimes the project uses.
    let dockerfile = read(&project.join("Dockerfile"));
    for line in [
        "COPY --from=ghcr.io/astral-sh/uv:",
        "COPY --from=build --chown=spool:spool /project /spool/project",
        "WORKDIR /spool/project",
        "RUN spool build --install",
        "ENV UV_OFFLINE=1",
        "CMD [\"serve\", \"--listen\", \"0.0.0.0:7765\"]",
    ] {
        assert!(dockerfile.contains(line), "no `{line}` in:\n{dockerfile}");
    }
    assert!(!dockerfile.contains("bun"), "{dockerfile}");

    let other = dir.path().join("other");
    write_things(&other);
    write_strategy(&other, "by_code.ts", "export {};\n");
    succeed(&["build", path(&other)]);
    let dockerfile = read(&other.join("Dockerfile"));
    assert!(dockerfile.contains("COPY --from=oven/bun:"), "{dockerfile}");
    assert!(!dockerfile.contains("uv"), "{dockerfile}");
    assert!(!dockerfile.contains("UV_OFFLINE"), "{dockerfile}");
}

#[test]
#[ignore = "runs uv"]
fn build_with_install_starts_each_strategy_and_reports_one_that_does_not_start() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    write_things(project);
    write_strategy(project, "by_code.py", BY_CODE);

    let built = succeed(&["build", path(project), "--install"]);
    assert!(
        built.contains("starts    entities/thing/strategies/by_code.py"),
        "{built}"
    );

    write_strategy(project, "unfinished.py", "def judge(:\n");
    let refused = run(&["build", path(project), "--install"]);
    assert_eq!(refused.status.code(), Some(1));
    let complaint = text(&refused.stderr);
    assert!(
        complaint.contains("strategy `unfinished`: it does not start"),
        "{complaint}"
    );
    assert!(complaint.contains("SyntaxError"), "{complaint}");
}

#[test]
#[ignore = "runs uv"]
#[expect(clippy::too_many_lines)]
fn the_server_runs_the_projects_strategies_and_no_other_code() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    write_strategy(&project, "by_code.py", BY_CODE);
    write_strategy(&project, "broken.py", BROKEN);
    let manifest = build(&project);
    let tasks = dir.path().join("tasks");
    let server = Server::start(&[
        "serve",
        "--manifest",
        path(&manifest),
        "--project",
        path(&project),
        "--tasks",
        path(&tasks),
        "--store",
        path(&dir.path().join("store.duckdb")),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ]);

    let strategies = thing(&server)["strategies"].clone();
    let names: Vec<&str> = strategies
        .as_array()
        .unwrap()
        .iter()
        .map(|strategy| strategy["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["broken", "by_code"]);
    let by_code = strategies[1].clone();
    assert_eq!(by_code["runtime"], "python");

    // The rules keep the blue anvil apart from the red ones.
    load(
        &server,
        &[
            &["id", "code", "colour"],
            &["1", "anvil", "red"],
            &["2", "anvil", "red"],
            &["3", "anvil", "blue"],
        ],
    );
    let run = server.call("/v1/match", json!({ "entity_type": "thing" }))["run"].clone();
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(thing(&server)["identities"], 2);

    // A rematch with the strategy says where it disagrees, and changes
    // nothing.
    let head = server.read("/v1/info")["store"]["head"].clone();
    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "thing", "strategy": { "name": "by_code" }, "rematch": true }),
    )["run"]
        .clone();
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(run["strategy"]["name"], "by_code");
    assert_eq!(run["strategy"]["version"], by_code["hash"]);
    let suggestions = &run["resolution"]["suggestions"];
    assert_eq!(suggestions["splits"], json!([]), "{run}");
    let merges = suggestions["merges"].as_array().unwrap();
    assert_eq!(merges.len(), 1, "{run}");
    assert_eq!(merges[0]["matches"], 2);
    assert_eq!(merges[0]["no_matches"], 0);
    assert_eq!(thing(&server)["identities"], 2);
    assert_eq!(server.read("/v1/info")["store"]["head"], head);

    // A version that is not the one the project was built with is refused.
    load(
        &server,
        &[
            &["id", "code", "colour"],
            &["4", "hammer", "red"],
            &["5", "hammer", "green"],
        ],
    );
    let refused = server
        .post(
            "/v1/match",
            json!({ "entity_type": "thing", "strategy": { "name": "by_code", "version": "1" } }),
        )
        .error(400, "invalid");
    assert!(refused.contains("not 1"), "{refused}");

    // A strategy that fails is a failed run, and nothing changed.
    let failed = server
        .post(
            "/v1/match",
            json!({ "entity_type": "thing", "strategy": { "name": "broken" } }),
        )
        .error(500, "strategy_failed");
    assert!(failed.contains("strategy `broken`"), "{failed}");
    assert!(failed.contains("the model is missing"), "{failed}");
    let runs = server.read("/v1/runs?status=failed")["runs"].clone();
    assert_eq!(runs.as_array().unwrap().len(), 1, "{runs}");
    assert_eq!(runs[0]["strategy"]["name"], "broken");
    assert!(
        runs[0]["error"]
            .as_str()
            .unwrap()
            .contains("the model is missing"),
        "{runs}"
    );
    assert_eq!(thing(&server)["unassigned"], 2);

    // The strategy joins the two hammers the rules would have kept apart.
    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "thing", "strategy": { "name": "by_code" } }),
    )["run"]
        .clone();
    assert_eq!(run["status"], "succeeded", "{run}");
    assert_eq!(run["resolution"]["identities_created"], 1, "{run}");
    assert_eq!(run["resolution"]["suggestions"], Value::Null, "{run}");
    assert_eq!(thing(&server)["identities"], 3);

    // A name the project does not have starts a run that waits for the
    // caller's judgements: the server runs nothing for it.
    load(&server, &[&["id", "code", "colour"], &["6", "saw", "red"]]);
    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "thing", "strategy": { "name": "mine" } }),
    )["run"]
        .clone();
    assert_eq!(run["status"], "open", "{run}");
    let cancelled = server.call(
        &format!("/v1/runs/{}/cancel", run["run"].as_u64().unwrap()),
        json!({}),
    );
    assert_eq!(cancelled["run"]["error"], "cancelled", "{cancelled}");

    // A file that is no longer the one the manifest was compiled from is
    // not run.
    write_strategy(&project, "by_code.py", &format!("{BY_CODE}\n# edited\n"));
    let changed = server
        .post(
            "/v1/match",
            json!({ "entity_type": "thing", "strategy": { "name": "by_code" } }),
        )
        .error(500, "strategy_failed");
    assert!(changed.contains("has changed since"), "{changed}");
    assert_eq!(thing(&server)["unassigned"], 1);

    // A token of the server is not something a strategy can read: no task
    // was given it.
    let token = server.token().as_bytes();
    for entry in walk(&tasks) {
        if entry.is_file() {
            let contents = std::fs::read(&entry).unwrap();
            assert!(
                !contents.windows(token.len()).any(|window| window == token),
                "{}",
                entry.display()
            );
        }
    }
    server.stop();
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(walk(&path));
        }
        found.push(path);
    }
    found
}

#[test]
#[ignore = "runs uv"]
fn dev_runs_a_strategy_as_it_was_last_saved() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    write_strategy(dir.path(), "by_code.py", BY_CODE);
    let server = Server::start_with(&[
        "dev",
        path(dir.path()),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
        "--tasks",
        path(&dir.path().join("tasks")),
    ])
    .unwrap();
    let first = thing(&server)["strategies"][0]["hash"].clone();

    load(
        &server,
        &[
            &["id", "code", "colour"],
            &["1", "anvil", "red"],
            &["2", "anvil", "blue"],
        ],
    );
    let start = json!({ "entity_type": "thing", "strategy": { "name": "by_code" } });
    let run = server.call("/v1/match", start.clone())["run"].clone();
    assert_eq!(run["resolution"]["identities_created"], 1, "{run}");

    // The saved file is the strategy from then on.
    write_strategy(
        dir.path(),
        "by_code.py",
        &BY_CODE.replace("\"match\"", "\"no_match\""),
    );
    server.printed("applied");
    let second = thing(&server)["strategies"][0]["hash"].clone();
    assert_ne!(second, first);
    load(
        &server,
        &[&["id", "code", "colour"], &["3", "anvil", "red"]],
    );
    let run = server.call("/v1/match", start)["run"].clone();
    assert_eq!(run["strategy"]["version"], second);
    assert_eq!(run["resolution"]["identities_created"], 1, "{run}");
    assert_eq!(thing(&server)["identities"], 2);
    server.stop();
}
