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

/// Where `spool dev` listens unless it is told otherwise.
const DEFAULT_URL: &str = "http://127.0.0.1:7765";

/// The largest answer that is read. A document is the largest thing a
/// server returns in one piece.
const MAX_BODY_BYTES: u64 = 256 * 1024 * 1024;

/// How long one request waits for a run before the command asks again.
pub const WAIT_MS: u64 = 30_000;

/// The options every command that calls a server takes.
#[derive(clap::Args, Debug)]
#[command(next_help_heading = "Server options")]
pub struct RemoteArgs {
    /// The server
    #[arg(long, env = "SPOOL_URL", hide_env_values = true, default_value = DEFAULT_URL, global = true)]
    url: String,
    /// A token for that server; `spool dev` needs none
    #[arg(long, env = "SPOOL_TOKEN", hide_env_values = true, global = true)]
    token: Option<String>,
}

/// The source a file is named after, of the `sources` a project declares:
/// the one its name begins with, up to the dot or to a character that is
/// neither a letter nor a digit. `crm.csv`, `crm-1.csv` and `crm_2024.csv`
/// are named after `crm`, and `crmx.csv` is not. Of two such sources it is
/// the longer name: `crm-eu-1.csv` is named after `crm-eu`.
///
/// A command that takes a file takes its source from this when the project
/// has several, so that the files of a new project need no `--source`.
pub fn named_after<'a>(file: &str, sources: &[&'a str]) -> Option<&'a str> {
    let stem = std::path::Path::new(file).file_stem()?.to_str()?;
    sources
        .iter()
        .filter(|source| {
            stem.strip_prefix(**source).is_some_and(|rest| {
                !source.is_empty() && !rest.chars().next().is_some_and(char::is_alphanumeric)
            })
        })
        .max_by_key(|source| source.len())
        .copied()
}

/// Names in a sentence: `a`, `a` and `b`, or `a`, `b` and `c`.
pub fn listed(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

pub struct Remote {
    agent: ureq::Agent,
    pub url: String,
    /// What the server knows this caller by. `spool dev` asks for none.
    token: Option<String>,
}

impl Remote {
    pub fn new(arguments: &RemoteArgs) -> Remote {
        // A variable that is set and empty says nothing.
        let given = |value: Option<&String>| {
            value
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
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
            url: given(Some(&arguments.url))
                .unwrap_or_else(|| DEFAULT_URL.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            token: given(arguments.token.as_ref()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_named_after_the_source_its_name_begins_with() {
        let sources = ["crm", "crm-eu", "notes"];
        for (file, source) in [
            ("crm.csv", Some("crm")),
            ("crm-1.csv", Some("crm")),
            ("samples/crm_2024.tsv", Some("crm")),
            ("crm.2024.csv", Some("crm")),
            ("crm", Some("crm")),
            ("samples/notes-1.txt", Some("notes")),
            // The longer name of two that fit.
            ("crm-eu-1.csv", Some("crm-eu")),
            ("crm-europe.csv", Some("crm")),
            // A name that goes on in letters or digits is another name.
            ("crms.csv", None),
            ("crm2.csv", None),
            ("CRM-1.csv", None),
            ("old-crm.csv", None),
            // The directory a file is in says nothing.
            ("crm/rows.csv", None),
            ("-", None),
            ("", None),
        ] {
            assert_eq!(named_after(file, &sources), source, "{file}");
        }
        assert_eq!(named_after("crm.csv", &[]), None);
    }

    #[test]
    fn a_variable_that_is_set_and_empty_says_nothing() {
        let remote = Remote::new(&RemoteArgs {
            url: " ".to_owned(),
            token: Some(String::new()),
        });
        assert_eq!(remote.url, DEFAULT_URL);
        assert_eq!(remote.token, None);
        let remote = Remote::new(&RemoteArgs {
            url: "http://spool.example.com/".to_owned(),
            token: Some(" secret\n".to_owned()),
        });
        assert_eq!(remote.url, "http://spool.example.com");
        assert_eq!(remote.token.as_deref(), Some("secret"));
    }

    #[test]
    fn names_are_listed_as_a_sentence_lists_them() {
        assert_eq!(listed(&["crm"]), "`crm`");
        assert_eq!(listed(&["crm", "notes"]), "`crm` and `notes`");
        assert_eq!(listed(&["a", "b", "c"]), "`a`, `b` and `c`");
    }
}
