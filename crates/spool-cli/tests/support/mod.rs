//! A running `spool` server and a small HTTP client to call it with.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::{RecordBatch, RecordBatchReader};
use serde_json::Value;
use spool::Rows;

pub const TOKEN: &str = "test-token";
pub const ARROW: &str = "application/vnd.apache.arrow.stream";

pub fn benchmarks() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks")
}

/// The `spool` binary, started the way a user starts it: with no library
/// path set for it.
pub fn spool() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spool"));
    for name in [
        "DYLD_LIBRARY_PATH",
        "DYLD_FALLBACK_LIBRARY_PATH",
        "LD_LIBRARY_PATH",
        "SPOOL_TOKEN",
        "SPOOL_STORE",
        "SPOOL_URL",
    ] {
        command.env_remove(name);
    }
    command
}

pub fn run(arguments: &[&str]) -> Output {
    spool().args(arguments).output().unwrap()
}

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Runs a command that has to succeed and returns what it printed.
pub fn succeed(arguments: &[&str]) -> String {
    let output = run(arguments);
    assert!(
        output.status.success(),
        "spool {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    text(&output.stdout)
}

pub fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Writes a project with one entity type, `thing`: two things are the same
/// if their `code` is the same, unless their `colour` differs.
pub fn write_things(dir: &Path) {
    let entity = dir.join("entities/thing");
    for (file, sql) in [
        (
            "features.sql",
            "SELECT observation_id, payload->>'code' AS code, payload->>'colour' AS colour\nFROM observation\n",
        ),
        ("rules/block/same_code.sql", "l.code = r.code\n"),
        ("rules/match/same_code.sql", "l.code = r.code\n"),
        ("rules/no_match/other_colour.sql", "l.colour <> r.colour\n"),
    ] {
        let file = entity.join(file);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, sql).unwrap();
    }
}

/// Copies a project's `entities` directory, so that building it leaves the
/// original as it was.
pub fn copy_project(from: &Path, to: &Path) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    copy(&from.join("entities"), &to.join("entities"));
}

/// Builds the project in `dir` and returns the path of its manifest.
pub fn build(dir: &Path) -> PathBuf {
    succeed(&["build", path(dir)]);
    dir.join(".spool/manifest.json")
}

/// A server that stopped instead of listening.
#[derive(Debug)]
pub struct Stopped {
    pub code: Option<i32>,
    /// What it printed, on either stream.
    pub output: String,
}

#[derive(Debug)]
pub struct Server {
    child: Child,
    pub address: String,
    pub token: String,
    stdout: Mutex<Receiver<String>>,
    stderr: Mutex<Receiver<String>>,
}

fn lines(stream: impl Read + Send + 'static) -> Receiver<String> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    receive
}

fn wait_for(lines: &Receiver<String>, what: &str, seen: &mut Vec<String>) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left) {
            Ok(line) => {
                seen.push(line.clone());
                if line.contains(what) {
                    return Some(line);
                }
            }
            Err(_) => return None,
        }
    }
}

impl Server {
    /// Starts `spool <arguments>` with the test token and waits until it
    /// listens. The arguments must make it listen on a port of its choosing.
    pub fn start(arguments: &[&str]) -> Server {
        Self::start_with(arguments, Some(TOKEN)).unwrap_or_else(|stopped| {
            panic!("the server did not start:\n{}", stopped.output);
        })
    }

    /// As `start`. `Err` if the process stopped instead of listening.
    pub fn start_with(arguments: &[&str], token: Option<&str>) -> Result<Server, Stopped> {
        let mut command = spool();
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(token) = token {
            command.env("SPOOL_TOKEN", token);
        }
        let mut child = command.spawn().unwrap();
        let stdout = lines(child.stdout.take().unwrap());
        let stderr = lines(child.stderr.take().unwrap());
        let mut seen = Vec::new();
        match wait_for(&stdout, "listening on http://", &mut seen) {
            Some(line) => Ok(Server {
                child,
                address: line.rsplit_once("http://").unwrap().1.trim().to_owned(),
                token: token.unwrap_or("dev").to_owned(),
                stdout: Mutex::new(stdout),
                stderr: Mutex::new(stderr),
            }),
            None => {
                let _ = child.kill();
                let status = child.wait().unwrap();
                seen.extend(stderr.iter());
                Err(Stopped {
                    code: status.code(),
                    output: seen.join("\n"),
                })
            }
        }
    }

    /// Kills the server with no chance to finish what it was doing.
    pub fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }

    /// `spool serve` on a new store in `dir`, from the manifest of the
    /// project in `project`.
    pub fn serve(project: &Path, dir: &Path, seed: u64) -> Server {
        let manifest = build(project);
        Self::start(&[
            "serve",
            "--manifest",
            path(&manifest),
            "--store",
            path(&dir.join("store.duckdb")),
            "--listen",
            "127.0.0.1:0",
            "--seed",
            &seed.to_string(),
        ])
    }

    /// Waits for a line on the server's standard output that contains `what`.
    pub fn printed(&self, what: &str) -> String {
        let mut seen = Vec::new();
        wait_for(&self.stdout.lock().unwrap(), what, &mut seen)
            .unwrap_or_else(|| panic!("the server never printed `{what}`; it printed {seen:?}"))
    }

    /// Waits for a line on the server's standard error that contains `what`.
    pub fn complained(&self, what: &str) -> String {
        let mut seen = Vec::new();
        wait_for(&self.stderr.lock().unwrap(), what, &mut seen)
            .unwrap_or_else(|| panic!("the server never complained `{what}`; it said {seen:?}"))
    }

    /// Stops the server the way a container runtime does and waits for it.
    pub fn stop(mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let status = self.child.wait().unwrap();
        assert!(status.success(), "the server stopped with {status}");
    }

    pub fn request(&self, method: &str, path: &str) -> Request<'_> {
        Request::to(&self.address, method, path)
            .header("Authorization", &format!("Bearer {}", self.token))
    }

    pub fn get(&self, path: &str) -> Response {
        self.request("GET", path).send()
    }

    /// A GET that has to succeed, as JSON.
    pub fn read(&self, path: &str) -> Value {
        self.get(path).ok()
    }

    pub fn post(&self, path: &str, body: Value) -> Response {
        self.request("POST", path).json(&body).send()
    }

    /// A POST that has to succeed, as JSON.
    pub fn call(&self, path: &str, body: Value) -> Value {
        self.post(path, body).ok()
    }

    pub fn post_arrow(&self, path: &str, rows: Rows) -> Response {
        self.request("POST", path).arrow(rows).send()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Request<'a> {
    address: &'a str,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl<'a> Request<'a> {
    pub fn to(address: &'a str, method: &str, path: &str) -> Self {
        Request {
            address,
            method: method.to_owned(),
            path: path.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers
            .retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn without(mut self, name: &str) -> Self {
        self.headers
            .retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
        self
    }

    pub fn json(mut self, body: &Value) -> Self {
        self.body = serde_json::to_vec(body).unwrap();
        self.header("Content-Type", "application/json")
    }

    pub fn arrow(mut self, rows: Rows) -> Self {
        self.body = arrow_stream(rows);
        self.header("Content-Type", ARROW)
    }

    pub fn body(mut self, body: &[u8]) -> Self {
        self.body = body.to_vec();
        self
    }

    pub fn send(self) -> Response {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(120)))
            .unwrap();
        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.method,
            self.path,
            self.address,
            self.body.len()
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&self.body).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        Response::parse(&raw)
    }
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn parse(raw: &[u8]) -> Response {
        let end = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("a response has a head");
        let head = String::from_utf8_lossy(&raw[..end]).into_owned();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|status| status.parse().ok())
            .expect("a response has a status");
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let mut body = raw[end + 4..].to_vec();
        if headers
            .iter()
            .any(|(name, value)| name == "transfer-encoding" && value.contains("chunked"))
        {
            body = unchunk(&body);
        }
        Response {
            status,
            headers,
            body,
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "the body is not JSON ({error}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The body as JSON, of a response that has to be `200`.
    pub fn ok(&self) -> Value {
        self.expect(200)
    }

    pub fn expect(&self, status: u16) -> Value {
        assert_eq!(
            self.status,
            status,
            "{}",
            String::from_utf8_lossy(&self.body)
        );
        self.json()
    }

    /// Checks that this is an error response of the given status and code,
    /// and returns its message.
    pub fn error(&self, status: u16, code: &str) -> String {
        let body = self.expect(status);
        assert_eq!(body["error"]["code"], code, "{body}");
        body["error"]["message"].as_str().unwrap().to_owned()
    }

    pub fn batches(&self) -> Vec<RecordBatch> {
        assert_eq!(self.status, 200, "{}", String::from_utf8_lossy(&self.body));
        assert_eq!(self.header("content-type"), Some(ARROW));
        StreamReader::try_new(std::io::Cursor::new(self.body.clone()), None)
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }
}

fn unchunk(mut raw: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let end = raw
            .windows(2)
            .position(|window| window == b"\r\n")
            .expect("a chunk has a size line");
        let size = String::from_utf8_lossy(&raw[..end]);
        let size = usize::from_str_radix(size.split(';').next().unwrap().trim(), 16).unwrap();
        if size == 0 {
            return body;
        }
        let start = end + 2;
        body.extend_from_slice(&raw[start..start + size]);
        raw = &raw[start + size + 2..];
    }
}

pub fn arrow_stream(rows: Rows) -> Vec<u8> {
    let mut writer = StreamWriter::try_new(Vec::new(), &rows.schema()).unwrap();
    for batch in rows.into_batches() {
        writer.write(&batch).unwrap();
    }
    writer.into_inner().unwrap()
}

/// Rows as the JSON objects `POST /v1/ingest` takes. A missing value is left
/// out of its object.
pub fn json_rows(rows: Rows) -> Vec<Value> {
    use arrow::array::{Array, AsArray};
    let schema = rows.schema();
    let mut objects = Vec::new();
    for batch in rows.into_batches() {
        for row in 0..batch.num_rows() {
            let mut object = serde_json::Map::new();
            for (index, field) in schema.fields().iter().enumerate() {
                let column = batch.column(index).as_string::<i32>();
                if !column.is_null(row) {
                    object.insert(field.name().clone(), column.value(row).into());
                }
            }
            objects.push(Value::Object(object));
        }
    }
    objects
}

/// Text rows: the first row is the column names.
pub fn rows(table: &[&[&str]]) -> Rows {
    let columns = table[0];
    let rows: Vec<Vec<Option<&str>>> = table[1..]
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| (!value.is_empty()).then_some(*value))
                .collect()
        })
        .collect();
    Rows::from_text(columns, &rows).unwrap()
}
