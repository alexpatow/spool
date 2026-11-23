//! The HTTP API, called over a socket against the real binary.

use std::path::{Path, PathBuf};

use arrow::array::{Array, AsArray};
use serde_json::{Value, json};
use spool::core::Seq;
use spool::{Engine, IdentityFilter, Options, ReviewFilter, ReviewStatus, Rows};

use crate::support::*;

mod connections;
mod loading;
mod reading;
mod runs;
mod stopping;

fn companies() -> PathBuf {
    benchmarks().join("identity/companies-small")
}

/// The files of the company scenario, in the order it loads them, in two
/// steps: `(file, source)`.
const STEPS: [[(&str, &str); 2]; 2] = [
    [
        ("batch-1-crm.csv", "crm"),
        ("batch-1-registry.csv", "registry"),
    ],
    [
        ("batch-2-crm.csv", "crm"),
        ("batch-2-registry.csv", "registry"),
    ],
];

/// The company scenario run in this process, with the library.
fn library(seed: u64) -> Engine {
    let engine = Engine::open_in_memory(Options::seeded(seed)).unwrap();
    spool_bench::Scenario::load(companies())
        .unwrap()
        .run(&engine, |_| Ok(()))
        .unwrap();
    engine
}

#[derive(Clone, Copy, Debug)]
enum Encoding {
    Arrow,
    Json,
}

/// A server on the company project and an empty store.
fn company_server(dir: &Path, seed: u64) -> Server {
    let project = dir.join("project");
    copy_project(&companies(), &project);
    Server::serve(&project, dir, seed)
}

/// The company scenario run through the server: the same calls the library
/// makes, in the same order.
fn run_companies(server: &Server, encoding: Encoding) {
    for step in STEPS {
        for (file, source) in step {
            let rows = Rows::from_csv(companies().join(file)).unwrap();
            let response = match encoding {
                Encoding::Arrow => server.post_arrow(
                    &format!("/v1/load?source={source}&entity_type=company"),
                    rows,
                ),
                Encoding::Json => server.post(
                    "/v1/load",
                    json!({
                        "source": source,
                        "entity_type": "company",
                        "rows": json_rows(rows),
                    }),
                ),
            };
            assert_eq!(response.ok()["run"]["status"], "succeeded");
        }
        let run = server.call("/v1/match", json!({ "entity_type": "company" }));
        assert_eq!(run["run"]["status"], "succeeded", "{run}");
    }
}

fn hashes(log: &Value) -> Vec<String> {
    log["changesets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["content_hash"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_same_seed_gives_the_same_changesets_through_the_library_and_the_server() {
    let library = library(11);
    let expected: Vec<String> = library
        .log_after(Seq(0), 1_000)
        .unwrap()
        .iter()
        .map(|entry| entry.content_hash.to_string())
        .collect();
    assert_eq!(expected.len(), 2, "one changeset per step");

    for encoding in [Encoding::Arrow, Encoding::Json] {
        let dir = tempfile::tempdir().unwrap();
        let server = company_server(dir.path(), 11);
        run_companies(&server, encoding);

        let log = server.read("/v1/changesets");
        assert_eq!(hashes(&log), expected, "{encoding:?}");
        assert_eq!(log["head"], 2);
        // The entries are the same but for who started the runs: the token
        // here, and nobody in the library.
        let mut entries =
            serde_json::to_value(library.logged_after(Seq(0), 1_000).unwrap()).unwrap();
        for entry in entries.as_array_mut().unwrap() {
            assert_eq!(entry["started_by"], "engine");
            entry["started_by"] = json!("agent:api");
        }
        assert_eq!(log["changesets"], entries, "{encoding:?}");

        // The state the log leaves is the same too.
        let identities = server.read("/v1/identities?limit=1000");
        assert_eq!(
            identities["identities"],
            serde_json::to_value(library.identities(&IdentityFilter::default()).unwrap()).unwrap(),
            "{encoding:?}"
        );
        assert_eq!(
            server.read("/v1/info")["store"],
            serde_json::to_value(library.info().unwrap()).unwrap(),
            "{encoding:?}"
        );
    }

    // Another seed gives other IDs, and so other hashes.
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 12);
    run_companies(&server, Encoding::Arrow);
    assert_ne!(hashes(&server.read("/v1/changesets")), expected);
}

/// How long the servers in this process answer what they have once they are
/// told to stop. `spool serve` gives it longer.
const STOP_WITHIN: std::time::Duration = std::time::Duration::from_millis(500);

/// The same server, which also stops when `shutdown` completes, and what
/// its serving ends with.
fn small_bodied_server_until(
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> (
    tokio::runtime::Runtime,
    String,
    String,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    use spool_cli::server::{ServerOptions, serve};

    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let accounts = spool_cli::auth::Accounts::in_memory();
    let (_, token) = accounts.add_token("api", None).unwrap();
    let options = ServerOptions {
        max_body_bytes: 1_000,
        stop_within: STOP_WITHIN,
        ..ServerOptions::new(accounts)
    };
    let serving = runtime.spawn(async move {
        serve(listener, std::sync::Arc::new(engine), &options, shutdown).await
    });
    (runtime, address, token, serving)
}

fn load_things(server: &Server, source: &str, table: &[&[&str]]) {
    let response = server.post(
        "/v1/load",
        json!({
            "source": source,
            "entity_type": "thing",
            "rows": json_rows(rows(table)),
        }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
}

fn two_anvils_and_a_hammer(server: &Server) {
    load_things(
        server,
        "shop",
        &[
            &["id", "code"],
            &["1", "anvil"],
            &["2", "anvil"],
            &["3", "hammer"],
        ],
    );
}

fn unassigned(server: &Server) -> usize {
    server.read("/v1/records?unassigned=true")["records"]
        .as_array()
        .unwrap()
        .len()
}

fn things_server(dir: &Path) -> Server {
    let project = dir.join("project");
    write_things(&project);
    Server::serve(&project, dir, 1)
}

/// The keys and payloads of a source's records, in the order they are
/// listed.
fn held(server: &Server, source: &str) -> Vec<(Value, Value)> {
    server.read(&format!("/v1/records?source={source}"))["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| (record["key"].clone(), record["payload"].clone()))
        .collect()
}
