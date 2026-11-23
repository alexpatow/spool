//! Who may call the server, what it refuses, and how it answers a connection.

use super::*;

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
