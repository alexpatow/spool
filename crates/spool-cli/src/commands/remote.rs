//! Calls to a running server, for the commands that show what it holds or
//! ask it to do something.
//!
//! These commands open no store: the server is the only process that does.
//! They go through the same API as every other client.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use spool::{RunEntry, RunStatus};
use spool_cli::api::{ErrorResponse, Info};

use crate::Failure;
use crate::args::Arguments;

/// Where `spool dev` listens unless it is told otherwise.
const DEFAULT_URL: &str = "http://127.0.0.1:7765";

/// The largest answer that is read. A document is the largest thing a
/// server returns in one piece.
const MAX_BODY_BYTES: u64 = 256 * 1024 * 1024;

/// How long one request waits for a run before the command asks again.
pub const WAIT_MS: u64 = 30_000;

/// The options every command that calls a server takes.
pub const VALUES: [&str; 2] = ["--url", "--token"];

pub struct Remote {
    agent: ureq::Agent,
    pub url: String,
    /// What the server knows this caller by. `spool dev` asks for none.
    token: Option<String>,
}

impl Remote {
    pub fn new(arguments: &Arguments) -> Remote {
        let named = |option: &str, variable: &str| {
            arguments.value(option).map(str::to_owned).or_else(|| {
                std::env::var(variable)
                    .ok()
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
            })
        };
        let config = ureq::Agent::config_builder()
            // A refusal has a body that says why, which is read like any
            // other answer.
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(concat!("spool/", env!("CARGO_PKG_VERSION")))
            .build();
        Remote {
            agent: ureq::Agent::new_with_config(config),
            url: named("--url", "SPOOL_URL")
                .unwrap_or_else(|| DEFAULT_URL.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            token: named("--token", "SPOOL_TOKEN"),
        }
    }

    /// `GET <path>` under `/v1`, as JSON. `Ok(None)` when the server has no
    /// such thing.
    pub fn find(&self, path: &str, query: &[(&str, String)]) -> Result<Option<Value>, Failure> {
        let mut request = self.agent.get(format!("{}/v1{path}", self.url));
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        for (name, value) in query {
            request = request.query(*name, value);
        }
        self.answer(request.call())
    }

    /// `POST <path>` under `/v1` with a JSON body, and the JSON it answers.
    pub fn post(&self, path: &str, body: &Value) -> Result<Value, Failure> {
        let mut request = self
            .agent
            .post(format!("{}/v1{path}", self.url))
            .header("Content-Type", "application/json");
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        self.answer(request.send(body.to_string()))?.ok_or_else(|| {
            Failure::Failed(format!(
                "{} has no `/v1{path}`; is it a Spool server of this version?",
                self.url
            ))
        })
    }

    /// `POST <path>` under `/v1` with a body that is not JSON, and the JSON
    /// it answers.
    pub fn send(
        &self,
        path: &str,
        query: &[(&str, String)],
        content_type: &str,
        body: &[u8],
    ) -> Result<Value, Failure> {
        let mut request = self
            .agent
            .post(format!("{}/v1{path}", self.url))
            .header("Content-Type", content_type);
        if let Some(token) = &self.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        for (name, value) in query {
            request = request.query(*name, value);
        }
        self.answer(request.send(body))?.ok_or_else(|| {
            Failure::Failed(format!(
                "{} has no `/v1{path}`; is it a Spool server of this version?",
                self.url
            ))
        })
    }

    /// What the server holds and what its project declares.
    pub fn info(&self) -> Result<Info, Failure> {
        typed(&self.list("/info", &[])?)
    }

    /// Waits until the run that `answer` started has ended, and returns the
    /// answer that says how. `None` when the server had nothing to do and
    /// started no run.
    pub fn ended(&self, mut answer: Value) -> Result<Option<(Value, RunEntry)>, Failure> {
        if answer["run"].is_null() {
            return Ok(None);
        }
        let mut run: RunEntry = typed(&answer["run"])?;
        while run.status == RunStatus::Running {
            answer = self.get(
                &format!("/runs/{}", run.run),
                &[("wait_ms", WAIT_MS.to_string())],
                || format!("the server no longer has run {}", run.run),
            )?;
            run = typed(&answer["run"])?;
        }
        Ok(Some((answer, run)))
    }

    /// What the server answered, as JSON. `Ok(None)` when it has no such
    /// thing.
    fn answer(
        &self,
        sent: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Option<Value>, Failure> {
        let mut response = sent.map_err(|error| {
            Failure::Failed(format!(
                "the server at {} cannot be reached: {error}\nStart one with `spool dev`, or name another with `--url` or SPOOL_URL.",
                self.url
            ))
        })?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_string()
            .map_err(|error| {
                Failure::Failed(format!(
                    "the answer of the server at {} could not be read: {error}",
                    self.url
                ))
            })?;
        let unexpected = || {
            Failure::Failed(format!(
                "{} answered {status} and not the way a Spool server does; is it one?",
                self.url
            ))
        };
        if (200..300).contains(&status) {
            return serde_json::from_str(&body)
                .map(Some)
                .map_err(|_| unexpected());
        }
        let refusal: ErrorResponse = serde_json::from_str(&body).map_err(|_| unexpected())?;
        match status {
            404 => Ok(None),
            401 => Err(Failure::Failed(match self.token {
                Some(_) => format!(
                    "the server at {} did not accept the token: pass one of its tokens with `--token` or in SPOOL_TOKEN",
                    self.url
                ),
                None => format!(
                    "the server at {} asks for a token: pass one with `--token` or in SPOOL_TOKEN. \
                     An admin makes one on the studio's Tokens page, and `spool token add` makes \
                     one on the server's machine.",
                    self.url
                ),
            })),
            _ => Err(Failure::Failed(refusal.error.message)),
        }
    }

    /// `GET <path>` under `/v1`, as JSON. `missing` says what is not there
    /// when the server has no such thing.
    pub fn get(
        &self,
        path: &str,
        query: &[(&str, String)],
        missing: impl FnOnce() -> String,
    ) -> Result<Value, Failure> {
        self.find(path, query)?
            .ok_or_else(|| Failure::Failed(missing()))
    }

    /// `GET <path>` of something that is always there, such as a list.
    pub fn list(&self, path: &str, query: &[(&str, String)]) -> Result<Value, Failure> {
        self.get(path, query, || {
            format!(
                "{} has no `/v1{path}`; is it a Spool server of this version?",
                self.url
            )
        })
    }
}

/// Reads a value the server sent as one of the API's types.
pub fn typed<T: DeserializeOwned>(value: &Value) -> Result<T, Failure> {
    T::deserialize(value).map_err(|error| {
        Failure::Failed(format!(
            "the server's answer is not what this version of `spool` expects: {error}"
        ))
    })
}

/// Why a run that did not succeed ended as it did, as a command's failure.
pub fn unsuccessful(run: &RunEntry) -> Failure {
    Failure::Failed(format!(
        "run {} {}: {}",
        run.run,
        run.status,
        run.error.as_deref().unwrap_or("the server did not say why")
    ))
}

pub fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("JSON that was read can be written")
    );
}
