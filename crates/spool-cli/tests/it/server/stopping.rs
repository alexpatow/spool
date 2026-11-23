//! Calls that take a while, and a server told to stop while a client waits.

use super::*;

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
