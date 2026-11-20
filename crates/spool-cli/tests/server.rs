//! The HTTP API, called over a socket against the real binary.

mod support;

use std::path::{Path, PathBuf};

use arrow::array::{Array, AsArray};
use serde_json::{Value, json};
use spool::core::Seq;
use spool::{Engine, IdentityFilter, Options, ReviewFilter, ReviewStatus, Rows, Scenario};
use support::*;

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
    Scenario::load(companies())
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

#[test]
fn every_call_under_v1_needs_a_caller_the_server_knows() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    // Whether the server is up is not a secret.
    let health = server
        .request("GET", "/healthz")
        .without("Authorization")
        .send();
    assert_eq!(health.ok(), json!({ "status": "ok" }));

    let token = server.token();
    for path in ["/v1/info", "/v1/identities", "/v1/no-such-path"] {
        for authorization in [
            None,
            Some("Bearer wrong".to_owned()),
            Some("Bearer api".to_owned()),
            Some(format!("Bearer {token}-and-more")),
            Some(format!("Bearer {}", &token[..token.len() - 1])),
            Some(format!("Bearer {}", token.to_uppercase())),
            Some(format!("Basic {token}")),
            Some(token.to_owned()),
            Some("Bearer ".to_owned()),
        ] {
            let request = server.request("GET", path).without("Authorization");
            let response = match &authorization {
                Some(value) => request.header("Authorization", value),
                None => request,
            }
            .send();
            response.error(401, "unauthorized");
            assert_eq!(response.header("www-authenticate"), Some("Bearer"));
        }
    }
    let refused = server
        .request("POST", "/v1/load")
        .without("Authorization")
        .json(&json!({ "source": "s", "entity_type": "thing", "rows": [{ "code": "a" }] }))
        .send();
    refused.error(401, "unauthorized");
    assert_eq!(
        server.read("/v1/info")["store"]["entity_types"][0]["records"],
        0
    );

    // The scheme's name is not case-sensitive.
    let response = server
        .request("GET", "/v1/info")
        .header("Authorization", &format!("bearer {token}"))
        .send();
    assert_eq!(response.ok()["api"], "v1");
}

#[test]
fn failures_are_described_the_same_way_whatever_went_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    server.get("/v1/no-such-path").error(404, "not_found");
    server.get("/elsewhere").error(404, "not_found");
    server.get("/v1/runs/999").error(404, "not_found");
    server
        .get("/v1/identities/00000000-0000-7000-8000-000000000000")
        .error(404, "not_found");
    server
        .get("/v1/records/00000000-0000-7000-8000-000000000000")
        .error(404, "not_found");
    server
        .get("/v1/drafts/00000000-0000-7000-8000-000000000000")
        .error(404, "not_found");

    let message = server.get("/v1/runs/first").error(400, "invalid");
    assert!(message.contains("`first` is not a run ID"), "{message}");
    server.get("/v1/identities/nonsense").error(400, "invalid");
    server.get("/v1/runs?status=asleep").error(400, "invalid");
    server.get("/v1/runs?stauts=open").error(400, "invalid");
    server
        .request("DELETE", "/v1/runs")
        .send()
        .error(405, "invalid");

    let message = server
        .request("POST", "/v1/match")
        .body(b"{ not json")
        .send()
        .error(400, "invalid");
    assert!(message.contains("JSON"), "{message}");
    // A misspelt field is refused, not ignored: `comit` would otherwise commit.
    let message = server
        .post(
            "/v1/match",
            json!({ "entity_type": "thing", "comit": false }),
        )
        .error(400, "invalid");
    assert!(message.contains("comit"), "{message}");
    let message = server
        .post("/v1/match", json!({ "entity_type": "gadget" }))
        .error(400, "invalid");
    assert!(message.contains("`gadget` is not declared"), "{message}");
    let message = server
        .post(
            "/v1/load",
            json!({ "source": "s", "entity_type": "thing", "rows": [{ "code": { "nested": 1 } }] }),
        )
        .error(400, "invalid");
    assert!(message.contains("code"), "{message}");
    // An integer that would be stored as another number is refused.
    let message = server
        .post(
            "/v1/load",
            json!({
                "source": "s",
                "entity_type": "thing",
                "rows": [{ "weight": 0.5 }, { "weight": 9007199254740993u64 }],
            }),
        )
        .error(400, "invalid");
    assert!(message.contains("row 2"), "{message}");
    // A row that says nothing is still a row, and is counted.
    let report = server.call(
        "/v1/load",
        json!({ "source": "s", "entity_type": "thing", "rows": [{}, { "name": null }] }),
    );
    assert_eq!(
        (
            &report["run"]["load"]["received"],
            &report["run"]["load"]["inserted"]
        ),
        (&json!(2), &json!(1))
    );
    server
        .request("POST", "/v1/load?source=s&entity_type=thing")
        .header("Content-Type", ARROW)
        .body(b"not an arrow stream")
        .send()
        .error(400, "invalid");
    server
        .post("/v1/ids", json!({ "identities": 10_001 }))
        .error(400, "invalid");

    // A changeset that breaks a rule comes back with the operations at fault.
    let minted = server.call("/v1/ids", json!({ "changesets": 1 }));
    let response = server.post(
        "/v1/changesets",
        json!({
            "id": minted["changesets"][0],
            "base": 0,
            "reason": null,
            "run": null,
            "ops": [
                { "op": "create_identity", "identity": "00000000-0000-7000-8000-000000000001", "entity_type": "thing" },
                { "op": "assign", "record": "00000000-0000-7000-8000-000000000002", "from": null, "to": "00000000-0000-7000-8000-000000000001" },
            ],
        }),
    );
    response.error(409, "rejected");
    let rejections = &response.json()["error"]["rejections"];
    assert_eq!(rejections.as_array().unwrap().len(), 1, "{rejections}");
    assert_eq!(rejections[0]["op_index"], 1, "{rejections}");
    assert_eq!(server.read("/v1/changesets")["head"], 0);
}

/// A server in this process that reads bodies of up to 1,000 bytes, its
/// address, and a token it takes. It stops when the runtime is dropped.
fn small_bodied_server() -> (tokio::runtime::Runtime, String, String) {
    let (runtime, address, token, _) = small_bodied_server_until(std::future::pending());
    (runtime, address, token)
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
    let accounts = spool_cli::accounts::Accounts::in_memory();
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

#[test]
fn a_body_over_the_limit_is_refused() {
    let (_runtime, address, token) = small_bodied_server();
    let send = |body: &Value| {
        Request::to(&address, "POST", "/v1/ids")
            .header("Authorization", &format!("Bearer {token}"))
            .json(body)
            .send()
    };
    assert_eq!(
        send(&json!({ "identities": 1 })).ok()["identities"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let padded = json!({ "identities": 1, "padding": "x".repeat(2_000) });
    send(&padded).error(413, "too_large");
}

/// Reads one response from a connection that stays open: its head, then the
/// body its `Content-Length` counts.
fn read_response(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;

    let mut raw = Vec::new();
    loop {
        let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
        if let Some(head) = text.find("\r\n\r\n") {
            let length: usize = text[..head]
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .expect("the response says how long it is")
                .trim()
                .parse()
                .unwrap();
            if raw.len() >= head + 4 + length {
                return text;
            }
        }
        let mut some = [0; 4096];
        let read = stream.read(&mut some).unwrap();
        assert!(read > 0, "the server closed the connection: {text}");
        raw.extend_from_slice(&some[..read]);
    }
}

#[test]
fn a_client_is_told_when_its_connection_will_not_take_another_request() {
    use std::io::Write;

    let (_runtime, address, token) = small_bodied_server();
    let bearer = format!("Bearer {token}");
    // The server answers these without reading the megabyte to its end, and
    // what is left of it is in the way of the next request. A client that is
    // not told sends its next request into a connection that is gone.
    let long = "x".repeat(1_000_000);
    for (path, authorization, body, status, code) in [
        (
            "/auth/sign-in",
            None,
            json!({ "username": "ada", "password": long }),
            413,
            "too_large",
        ),
        (
            "/v1/ids",
            Some(&bearer),
            json!({ "identities": 1, "padding": long }),
            413,
            "too_large",
        ),
        (
            "/v1/ids",
            None,
            json!({ "identities": 1, "padding": long }),
            401,
            "unauthorized",
        ),
    ] {
        let mut request = Request::to(&address, "POST", path).json(&body).kept();
        if let Some(authorization) = authorization {
            request = request.header("Authorization", authorization);
        }
        let response = request.send();
        response.error(status, code);
        assert_eq!(
            response.header("connection"),
            Some("close"),
            "{path} answered {status}"
        );
    }

    // A small request leaves the connection open, whether it is answered or
    // refused before its body is read.
    let mut stream = std::net::TcpStream::connect(&address).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    for (authorization, status) in [(bearer.as_str(), "200"), ("Bearer made-up", "401")] {
        for _ in 0..2 {
            let body = r#"{"identities":1}"#;
            let request = format!(
                "POST /v1/ids HTTP/1.1\r\nHost: {address}\r\nAuthorization: {authorization}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(request.as_bytes()).unwrap();
            let response = read_response(&mut stream);
            assert!(
                response.starts_with(&format!("http/1.1 {status}")),
                "{response}"
            );
            assert!(!response.contains("connection: close"), "{response}");
        }
    }
}

#[test]
fn a_client_refused_while_it_is_still_writing_reads_why() {
    let (_runtime, address, token) = small_bodied_server();
    let bearer = format!("Bearer {token}");
    // More than a connection holds on its way, so the client is still
    // writing when the server answers, and reads the answer only once it has
    // written everything. A server that closed as soon as it had answered
    // would stop the client in the middle of writing. It would also reset
    // the connection, and some systems then throw away an answer that had
    // arrived and was not yet read. So the server goes on reading, and the
    // client writes to the end, reads the answer, and finds no reset.
    let long = "x".repeat(16_000_000);
    for (path, authorization, body, status, code) in [
        (
            "/auth/sign-in",
            None,
            json!({ "username": "ada", "password": long }),
            413,
            "too_large",
        ),
        (
            "/v1/ids",
            Some(&bearer),
            json!({ "identities": 1, "padding": long }),
            413,
            "too_large",
        ),
        (
            "/v1/ids",
            None,
            json!({ "identities": 1, "padding": long }),
            401,
            "unauthorized",
        ),
    ] {
        // Once as a client that keeps its connections, and once as one that
        // said it would close this one.
        for kept in [true, false] {
            let mut request = Request::to(&address, "POST", path).json(&body);
            if kept {
                request = request.kept();
            }
            if let Some(authorization) = authorization {
                request = request.header("Authorization", authorization);
            }
            request.send().error(status, code);
        }
    }
}

#[test]
fn a_client_that_was_refused_and_does_not_go_away_does_not_keep_the_server_from_stopping() {
    use std::io::{Read, Write};

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (runtime, address, _, serving) = small_bodied_server_until(async {
        let _ = stopped.await;
    });
    // A megabyte is announced and refused. The client reads why, and then
    // neither sends the megabyte nor closes, so the server is still waiting
    // to read out the connection.
    let mut stream = std::net::TcpStream::connect(&address).unwrap();
    let head = format!(
        "POST /v1/ids HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 1000000\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let answer = answer.to_ascii_lowercase();
    assert!(answer.starts_with("http/1.1 401"), "{answer}");
    assert!(answer.contains("connection: close"), "{answer}");

    let asked = std::time::Instant::now();
    stop.send(()).unwrap();
    runtime.block_on(serving).unwrap().unwrap();
    let took = asked.elapsed();
    assert!(
        took < std::time::Duration::from_secs(3),
        "the server took {took:?} to stop"
    );
    drop(stream);
}

/// How long a server in this process takes to stop once it is told to,
/// with whatever `leave` left on its connections.
fn time_to_stop<T>(leave: impl FnOnce(&str, &str) -> T) -> std::time::Duration {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (runtime, address, token, serving) = small_bodied_server_until(async {
        let _ = stopped.await;
    });
    let left = leave(&address, &token);
    // The server has the connections and what was sent on them.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let asked = std::time::Instant::now();
    stop.send(()).unwrap();
    runtime.block_on(serving).unwrap().unwrap();
    let took = asked.elapsed();
    drop(left);
    took
}

#[test]
fn a_client_that_leaves_a_request_unfinished_does_not_keep_the_server_from_stopping() {
    use std::io::Write;
    use std::net::TcpStream;
    use std::time::Duration;

    let within = STOP_WITHIN + Duration::from_secs(2);
    // Half of a request's head, and then nothing.
    let took = time_to_stop(|address, _| {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(stream, "POST /v1/ids HTTP/1.1\r\nHost: {address}\r\n").unwrap();
        stream
    });
    assert!(took < within, "the server took {took:?} to stop");
    // The rest of the request had that long to arrive and be answered.
    assert!(
        took >= STOP_WITHIN,
        "the server stopped after {took:?}, without waiting for the request"
    );

    // A body that is announced to a call that reads it, and never sent.
    let took = time_to_stop(|address, token| {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "POST /v1/ids HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\nContent-Length: 100\r\n\r\n"
        )
        .unwrap();
        stream
    });
    assert!(took < within, "the server took {took:?} to stop");

    // A connection with nothing on it, and one whose request was answered,
    // do not make the server wait at all.
    let took = time_to_stop(|address, token| {
        let idle = TcpStream::connect(address).unwrap();
        let mut answered = TcpStream::connect(address).unwrap();
        write!(
            answered,
            "POST /v1/ids HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\n\
             Content-Type: application/json\r\nContent-Length: 16\r\n\r\n{{\"identities\":1}}"
        )
        .unwrap();
        (idle, answered)
    });
    assert!(
        took < Duration::from_secs(1),
        "the server took {took:?} to stop"
    );
}

/// Many rows of the `thing` type, each with a code of its own.
fn many_things(count: usize) -> Rows {
    let ids: Vec<String> = (0..count).map(|index| index.to_string()).collect();
    let rows: Vec<Vec<Option<&str>>> = ids
        .iter()
        .map(|id| vec![Some(id.as_str()), Some(id.as_str())])
        .collect();
    Rows::from_text(&["id", "code"], &rows).unwrap()
}

#[test]
fn a_call_that_outlasts_its_wait_answers_202_and_the_run_is_read_until_it_ends() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    let response = server.post_arrow(
        "/v1/load?source=bulk&entity_type=thing&wait_ms=0",
        many_things(30_000),
    );
    let accepted = response.expect(202);
    let run = accepted["run"]["run"].as_u64().unwrap();
    assert_eq!(accepted["run"]["kind"], "load");
    assert_eq!(accepted["run"]["status"], "running", "{accepted}");
    assert_eq!(accepted["run"]["finished_at"], Value::Null);

    // Reading does not wait for the writer: the run in progress is visible,
    // and so is committed state.
    assert_eq!(
        server.read("/v1/runs?status=running")["runs"][0]["run"],
        run
    );
    assert_eq!(server.read("/v1/info")["store"]["head"], 0);

    // A read with `wait_ms` answers when the run has ended.
    let finished = server.read(&format!("/v1/runs/{run}?wait_ms=120000"));
    assert_eq!(finished["run"]["status"], "succeeded", "{finished}");
    assert_eq!(finished["run"]["load"]["inserted"], 30_000);
    assert_eq!(server.read("/v1/runs?status=running")["runs"], json!([]));

    // The same for a resolve: with no wait it answers at once with the match
    // run, which goes on to commit.
    let response = server.post("/v1/match", json!({ "entity_type": "thing", "wait_ms": 0 }));
    let accepted = response.expect(202);
    let run = accepted["run"]["run"].as_u64().unwrap();
    assert_eq!(accepted["run"]["kind"], "match");
    assert_eq!(accepted["run"]["status"], "running", "{accepted}");
    let finished = server.read(&format!("/v1/runs/{run}?wait_ms=120000"));
    assert_eq!(finished["run"]["status"], "succeeded", "{finished}");
    assert_eq!(finished["run"]["resolution"]["identities_created"], 30_000);
    assert_eq!(finished["run"]["resolution"]["committed"]["seq"], 1);
    assert_eq!(server.read("/v1/info")["store"]["head"], 1);

    // A call that is given the time answers 200 with the finished run.
    let response = server.post_arrow("/v1/load?source=more&entity_type=thing", many_things(10));
    assert_eq!(response.ok()["run"]["status"], "succeeded");
}

#[test]
fn a_server_told_to_stop_answers_whoever_waits_on_a_run_and_finishes_the_run() {
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);
    let (address, token) = (server.address.clone(), server.token().to_owned());

    // A load that takes a while, which nobody waits for, and one client
    // that asks to be told when it is done, however long that takes.
    let response = server.post_arrow(
        "/v1/load?source=bulk&entity_type=thing&wait_ms=0",
        many_things(250_000),
    );
    let run = response.expect(202)["run"]["run"].as_u64().unwrap();
    let waiting = std::thread::spawn(move || {
        let response = Request::to(&address, "GET", &format!("/v1/runs/{run}?wait_ms=600000"))
            .header("Authorization", &format!("Bearer {token}"))
            .send();
        (response.ok(), Instant::now())
    });
    std::thread::sleep(Duration::from_millis(200));

    let asked = Instant::now();
    server.ask_to_stop();
    // Whoever waits on a run is answered when the server is told to stop,
    // with the run as far as it has got.
    let (answer, answered) = waiting.join().unwrap();
    let took = answered.duration_since(asked);
    assert!(took < Duration::from_secs(2), "the answer took {took:?}");
    assert_eq!(answer["run"]["run"], run);
    assert_eq!(answer["run"]["status"], "running", "{answer}");

    // The run is finished before the process ends: the store opens again
    // and holds what was loaded.
    let lines = server.stopped();
    assert_eq!(lines.last().map(String::as_str), Some("stopped"));
    let server = Server::serve(&project, dir.path(), 1);
    let finished = server.read(&format!("/v1/runs/{run}"));
    assert_eq!(finished["run"]["status"], "succeeded", "{finished}");
    assert_eq!(finished["run"]["load"]["inserted"], 250_000);
}

#[test]
fn a_server_told_to_stop_gives_an_unfinished_request_the_time_it_says_and_no_longer() {
    use std::io::Write;
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);
    let address = server.address.clone();

    // A client sent half of a request and nothing more.
    let mut unfinished = std::net::TcpStream::connect(&address).unwrap();
    write!(unfinished, "POST /v1/ids HTTP/1.1\r\nHost: {address}\r\n").unwrap();
    std::thread::sleep(Duration::from_millis(300));

    let asked = Instant::now();
    let lines = server.stopped();
    let took = asked.elapsed();
    let given = spool_cli::server::STOP_WITHIN;
    assert!(took >= given, "the server stopped after {took:?}");
    assert!(
        took < given + Duration::from_secs(3),
        "the server took {took:?} to stop"
    );
    let last: Vec<&str> = lines.iter().rev().take(2).map(String::as_str).collect();
    assert_eq!(last, ["stopped", "stopping"]);
    drop(unfinished);
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

#[test]
fn a_draft_left_for_review_is_stored_listed_and_committed_when_approved() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);

    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "thing", "commit": false }),
    );
    let run = &run["run"];
    assert_eq!(run["status"], "drafted", "{run}");
    assert_eq!(run["finished_at"], Value::Null);
    assert_eq!(run["resolution"]["committed"], Value::Null);
    let draft = run["resolution"]["draft"].as_str().unwrap().to_owned();

    // Nothing is committed yet.
    assert_eq!(server.read("/v1/changesets")["head"], 0);
    assert_eq!(unassigned(&server), 3);
    let info = server.read("/v1/info");
    assert_eq!(
        info["store"]["entity_types"][0]["open_draft"],
        draft.as_str()
    );

    let open = server.read("/v1/drafts?status=open&entity_type=thing");
    let [summary] = &open["drafts"].as_array().unwrap()[..] else {
        panic!("one open draft: {open}");
    };
    assert_eq!(summary["id"], draft.as_str());
    assert_eq!(summary["match_run"], run["run"]);
    assert_eq!(summary["identities_created"], 2);
    assert_eq!(summary["records_assigned"], 3);

    // The draft can be read whole before it is decided.
    let record = server.read(&format!("/v1/drafts/{draft}"));
    assert_eq!(record["draft"], *summary);
    assert_eq!(record["changeset"]["id"], draft.as_str());
    assert_eq!(record["changeset"]["ops"].as_array().unwrap().len(), 5);

    let committed = server.call(&format!("/v1/drafts/{draft}/approve"), json!({}));
    assert_eq!(committed["seq"], 1);
    assert_eq!(committed["changeset"], draft.as_str());
    assert_eq!(unassigned(&server), 0);
    assert_eq!(server.read("/v1/drafts?status=open")["drafts"], json!([]));
    let record = server.read(&format!("/v1/drafts/{draft}"));
    assert_eq!(record["draft"]["status"], "committed");
    assert_eq!(record["draft"]["committed_seq"], 1);

    // The run that drafted it has now succeeded, and reports the commit.
    let run = server.read(&format!("/v1/runs/{}", run["run"]));
    assert_eq!(run["run"]["status"], "succeeded");
    assert_eq!(run["run"]["resolution"]["committed"], committed);

    // It cannot be decided twice.
    server
        .post(&format!("/v1/drafts/{draft}/approve"), json!({}))
        .error(409, "conflict");
    server
        .post(&format!("/v1/drafts/{draft}/reject"), json!({}))
        .error(409, "conflict");
}

#[test]
fn a_draft_nobody_commits_does_not_strand_its_records() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);
    let leave = json!({ "entity_type": "thing", "commit": false });

    // Rejected: the next run examines the same records again.
    let first = server.call("/v1/match", leave.clone());
    let first_draft = first["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let rejected = server.call(
        &format!("/v1/drafts/{first_draft}/reject"),
        json!({ "reason": "not yet" }),
    );
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["rejected_reason"], "not yet");
    assert_eq!(unassigned(&server), 3);
    server
        .post(&format!("/v1/drafts/{first_draft}/approve"), json!({}))
        .error(409, "conflict");

    // Left alone: a later run drafts again over everything still unassigned,
    // and the draft it replaces is marked as superseded.
    let second = server.call("/v1/match", leave.clone());
    let second_draft = second["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(second["run"]["resolution"]["new_records"], 3);
    load_things(&server, "shop", &[&["id", "code"], &["4", "hammer"]]);
    let third = server.call("/v1/match", leave);
    let third_draft = third["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let third_reconcile = third["run"]["resolution"]["reconcile_run"].clone();
    assert_eq!(third["run"]["resolution"]["new_records"], 4);
    assert_eq!(third["run"]["resolution"]["records_assigned"], 4);

    let superseded = server.read(&format!("/v1/drafts/{second_draft}"));
    assert_eq!(superseded["draft"]["status"], "superseded");
    assert_eq!(superseded["draft"]["superseded_by"], third_reconcile);
    let message = server
        .post(&format!("/v1/drafts/{second_draft}/approve"), json!({}))
        .error(409, "conflict");
    assert!(message.contains("superseded"), "{message}");
    let open = server.read("/v1/drafts?status=open");
    assert_eq!(open["drafts"].as_array().unwrap().len(), 1, "{open}");
    assert_eq!(open["drafts"][0]["id"], third_draft.as_str());

    // A run that commits takes everything, and leaves no draft open.
    let committed = server.call("/v1/match", json!({ "entity_type": "thing" }));
    assert_eq!(committed["run"]["status"], "succeeded");
    assert_eq!(committed["run"]["resolution"]["records_assigned"], 4);
    assert_eq!(unassigned(&server), 0);
    assert_eq!(server.read("/v1/drafts?status=open")["drafts"], json!([]));
    assert_eq!(
        server.read("/v1/identities")["identities"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn approving_a_draft_that_went_stale_is_rejected_and_names_the_operation() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);

    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "thing", "commit": false }),
    );
    let draft = run["run"]["resolution"]["draft"]
        .as_str()
        .unwrap()
        .to_owned();
    let record = server.read(&format!("/v1/drafts/{draft}"));

    // Someone commits the same assignments first, in a changeset of their own.
    let minted = server.call("/v1/ids", json!({ "identities": 2, "changesets": 1 }));
    let mut theirs = record["changeset"].clone();
    theirs["id"] = minted["changesets"][0].clone();
    // The author is whoever sends it.
    theirs.as_object_mut().unwrap().remove("actor");
    theirs["run"] = Value::Null;
    let mut renamed = serde_json::to_string(&theirs).unwrap();
    let created: Vec<&str> = record["changeset"]["ops"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|op| op["op"] == "create_identity")
        .map(|op| op["identity"].as_str().unwrap())
        .collect();
    for (index, identity) in created.iter().enumerate() {
        renamed = renamed.replace(identity, minted["identities"][index].as_str().unwrap());
    }
    let theirs: Value = serde_json::from_str(&renamed).unwrap();
    let committed = server.call("/v1/changesets", theirs);
    assert_eq!(committed["seq"], 1);

    let response = server.post(&format!("/v1/drafts/{draft}/approve"), json!({}));
    let message = response.error(409, "rejected");
    assert!(message.contains("changeset rejected"), "{message}");
    let rejections = response.json()["error"]["rejections"].clone();
    let indices: Vec<u64> = rejections
        .as_array()
        .unwrap()
        .iter()
        .map(|rejection| rejection["op_index"].as_u64().unwrap())
        .collect();
    assert!(!indices.is_empty(), "{rejections}");
    for (index, rejection) in indices.iter().zip(rejections.as_array().unwrap()) {
        assert_eq!(
            rejection["op"], record["changeset"]["ops"][*index as usize],
            "{rejections}"
        );
    }

    // Nothing of the draft was applied, and it is still there to reject.
    assert_eq!(server.read("/v1/changesets")["head"], 1);
    assert_eq!(
        server.read(&format!("/v1/drafts/{draft}"))["draft"]["status"],
        "open"
    );
    let rejected = server.call(&format!("/v1/drafts/{draft}/reject"), json!({}));
    assert_eq!(rejected["status"], "rejected");
}

fn text_column(batches: &[arrow::record_batch::RecordBatch], name: &str) -> Vec<Option<String>> {
    let mut values = Vec::new();
    for batch in batches {
        let column = arrow::compute::cast(
            batch
                .column_by_name(name)
                .unwrap_or_else(|| panic!("a `{name}` column")),
            &arrow::datatypes::DataType::Utf8,
        )
        .unwrap();
        let column = column.as_string::<i32>();
        for row in 0..column.len() {
            values.push((!column.is_null(row)).then(|| column.value(row).to_owned()));
        }
    }
    values
}

#[test]
#[expect(clippy::too_many_lines)]
fn a_caller_judges_the_candidates_of_a_run_step_by_step() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    load_things(
        &server,
        "shop",
        &[
            &["id", "code", "colour"],
            &["1", "anvil", "black"],
            &["2", "anvil", "black"],
            &["3", "hammer", "red"],
            &["4", "hammer", "red"],
            &["5", "tongs", "grey"],
        ],
    );

    let strategy = json!({ "name": "coin-toss", "version": "2" });
    server
        .post(
            "/v1/match",
            json!({ "entity_type": "thing", "strategy": strategy, "commit": false }),
        )
        .error(400, "invalid");
    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "thing", "strategy": strategy }),
    );
    assert_eq!(run["run"]["status"], "open", "{run}");
    assert_eq!(run["run"]["strategy"], strategy);
    let run = run["run"]["run"].as_u64().unwrap();

    // The candidates come as Arrow, and as JSON when that is asked for.
    let batches = server.get(&format!("/v1/runs/{run}/candidates")).batches();
    let left = text_column(&batches, "left_id");
    let right = text_column(&batches, "right_id");
    let codes = text_column(&batches, "left_code");
    assert_eq!(codes.len(), 2, "one pair of anvils and one of hammers");
    assert_eq!(text_column(&batches, "right_code"), codes);
    let listed = server
        .request("GET", &format!("/v1/runs/{run}/candidates"))
        .header("Accept", "application/json")
        .send()
        .ok();
    let listed = listed["candidates"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    for (index, candidate) in listed.iter().enumerate() {
        assert_eq!(candidate["left_id"].as_str(), left[index].as_deref());
        assert_eq!(candidate["right_id"].as_str(), right[index].as_deref());
        assert_eq!(candidate["left_code"].as_str(), codes[index].as_deref());
        assert_eq!(candidate["left_colour"], candidate["right_colour"]);
    }
    let pair = |code: &str| {
        let index = codes
            .iter()
            .position(|found| found.as_deref() == Some(code))
            .unwrap();
        (left[index].clone().unwrap(), right[index].clone().unwrap())
    };
    let (anvils, hammers) = (pair("anvil"), pair("hammer"));

    // Judgements go back as JSON or as Arrow.
    let stored = server.call(
        &format!("/v1/runs/{run}/judgements"),
        json!({ "judgements": [{
            "left_id": anvils.0,
            "right_id": anvils.1,
            "verdict": "match",
            "score": 0.75,
            "detail": { "why": "heads" },
        }] }),
    );
    assert_eq!(stored, json!({ "stored": 1 }));
    let none = server.call(
        &format!("/v1/runs/{run}/judgements"),
        json!({ "judgements": [] }),
    );
    assert_eq!(none, json!({ "stored": 0 }));
    let stored = server
        .post_arrow(
            &format!("/v1/runs/{run}/judgements"),
            rows(&[
                &["left_id", "right_id", "verdict"],
                &[&hammers.0, &hammers.1, "no_match"],
            ]),
        )
        .ok();
    assert_eq!(stored, json!({ "stored": 1 }));
    server
        .post(
            &format!("/v1/runs/{run}/judgements"),
            json!({ "judgements": [{ "left_id": anvils.0, "right_id": anvils.1, "verdict": "perhaps" }] }),
        )
        .error(400, "invalid");
    let message = server
        .post(
            &format!("/v1/runs/{run}/judgements"),
            json!({ "judgements": [{
                "left_id": anvils.0,
                "right_id": "00000000-0000-7000-8000-000000000000",
                "verdict": "match",
            }] }),
        )
        .error(400, "invalid");
    assert!(message.contains("not a known thing"), "{message}");

    // Reconciling leaves a draft for review; approving it commits.
    let resolution = server.call(
        &format!("/v1/runs/{run}/reconcile"),
        json!({ "commit": false }),
    );
    assert_eq!(resolution["match_run"], run);
    assert_eq!(
        resolution["judgements"],
        json!({ "match": 1, "no_match": 1, "unsure": 0 })
    );
    assert_eq!(resolution["identities_created"], 4);
    assert_eq!(resolution["committed"], Value::Null);
    let draft = resolution["draft"].as_str().unwrap();
    assert_eq!(
        server.read(&format!("/v1/runs/{run}"))["run"]["status"],
        "drafted"
    );
    server.call(&format!("/v1/drafts/{draft}/approve"), json!({}));
    assert_eq!(
        server.read(&format!("/v1/runs/{run}"))["run"]["status"],
        "succeeded"
    );

    let identities = server.call(
        "/v1/records/identities",
        json!({ "records": [anvils.0, anvils.1, hammers.0, hammers.1] }),
    );
    let identities = identities["identities"].as_array().unwrap();
    assert!(identities.iter().all(|identity| identity.is_string()));
    assert_eq!(
        identities[0], identities[1],
        "the anvils were judged the same"
    );
    assert_ne!(
        identities[2], identities[3],
        "the hammers were judged different"
    );

    // The run is over: it takes no more judgements and cannot be cancelled.
    server
        .post(
            &format!("/v1/runs/{run}/judgements"),
            json!({ "judgements": [{ "left_id": anvils.0, "right_id": anvils.1, "verdict": "match" }] }),
        )
        .error(409, "conflict");
    server
        .post(
            &format!("/v1/runs/{run}/judgements"),
            json!({ "judgements": [] }),
        )
        .error(409, "conflict");
    server
        .post("/v1/runs/999/judgements", json!({ "judgements": [] }))
        .error(404, "not_found");
    server
        .post(&format!("/v1/runs/{run}/cancel"), json!({}))
        .error(409, "conflict");
    server
        .post("/v1/runs/999/reconcile", json!({}))
        .error(404, "not_found");
}

#[test]
fn an_open_run_is_reconciled_at_once_by_default_or_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);
    let strategy = json!({ "name": "nobody", "version": "1" });
    let start = json!({ "entity_type": "thing", "strategy": strategy });

    // Cancelled: the run fails and its records wait for another.
    let run = server.call("/v1/match", start.clone())["run"]["run"]
        .as_u64()
        .unwrap();
    let cancelled = server.call(&format!("/v1/runs/{run}/cancel"), json!({}));
    assert_eq!(cancelled["run"]["status"], "failed");
    assert_eq!(cancelled["run"]["error"], "cancelled");
    server
        .post(&format!("/v1/runs/{run}/reconcile"), json!({}))
        .error(409, "conflict");
    assert_eq!(unassigned(&server), 3);

    // Reconciled with no body and no judgements: every record gets an
    // identity of its own, committed.
    let run = server.call("/v1/match", start)["run"]["run"]
        .as_u64()
        .unwrap();
    let resolution = server
        .request("POST", &format!("/v1/runs/{run}/reconcile"))
        .send()
        .ok();
    assert_eq!(resolution["identities_created"], 3);
    assert_eq!(resolution["committed"]["seq"], 1);
    assert_eq!(resolution["draft"], Value::Null);
    assert_eq!(
        server.read(&format!("/v1/runs/{run}"))["run"]["status"],
        "succeeded"
    );
    assert_eq!(unassigned(&server), 0);

    let runs = server.read("/v1/runs?kind=match");
    let statuses: Vec<&str> = runs["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["succeeded", "failed"], "newest first");
}

#[test]
fn identities_records_and_review_items_are_looked_up() {
    let library = library(5);
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 5);
    run_companies(&server, Encoding::Arrow);

    // A record is found by its source and key, and names its identity.
    let found = server.read("/v1/records?source=crm&key=c-101");
    let [record] = &found["records"].as_array().unwrap()[..] else {
        panic!("one record: {found}");
    };
    assert_eq!(record["source"], "crm");
    assert_eq!(record["key"], "c-101");
    assert_eq!(record["entity_type"], "company");
    assert_eq!(record["payload"]["id"], "c-101");
    let id = record["id"].as_str().unwrap();
    let identity = record["identity"].as_str().unwrap();
    assert_eq!(server.read(&format!("/v1/records/{id}")), *record);

    // The identity lists its members, and its history names the changesets.
    let view = server.read(&format!("/v1/identities/{identity}"));
    assert_eq!(
        view,
        serde_json::to_value(library.identity(identity.parse().unwrap()).unwrap()).unwrap()
    );
    assert_eq!(view["current"], identity);
    let members = view["members"].as_array().unwrap();
    assert!(members.len() > 1, "{view}");
    assert!(members.iter().any(|member| member["record"] == id));
    let history = server.read(&format!("/v1/identities/{identity}/history"));
    assert_eq!(
        history["history"],
        serde_json::to_value(library.history(identity.parse().unwrap()).unwrap()).unwrap()
    );
    assert_eq!(history["history"][0]["seq"], 1);
    assert_eq!(history["history"][0]["ops"][0]["op"], "create_identity");

    // Many records are looked up in one call; an unknown one has none.
    let unknown = "00000000-0000-7000-8000-000000000000";
    let identities = server.call(
        "/v1/records/identities",
        json!({ "records": [id, unknown] }),
    );
    assert_eq!(identities, json!({ "identities": [identity, null] }));

    // Lists come in pages.
    let total = server.read("/v1/info")["store"]["entity_types"][0]["identities"]
        .as_u64()
        .unwrap();
    let mut listed = Vec::new();
    let mut after = String::new();
    loop {
        let page = server.read(&format!(
            "/v1/identities?entity_type=company&limit=4{after}"
        ));
        let identities = page["identities"].as_array().unwrap();
        assert!(identities.len() <= 4);
        listed.extend(identities.iter().map(|identity| identity["id"].clone()));
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => break,
        }
    }
    assert_eq!(listed.len() as u64, total);
    assert!(listed.contains(&json!(identity)));
    let first = server.read("/v1/records?source=registry&limit=3");
    assert_eq!(first["records"].as_array().unwrap().len(), 3);
    let next = first["next"].as_str().unwrap();
    let second = server.read(&format!("/v1/records?source=registry&limit=3&after={next}"));
    assert_ne!(second["records"][0]["id"], first["records"][2]["id"]);

    // The record that fits two identities is unassigned, and its review item
    // comes back typed, the same as the library reads it.
    let open = server.read("/v1/review?status=open");
    assert_eq!(
        open["items"],
        serde_json::to_value(
            library
                .review_items(&ReviewFilter {
                    status: Some(ReviewStatus::Open),
                    ..ReviewFilter::default()
                })
                .unwrap()
        )
        .unwrap()
    );
    let [item] = &open["items"].as_array().unwrap()[..] else {
        panic!("one open review item: {open}");
    };
    assert_eq!(item["entity_type"], "company");
    assert_eq!(item["status"], "open");
    assert_eq!(item["identities"].as_array().unwrap().len(), 2);
    let stuck = server.read("/v1/records?source=crm&key=c-117");
    assert_eq!(stuck["records"][0]["identity"], Value::Null);
    assert_eq!(item["records"], json!([stuck["records"][0]["id"]]));
    let waiting = server.read("/v1/records?unassigned=true");
    assert_eq!(waiting["records"], stuck["records"]);
    assert_eq!(
        server.read("/v1/info")["store"]["entity_types"][0]["open_review_items"],
        1
    );
    assert_eq!(
        server.read("/v1/review?entity_type=company")["items"],
        open["items"]
    );
}

/// Every page of a list, read by passing each page's `next` as `after`.
fn every_page(server: &Server, path: &str, list: &str) -> Vec<Value> {
    let mut listed = Vec::new();
    let mut after = String::new();
    loop {
        let page = server.read(&format!("{path}{after}"));
        listed.extend(page[list].as_array().unwrap().iter().cloned());
        match &page["next"] {
            Value::Null => break,
            next => after = format!("&after={}", next.as_str().unwrap()),
        }
    }
    listed
}

#[test]
fn lists_are_sorted_as_the_query_says_and_read_in_pages_in_that_order() {
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 5);
    run_companies(&server, Encoding::Arrow);

    // Pages of a sorted list walk the order of the whole of it.
    let whole = server.read("/v1/identities?sort=records&order=desc&limit=1000")["identities"]
        .as_array()
        .unwrap()
        .clone();
    assert!(whole.len() > 8, "{whole:?}");
    let members: Vec<u64> = whole
        .iter()
        .map(|identity| identity["members"].as_u64().unwrap())
        .collect();
    assert!(
        members.windows(2).all(|pair| pair[0] >= pair[1]),
        "{members:?}"
    );
    assert!(members[0] > *members.last().unwrap(), "{members:?}");
    assert_eq!(
        every_page(
            &server,
            "/v1/identities?sort=records&order=desc&limit=3",
            "identities"
        ),
        whole
    );
    let records = server.read("/v1/records?sort=label&limit=1000")["records"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        every_page(&server, "/v1/records?sort=label&limit=4", "records"),
        records
    );
    let labels: Vec<String> = records
        .iter()
        .filter_map(|record| record["label"].as_str().map(str::to_lowercase))
        .collect();
    assert!(
        labels.windows(2).all(|pair| pair[0] <= pair[1]),
        "{labels:?}"
    );

    // A list that is cut at `limit` is sorted before it is cut.
    let runs = server.read("/v1/runs?sort=kind&limit=1000")["runs"].clone();
    assert_eq!(
        server.read("/v1/runs?sort=kind&limit=2")["runs"]
            .as_array()
            .unwrap()[..],
        runs.as_array().unwrap()[..2]
    );
    // `order` alone turns the list's own order: runs are newest first.
    assert_eq!(
        server.read("/v1/runs?order=asc&limit=1")["runs"][0]["run"],
        1
    );
    assert_ne!(server.read("/v1/runs?limit=1")["runs"][0]["run"], 1);

    // Who did something is sorted by what they are called, which the
    // server knows and the store does not.
    server.get("/v1/runs?sort=started_by").expect(200);
    server
        .get("/v1/changesets?sort=actor&order=desc")
        .expect(200);

    // The log, newest first, a page at a time after a position.
    let head = server.read("/v1/info")["store"]["head"].as_u64().unwrap();
    let newest = server.read("/v1/changesets?sort=seq&order=desc&limit=1");
    assert_eq!(newest["changesets"][0]["seq"], head);
    let next = server.read(&format!(
        "/v1/changesets?sort=seq&order=desc&limit=1&after={head}"
    ));
    assert_eq!(next["changesets"][0]["seq"], head - 1);

    // What a list cannot be sorted by is refused, with what it can be.
    let refused = server.get("/v1/identities?sort=colour").expect(400);
    let said = refused["error"]["message"].as_str().unwrap();
    assert!(
        said.contains("colour") && said.contains("records"),
        "{said}"
    );
    server.get("/v1/documents?order=sideways").expect(400);
}

#[test]
#[expect(clippy::too_many_lines)]
fn what_is_read_says_what_things_are_called_where_they_came_from_and_what_waits() {
    let library = library(5);
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 5);
    run_companies(&server, Encoding::Json);

    // A record is called what its payload names it, whatever the field.
    let crm = server.read("/v1/records?source=crm&key=c-102");
    assert_eq!(crm["records"][0]["label"], "Acorn Tools");
    let registry = server.read("/v1/records?source=registry&key=X0000002");
    assert_eq!(registry["records"][0]["label"], "ACORN TOOL COMPANY LTD");
    let identity = crm["records"][0]["identity"].as_str().unwrap();
    assert_eq!(registry["records"][0]["identity"], identity);

    // An identity is called what most of its records are, and says how many
    // each source gave.
    let view = server.read(&format!("/v1/identities/{identity}"));
    assert_eq!(view["label"], "Acorn Tools");
    let page = server.read("/v1/identities?limit=1000");
    assert_eq!(
        page["identities"],
        serde_json::to_value(
            library
                .identities(&IdentityFilter {
                    limit: 1000,
                    ..IdentityFilter::default()
                })
                .unwrap()
        )
        .unwrap()
    );
    let listed = page["identities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["id"] == identity)
        .unwrap();
    assert_eq!(listed["label"], "Acorn Tools");
    assert_eq!(
        listed["sources"],
        json!([
            { "source": "crm", "records": 2 },
            { "source": "registry", "records": 1 },
        ])
    );

    // Its members are read as records, with what each of them says.
    let members = server.read(&format!("/v1/records?identity={identity}"));
    let mut labels: Vec<&str> = members["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["label"].as_str().unwrap())
        .collect();
    labels.sort();
    assert_eq!(
        labels,
        ["ACORN TOOL COMPANY LTD", "Acorn Tool Co.", "Acorn Tools"]
    );
    server
        .get("/v1/records?identity=acorn")
        .error(400, "invalid");

    // Identities are read by their IDs, as the list gives them.
    let all = page["identities"].as_array().unwrap();
    let other = all.iter().find(|listed| listed["id"] != identity).unwrap()["id"]
        .as_str()
        .unwrap();
    let named = server.read(&format!("/v1/identities?ids={other},{identity}"));
    let wanted: Vec<&serde_json::Value> = all
        .iter()
        .filter(|listed| listed["id"] == identity || listed["id"] == other)
        .collect();
    assert_eq!(named["identities"], json!(wanted));
    assert_eq!(named["next"], json!(null));
    let filtered = server.read(&format!("/v1/identities?ids={identity}&q=sheffield"));
    assert_eq!(filtered["identities"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["identities"][0]["id"], identity);
    assert_eq!(filtered["identities"][0]["match"]["kind"], "record");
    assert_eq!(filtered["identities"][0]["match"]["field"], "city");
    assert_eq!(server.read("/v1/identities?ids=")["identities"], json!([]));
    server
        .get(&format!("/v1/identities?ids={identity},acorn"))
        .error(400, "invalid");
    let many = vec![identity; 1001].join(",");
    server
        .get(&format!("/v1/identities?ids={many}"))
        .error(400, "invalid");

    // Records and identities are found by the words they hold.
    let found = server.read("/v1/identities?q=acorn%20sheffield");
    assert_eq!(found["identities"].as_array().unwrap().len(), 1);
    assert_eq!(found["identities"][0]["id"], identity);
    let found = server.read("/v1/records?q=ACORN&source=registry");
    assert_eq!(found["records"], registry["records"]);
    assert_eq!(
        server.read("/v1/records?q=no-such-company")["records"],
        json!([])
    );

    // The store says what each source gave.
    let info = server.read("/v1/info");
    let sources = info["store"]["entity_types"][0]["sources"]
        .as_array()
        .unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| (
                source["source"].as_str().unwrap(),
                source["records"].as_u64().unwrap(),
                source["unassigned"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("crm", 23, 1), ("registry", 16, 0)]
    );
    assert_eq!(
        info["store"],
        serde_json::to_value(library.info().unwrap()).unwrap()
    );
    assert_eq!(
        sources[0]["records"].as_u64().unwrap() + sources[1]["records"].as_u64().unwrap(),
        info["store"]["entity_types"][0]["records"]
    );

    // A review item is read with the records it is about: the one that
    // waits, the identities it matched with their members, and the
    // judgements that say so.
    let case = server.read("/v1/review/1/case");
    assert_eq!(
        case,
        serde_json::to_value(library.review_case(1).unwrap()).unwrap()
    );
    assert_eq!(case["item"], server.read("/v1/review/1"));
    assert_eq!(case["records"][0]["key"], "c-117");
    assert!(case["records"][0]["label"].is_string());
    let candidates = case["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    for candidate in candidates {
        assert!(candidate["identity"]["label"].is_string(), "{candidate}");
        assert_eq!(
            candidate["members"].as_array().unwrap().len() as u64,
            candidate["identity"]["members"].as_u64().unwrap()
        );
    }
    let judgements = case["judgements"].as_array().unwrap();
    assert!(judgements.len() >= 2, "{case}");
    assert!(
        judgements
            .iter()
            .all(|judgement| judgement["record"] == case["records"][0]["id"])
    );
    assert!(
        judgements
            .iter()
            .any(|judgement| judgement["verdict"] == "match")
    );
    // Each names the candidate it is about.
    for judgement in judgements {
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate["identity"]["id"] == judgement["identity"]),
            "{judgement}"
        );
    }
    server.get("/v1/review/99/case").error(404, "not_found");
    server.get("/v1/review/x/case").error(400, "invalid");
}

#[test]
#[expect(clippy::too_many_lines)]
fn a_pair_of_records_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    load_things(
        &server,
        "shop",
        &[
            &["id", "code", "colour"],
            &["1", "anvil", "black"],
            &["2", "anvil", "black"],
            &["3", "anvil", "red"],
        ],
    );
    server.call("/v1/match", json!({ "entity_type": "thing" }));
    let id = |key: &str| {
        server.read(&format!("/v1/records?source=shop&key={key}"))["records"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (first, second, red) = (id("1"), id("2"), id("3"));

    let explanation = server.read(&format!("/v1/explain?a={first}&b={second}"));
    assert_eq!(explanation["outcome"], "same_identity");
    assert_eq!(
        explanation["left"]["features"],
        json!({ "code": "anvil", "colour": "black" })
    );
    assert_eq!(explanation["comparisons"][0]["strategy"]["name"], "rules");
    assert_eq!(
        explanation["comparisons"][0]["judgements"][0]["verdict"],
        "match"
    );
    assert_eq!(explanation["rules"]["verdict"], "match");
    assert_eq!(explanation["path"][0]["kind"], "match");
    assert!(
        explanation["summary"][0]
            .as_str()
            .unwrap()
            .contains("belong to the same identity"),
        "{explanation}"
    );
    // The two can be named in either order.
    assert_eq!(
        server.read(&format!("/v1/explain?a={second}&b={first}")),
        explanation
    );

    let apart = server.read(&format!("/v1/explain?a={first}&b={red}"));
    assert_eq!(apart["outcome"], "different_identities");
    assert_eq!(apart["rules"]["verdict"], "no_match");
    assert_eq!(
        apart["rules"]["no_match"],
        json!([{ "name": "other_colour", "predicate": "l.colour <> r.colour", "holds": true }])
    );
    assert_eq!(apart["path"], json!([]));

    let unknown = "00000000-0000-7000-8000-000000000000";
    let message = server
        .get(&format!("/v1/explain?a={first}&b={unknown}"))
        .error(404, "not_found");
    assert_eq!(message, format!("there is no record {unknown}"));
    let message = server
        .get(&format!("/v1/explain?a={first}&b={first}"))
        .error(400, "invalid");
    assert!(message.contains("named twice"), "{message}");
    server
        .get(&format!("/v1/explain?a={first}"))
        .error(400, "invalid");
    let message = server
        .get(&format!("/v1/explain?a={first}&b=nothing"))
        .error(400, "invalid");
    assert!(message.contains("<source>:<key>"), "{message}");

    // A record is named by its source and key as well. That is the row the
    // source holds under the key now, whatever it held before.
    assert_eq!(server.read("/v1/explain?a=shop:1&b=shop:2"), explanation);
    assert_eq!(
        server.read(&format!("/v1/explain?a=shop:3&b={first}")),
        server.read(&format!("/v1/explain?a={red}&b={first}"))
    );
    let message = server
        .get("/v1/explain?a=shop:1&b=shop:9")
        .error(404, "not_found");
    assert_eq!(
        message,
        "there is no record with key `9` from source `shop`"
    );
    let unknown = server
        .get("/v1/explain?a=shop:1&b=shed:2")
        .error(404, "not_found");
    assert!(
        unknown.starts_with(
            "there is no record with key `2` from source `shed`: the project's sources are `"
        ),
        "{unknown}"
    );
    assert!(unknown.contains("`shop`"), "{unknown}");
    load_things(
        &server,
        "shop",
        &[
            &["id", "code", "colour"],
            &["2", "anvil", "blue"],
            &["a:b", "anvil", "black"],
        ],
    );
    let versions = server.read("/v1/records?source=shop&key=2");
    let versions = versions["records"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    let latest = versions
        .iter()
        .find(|version| version["superseded_by"].is_null())
        .unwrap();
    assert_eq!(latest["payload"]["colour"], "blue");
    let by_key = server.read("/v1/explain?a=shop:1&b=shop:2");
    let (left, right) = (&by_key["left"]["record"], &by_key["right"]["record"]);
    assert!(
        left["id"] == latest["id"] || right["id"] == latest["id"],
        "{by_key}"
    );
    // A key may have a colon in it: the first one ends the source's name.
    let colon = server.read("/v1/explain?a=shop:1&b=shop:a:b");
    let keys = [
        &colon["left"]["record"]["key"],
        &colon["right"]["record"]["key"],
    ];
    assert!(keys.contains(&&json!("a:b")), "{colon}");

    // Review items are listed by a record they name.
    assert_eq!(
        server.read(&format!("/v1/review?record={first}"))["items"],
        json!([])
    );
    server
        .get("/v1/review?record=nothing")
        .error(400, "invalid");
}

#[test]
fn calls_made_at_once_are_all_served_and_writes_take_turns() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());

    std::thread::scope(|scope| {
        let server = &server;
        let writers: Vec<_> = (0..8)
            .map(|writer| {
                scope.spawn(move || {
                    let rows: Vec<Value> = (0..50)
                        .map(|row| json!({ "id": format!("{writer}-{row}"), "code": format!("{row}") }))
                        .collect();
                    let response = server.post(
                        "/v1/load",
                        json!({
                            "source": format!("source-{writer}"),
                            "entity_type": "thing",
                            "rows": rows,
                        }),
                    );
                    assert_eq!(response.ok()["run"]["load"]["inserted"], 50);
                    let run = server.call("/v1/match", json!({ "entity_type": "thing" }));
                    assert_ne!(run["run"]["status"], "failed", "{run}");
                })
            })
            .collect();
        let readers: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(move || {
                    for _ in 0..20 {
                        assert_eq!(server.read("/v1/info")["api"], "v1");
                        server.read("/v1/identities?limit=5");
                        server.read("/v1/runs?limit=5");
                    }
                })
            })
            .collect();
        for thread in writers.into_iter().chain(readers) {
            thread.join().unwrap();
        }
    });

    // Whatever order the writers came in, every record ends up in one
    // of fifty identities: one per code.
    server.call("/v1/match", json!({ "entity_type": "thing" }));
    let info = server.read("/v1/info");
    let thing = &info["store"]["entity_types"][0];
    assert_eq!(thing["records"], 400);
    assert_eq!(thing["unassigned"], 0, "{info}");
    assert_eq!(thing["identities"], 50, "{info}");
    assert_eq!(thing["open_review_items"], 0, "{info}");
    let log = server.read("/v1/changesets?limit=1000");
    let seqs: Vec<u64> = log["changesets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    assert_eq!(log["head"], seqs.len() as u64);
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

#[test]
fn a_first_line_of_too_many_columns_is_refused_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    let csv = |body: &str| {
        server
            .request("POST", "/v1/load?source=shop&entity_type=thing")
            .header("Content-Type", "text/csv")
            .body(body.as_bytes())
            .send()
    };
    let line = |columns: usize| -> String {
        let names: Vec<String> = (0..columns).map(|column| format!("c{column}")).collect();
        names.join(",")
    };

    // Half a megabyte of names and no rows: what it costs to refuse does
    // not grow faster than the line does.
    let began = std::time::Instant::now();
    let message = csv(&line(80_000)).error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: line 1 names 80000 columns, which is more than the \
         10000 a file of rows may have"
    );
    // As many as there may be are looked at, and so is a name that comes
    // twice among them.
    let message = csv(&format!("{},c0\n", line(9_999))).error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: line 1: two columns are named `c0`"
    );
    assert!(
        began.elapsed() < std::time::Duration::from_secs(5),
        "the first lines took {:?}",
        began.elapsed()
    );
    // The server goes on reading rows.
    let answer = csv("id,code\n1,anvil\n").ok();
    assert_eq!(answer["run"]["load"]["inserted"], 1, "{answer}");
}

#[test]
fn rows_are_taken_as_csv_and_as_tab_separated_text() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    let csv = |query: &str, content_type: &str, body: &str| {
        server
            .request("POST", &format!("/v1/load?{query}"))
            .header("Content-Type", content_type)
            .body(body.as_bytes())
            .send()
    };
    let shop = "source=shop&entity_type=thing";

    // The first line names the columns, and every value is the text the
    // line has. A field with nothing in it is a value the row does not have.
    let body = "id,code,colour,name\n1,anvil,black,\"Anvil, large\"\n2,hammer,,\"\"\n";
    let answer = csv(shop, "text/csv", body).ok();
    assert_eq!(answer["run"]["status"], "succeeded");
    assert_eq!(answer["run"]["load"]["received"], 2);
    assert_eq!(answer["run"]["load"]["inserted"], 2);
    assert_eq!(
        held(&server, "shop"),
        [
            (
                json!("1"),
                json!({ "id": "1", "code": "anvil", "colour": "black", "name": "Anvil, large" })
            ),
            (json!("2"), json!({ "id": "2", "code": "hammer" })),
        ]
    );

    // The same rows are the same records however they come: as CSV with
    // its character set named and Windows line ends, as tabs, and as JSON.
    let again = csv(shop, "text/csv; charset=utf-8", &body.replace('\n', "\r\n")).ok();
    assert_eq!(again["run"]["load"]["duplicates"], 2, "{again}");
    let tabs = "id\tcode\tcolour\tname\n1\tanvil\tblack\tAnvil, large\n2\thammer\t\t\n";
    let again = csv(shop, "text/tab-separated-values", tabs).ok();
    assert_eq!(again["run"]["load"]["duplicates"], 2, "{again}");
    let again = server.call(
        "/v1/load",
        json!({
            "source": "shop",
            "entity_type": "thing",
            "rows": [
                { "id": "1", "code": "anvil", "colour": "black", "name": "Anvil, large" },
                { "id": "2", "code": "hammer", "colour": null },
            ],
        }),
    );
    assert_eq!(again["run"]["load"]["duplicates"], 2, "{again}");
    assert_eq!(held(&server, "shop").len(), 2);

    // What is wrong with a body is said with the line it is on.
    let message = csv(shop, "text/csv", "id,code\n3,saw\n4,file,flat\n").error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: line 3 has 3 fields, and the first line names 2 columns"
    );
    let message = csv(shop, "text/tab-separated-values", "id\tcode\n3\n").error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as tab-separated text: line 2 has 1 field, and the first line \
         names 2 columns"
    );
    let message = csv(shop, "text/csv", "").error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: it is empty: the first line has to name the columns"
    );
    let message = csv(shop, "text/csv", "id,code,id\n3,saw,3\n").error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: line 1: two columns are named `id`"
    );
    // Where the rows go is in the query, as it is for Arrow.
    let message = csv("entity_type=thing", "text/csv", body).error(400, "invalid");
    assert!(message.contains("source"), "{message}");
    // A column the source does not list is refused, with the ones it does.
    let message = csv(shop, "text/csv", "id,code,price\n3,saw,12\n").error(400, "invalid");
    assert_eq!(
        message,
        "load rejected: the rows have a column `price`, which sources/shop.yaml does not \
         list. It lists `id`, `code`, `colour`, `name`, `weight`. Add the column to that file to \
         keep it, or leave it out of the rows"
    );
    assert_eq!(held(&server, "shop").len(), 2, "none of those stored a row");

    // The first line alone is no rows, which is not a mistake.
    let none = csv(shop, "text/csv", "id,code\n").ok();
    assert_eq!(none["run"]["load"]["received"], 0, "{none}");
}

#[test]
fn a_name_that_two_records_have_is_refused_with_their_ids() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_two_of_each(&project);
    let server = Server::serve(&project, dir.path(), 1);
    let load = |entity_type: &str, rows: Value| {
        let body = json!({ "source": "crm", "entity_type": entity_type, "rows": rows });
        server.call("/v1/load", body);
    };
    // The source sends rows of two types, and has a company and a person
    // under the key 1. Each is the row of its type that nothing replaced.
    load(
        "company",
        json!([{ "id": "1", "name": "Dana" }, { "id": "2", "name": "Dana" }]),
    );
    load("person", json!([{ "id": "1", "name": "Dana Reyes" }]));
    let held = |key: &str, entity_type: &str| -> String {
        let records = server.read(&format!(
            "/v1/records?source=crm&key={key}&entity_type={entity_type}"
        ));
        let current = records["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["superseded_by"].is_null())
            .unwrap();
        current["id"].as_str().unwrap().to_owned()
    };
    let (company, person) = (held("1", "company"), held("1", "person"));

    // The name does not say which, so neither is taken for it.
    let message = server
        .get("/v1/explain?a=crm:1&b=crm:2")
        .error(400, "invalid");
    assert!(
        message.starts_with("2 records are named `crm:1`: name one by its ID, of "),
        "{message}"
    );
    assert!(
        message.contains(&format!("{company} (a `company`)")),
        "{message}"
    );
    assert!(
        message.contains(&format!("{person} (a `person`)")),
        "{message}"
    );
    // Whichever side it is on.
    server
        .get("/v1/explain?a=crm:2&b=crm:1")
        .error(400, "invalid");
    // By its ID the company is explained, and a name one record has still
    // names it.
    let by_id = server.read(&format!("/v1/explain?a={company}&b=crm:2"));
    let ids = [
        &by_id["left"]["record"]["id"],
        &by_id["right"]["record"]["id"],
    ];
    assert!(ids.contains(&&json!(company)), "{by_id}");

    // A new version of the company's row leaves two that nothing replaced,
    // the new one and the person.
    load("company", json!([{ "id": "1", "name": "Dana Ltd" }]));
    let message = server
        .get("/v1/explain?a=crm:1&b=crm:2")
        .error(400, "invalid");
    assert!(message.starts_with("2 records are named"), "{message}");
    assert!(!message.contains(&company), "{message}");
    assert!(message.contains(&held("1", "company")), "{message}");
}

#[test]
fn a_load_replaces_a_row_unless_it_is_told_not_to() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    let load = |rows: Value, replace: Option<bool>| {
        let mut body = json!({ "source": "shop", "entity_type": "thing", "rows": rows });
        if let Some(replace) = replace {
            body["replace"] = json!(replace);
        }
        server.post("/v1/load", body)
    };
    let csv = |query: &str, body: &str| {
        server
            .request(
                "POST",
                &format!("/v1/load?source=shop&entity_type=thing{query}"),
            )
            .header("Content-Type", "text/csv")
            .body(body.as_bytes())
            .send()
    };
    let first = json!([{ "id": "1", "code": "anvil" }, { "id": "2", "code": "saw" }]);
    load(first.clone(), Some(false)).ok();

    // Told not to replace, a load of a row the store holds with other
    // values is refused whole, in JSON and in CSV alike.
    let changed = json!([{ "id": "3", "code": "file" }, { "id": "2", "code": "saw, hand" }]);
    let refusal = "load rejected: 1 row has a key the store holds with other values: `2`. \
                   Nothing was loaded, because this load was not to replace rows. If the row is \
                   meant to change, load again with `replace` set to true";
    assert_eq!(
        load(changed.clone(), Some(false)).error(400, "invalid"),
        refusal
    );
    let lines = "id,code\n3,file\n2,\"saw, hand\"\n";
    assert_eq!(csv("&replace=false", lines).error(400, "invalid"), refusal);
    assert_eq!(held(&server, "shop").len(), 2);
    // The rows the store holds as they are add nothing and are not refused.
    let same = load(first, Some(false)).ok();
    assert_eq!(same["run"]["load"]["duplicates"], 2, "{same}");

    // A load that does not say replaces the row, as a load that says it may.
    let replaced = load(changed, None).ok();
    assert_eq!(replaced["run"]["load"]["superseded"], 1, "{replaced}");
    let again = csv("&replace=true", "id,code\n2,\"saw, bow\"\n").ok();
    assert_eq!(again["run"]["load"]["superseded"], 1, "{again}");
    let again = csv("", "id,code\n2,\"saw, pit\"\n").ok();
    assert_eq!(again["run"]["load"]["superseded"], 1, "{again}");
    csv("&replace=perhaps", "id,code\n2,saw\n").error(400, "invalid");
}

#[test]
fn a_load_and_a_match_are_called_by_the_words_the_commands_have() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    let rows = json!({
        "source": "shop",
        "entity_type": "thing",
        "rows": [{ "id": "1", "code": "anvil" }],
    });

    // The paths are the words of `spool load` and `spool match`, and what
    // comes back is in those words too.
    let loaded = server.call("/v1/load", rows.clone());
    assert_eq!(loaded["run"]["kind"], "load");
    assert_eq!(loaded["run"]["load"]["inserted"], 1);
    let matched = server.call("/v1/match", json!({ "entity_type": "thing" }));
    assert_eq!(matched["run"]["kind"], "match");
    let record = server.read("/v1/records")["records"][0].clone();
    assert_eq!(record["load_run"], loaded["run"]["run"]);
    assert!(record["loaded_at"].is_string(), "{record}");
    assert_eq!(
        server.read("/v1/runs?kind=load")["runs"][0]["run"],
        loaded["run"]["run"]
    );

    // Runs are read where they were, and nothing is started there.
    server
        .post("/v1/runs", json!({ "entity_type": "thing" }))
        .error(405, "invalid");
    server.get("/v1/match").error(405, "invalid");
    // The words the calls had before are no longer answered.
    server.post("/v1/ingest", rows).error(404, "not_found");
    server.get("/v1/runs?kind=ingest").error(400, "invalid");
}

#[test]
fn an_answer_as_json_ends_with_a_newline() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    two_anvils_and_a_hammer(&server);

    // What `curl` prints is followed by the prompt on a line of its own.
    let ends = |response: Response| {
        assert_eq!(
            response.header("content-type"),
            Some("application/json"),
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        assert!(
            response.body.ends_with(b"}\n") && !response.body.ends_with(b"\n\n"),
            "{:?}",
            String::from_utf8_lossy(&response.body)
        );
        // It still comes with its length, which is the length it has.
        assert_eq!(
            response.header("content-length"),
            Some(response.body.len().to_string().as_str())
        );
        response
    };
    ends(server.get("/v1/info")).ok();
    ends(server.get("/v1/records")).ok();
    ends(server.post("/v1/match", json!({ "entity_type": "thing" }))).ok();
    ends(server.get("/v1/no-such-path")).error(404, "not_found");
    ends(server.get("/v1/runs/first")).error(400, "invalid");
    ends(Request::to(&server.address, "GET", "/v1/info").send()).error(401, "unauthorized");

    // The studio's pages are not JSON, and are served as they are.
    let page = server.get("/");
    assert_eq!(page.status, 200);
    assert!(!page.body.ends_with(b"}\n"));
}

#[test]
fn records_are_listed_by_source_and_key_and_the_sources_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    load_things(
        &server,
        "shop",
        &[
            &["id", "code"],
            &["b", "bolt"],
            &["10", "ten"],
            &["2", "two"],
        ],
    );
    load_things(&server, "bulk", &[&["id", "code"], &["z", "zip"]]);
    load_things(&server, "shop", &[&["id", "code"], &["10", "ten again"]]);

    // A source's rows are together, by key as text, and a row's versions
    // follow each other.
    let all = server.read("/v1/records")["records"].clone();
    let listed: Vec<(&str, &str, &str)> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            (
                record["source"].as_str().unwrap(),
                record["key"].as_str().unwrap(),
                record["payload"]["code"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("bulk", "z", "zip"),
            ("shop", "10", "ten"),
            ("shop", "10", "ten again"),
            ("shop", "2", "two"),
            ("shop", "b", "bolt"),
        ]
    );
    // Pages follow the same order: `next` is the last record of a page.
    let mut walked = Vec::new();
    let mut after = String::new();
    loop {
        let page = server.read(&format!("/v1/records?limit=2{after}"));
        walked.extend(page["records"].as_array().unwrap().iter().cloned());
        match page["next"].as_str() {
            Some(next) => {
                assert_eq!(walked.last().unwrap()["id"], next);
                after = format!("&after={next}");
            }
            None => break,
        }
    }
    assert_eq!(json!(walked), all);
    // A place in the list is a record the server holds.
    let message = server
        .get("/v1/records?after=00000000-0000-7000-8000-000000000000")
        .error(404, "not_found");
    assert_eq!(
        message,
        "there is no record 00000000-0000-7000-8000-000000000000"
    );

    // What a client needs to know to send rows: the sources, the columns
    // they list, and the entity types that read them.
    let sources = server.read("/v1/info")["store"]["sources"].clone();
    let names: Vec<&str> = sources
        .as_array()
        .unwrap()
        .iter()
        .map(|source| source["name"].as_str().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    let shop = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["name"] == "shop")
        .unwrap();
    assert_eq!(
        *shop,
        json!({
            "name": "shop",
            "key": "id",
            "evidence": null,
            "columns": ["id", "code", "colour", "name", "weight"],
            "entity_types": ["thing"],
        })
    );
}
