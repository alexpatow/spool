//! Sign-in with Google, against something that stands in for Google: it
//! says where its sign-in is, hands out codes, and exchanges a code for a
//! token it signed, as Google does. The test is the browser.
//!
//! The keys under `fixtures/` were made for these tests and guard nothing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing;
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use openidconnect::core::{CoreJsonWebKeySet, CoreJwsSigningAlgorithm, CoreRsaPrivateSigningKey};
use openidconnect::{JsonWebKeyId, PrivateSigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::support::{self, Response, Server, path, write_things};

mod accounts;
mod setup;
mod signing_in;

const PASSWORD: &str = "a long enough one";
const CLIENT_ID: &str = "spool-under-test";
const CLIENT_SECRET: &str = "what-google-and-the-server-share";

const SESSION: &str = "spool_session";
const AT_GOOGLE: &str = "spool_google";

fn key(pem: &str) -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(pem, Some(JsonWebKeyId::new("the-key".to_owned()))).unwrap()
}

/// What a query or a form says, with its `%` and `+` read.
fn fields(encoded: &str) -> HashMap<String, String> {
    fn decoded(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut read = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'%' => {
                    let pair = std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap();
                    read.push(u8::from_str_radix(pair, 16).unwrap());
                    at += 2;
                }
                b'+' => read.push(b' '),
                byte => read.push(byte),
            }
            at += 1;
        }
        String::from_utf8(read).unwrap()
    }
    encoded
        .split('&')
        .filter_map(|field| field.split_once('='))
        .map(|(name, value)| (decoded(name), decoded(value)))
        .collect()
}

/// Who signs in at the stand-in.
#[derive(Clone, Debug)]
struct Who {
    subject: &'static str,
    email: &'static str,
    verified: bool,
    /// The organisation whose Google Workspace the account is of, as its
    /// domain. An account anyone made for themselves has none.
    hosted: Option<&'static str>,
}

const GRACE: Who = Who {
    subject: "google-1",
    email: "grace@example.com",
    verified: true,
    hosted: Some("example.com"),
};

/// A way the token the stand-in hands out is not one to trust.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Wrong {
    /// Signed by someone who is not the stand-in.
    Signer,
    /// Made for another client.
    Audience,
    /// Says it is from somewhere else.
    Issuer,
    /// No longer good.
    Expired,
    /// Not signed at all.
    Unsigned,
}

/// What a code stands for: who signed in, and the sign-in it was for.
#[derive(Clone, Debug)]
struct Grant {
    who: Who,
    nonce: String,
    challenge: String,
}

/// A question the stand-in can be slow to answer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Asked {
    /// Where its sign-in is.
    Where,
    /// Who a code is for.
    Who,
}

#[derive(Default)]
struct Told {
    codes: HashMap<String, Grant>,
    wrong: Option<Wrong>,
    asked: usize,
    /// The question it leaves unanswered for now, and how many wait for it.
    slow: Option<Asked>,
    waiting: usize,
}

#[derive(Clone)]
struct StandIn {
    issuer: String,
    told: Arc<Mutex<Told>>,
}

impl StandIn {
    /// Starts listening, for as long as the test runs.
    fn start() -> StandIn {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let stand_in = StandIn {
            issuer: format!("http://{}", listener.local_addr().unwrap()),
            told: Arc::default(),
        };
        let router = Router::new()
            .route(
                "/.well-known/openid-configuration",
                routing::get(configuration),
            )
            .route("/jwks", routing::get(keys))
            .route("/token", routing::post(exchange))
            .with_state(stand_in.clone());
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, router).await.unwrap();
            });
        });
        stand_in
    }

    /// What the server is started with to be a client of the stand-in.
    fn environment(&self) -> [(&'static str, &str); 3] {
        [
            ("SPOOL_GOOGLE_CLIENT_ID", CLIENT_ID),
            ("SPOOL_GOOGLE_CLIENT_SECRET", CLIENT_SECRET),
            ("SPOOL_GOOGLE_ISSUER", &self.issuer),
        ]
    }

    /// Someone signs in at the stand-in's page: the code their browser
    /// takes back.
    fn sign_in(&self, who: &Who, sent: &HashMap<String, String>) -> String {
        let mut told = self.told.lock().unwrap();
        let code = format!("code-{}", told.codes.len() + 1);
        told.codes.insert(
            code.clone(),
            Grant {
                who: who.clone(),
                nonce: sent["nonce"].clone(),
                challenge: sent["code_challenge"].clone(),
            },
        );
        code
    }

    fn hands_out(&self, wrong: Option<Wrong>) {
        self.told.lock().unwrap().wrong = wrong;
    }

    /// How many codes it has been asked to exchange.
    fn asked(&self) -> usize {
        self.told.lock().unwrap().asked
    }

    /// Leaves a question unanswered until told otherwise.
    fn is_slow_to_say(&self, asked: Option<Asked>) {
        self.told.lock().unwrap().slow = asked;
    }

    /// Returns once `many` requests wait for an answer.
    fn until_waiting(&self, many: usize) {
        let patience = std::time::Instant::now() + Duration::from_secs(20);
        while self.told.lock().unwrap().waiting != many {
            assert!(std::time::Instant::now() < patience, "{many} never waited");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Answers `asked` when it is no longer slow to.
    async fn in_its_time(&self, asked: Asked) {
        self.told.lock().unwrap().waiting += 1;
        while self.told.lock().unwrap().slow == Some(asked) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.told.lock().unwrap().waiting -= 1;
    }
}

async fn configuration(State(stand_in): State<StandIn>) -> Json<Value> {
    stand_in.in_its_time(Asked::Where).await;
    let issuer = &stand_in.issuer;
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    }))
}

async fn keys() -> Json<Value> {
    let key = key(include_str!("../fixtures/stand-in-for-google.pem"));
    let set = CoreJsonWebKeySet::new(vec![key.as_verification_key()]);
    Json(serde_json::to_value(set).unwrap())
}

fn refusal(error: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": error })))
}

async fn exchange(
    State(stand_in): State<StandIn>,
    headers: axum::http::HeaderMap,
    body: String,
) -> (StatusCode, Json<Value>) {
    let form = fields(&body);
    stand_in.in_its_time(Asked::Who).await;
    let mut told = stand_in.told.lock().unwrap();
    told.asked += 1;
    // The client says who it is, as Google's own would have it.
    let client = format!(
        "Basic {}",
        STANDARD.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}"))
    );
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&client) {
        return refusal("invalid_client");
    }
    // A code is good once.
    let Some(grant) = form.get("code").and_then(|code| told.codes.remove(code)) else {
        return refusal("invalid_grant");
    };
    // Whoever brings the code is who asked for it.
    let verifier = form.get("code_verifier").map_or("", String::as_str);
    if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier)) != grant.challenge {
        return refusal("invalid_grant");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let wrong = told.wrong;
    let claims = json!({
        "iss": if wrong == Some(Wrong::Issuer) { "https://elsewhere.example" } else { &stand_in.issuer },
        "aud": if wrong == Some(Wrong::Audience) { "another-client" } else { CLIENT_ID },
        "sub": grant.who.subject,
        "email": grant.who.email,
        "email_verified": grant.who.verified,
        "hd": grant.who.hosted,
        "nonce": grant.nonce,
        "iat": now - if wrong == Some(Wrong::Expired) { 7_200 } else { 0 },
        "exp": if wrong == Some(Wrong::Expired) { now - 3_600 } else { now + 300 },
    });
    let algorithm = if wrong == Some(Wrong::Unsigned) {
        "none"
    } else {
        "RS256"
    };
    let head = json!({ "alg": algorithm, "kid": "the-key", "typ": "JWT" });
    let signed = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(head.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let signer = match wrong {
        Some(Wrong::Signer) => key(include_str!("../fixtures/someone-else.pem")),
        _ => key(include_str!("../fixtures/stand-in-for-google.pem")),
    };
    let signature = match wrong {
        Some(Wrong::Unsigned) => Vec::new(),
        _ => signer
            .sign(
                &CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
                signed.as_bytes(),
            )
            .unwrap(),
    };
    let token = format!("{signed}.{}", URL_SAFE_NO_PAD.encode(&signature));
    (
        StatusCode::OK,
        Json(json!({ "access_token": "unused", "token_type": "Bearer", "id_token": token })),
    )
}

// The browser

/// The cookies a response has the browser keep, by name. One it has the
/// browser forget has no value.
fn cookies(response: &Response) -> HashMap<String, String> {
    response
        .all("set-cookie")
        .into_iter()
        .map(|set| {
            assert!(
                set.contains("HttpOnly") && set.contains("SameSite=Lax"),
                "{set}"
            );
            let (name, value) = set.split(';').next().unwrap().split_once('=').unwrap();
            (name.to_owned(), value.to_owned())
        })
        .collect()
}

/// A request as a browser with `cookie` sends it: no token.
fn browse<'a>(server: &'a Server, method: &str, path: &str, cookie: &str) -> support::Request<'a> {
    let request = server.request(method, path).without("Authorization");
    match cookie {
        "" => request,
        cookie => request.header("Cookie", cookie),
    }
}

fn post(server: &Server, path: &str, cookie: &str, body: Value) -> Response {
    browse(server, "POST", path, cookie).json(&body).send()
}

fn location(response: &Response) -> &str {
    assert_eq!(response.status, 303, "{response:?}");
    response.header("location").expect("somewhere to go")
}

/// A sign-in the browser has been sent to the stand-in with.
struct Leaving {
    /// What the server asked of the stand-in, in the address it sent the
    /// browser to.
    sent: HashMap<String, String>,
    /// The cookie the browser keeps until it comes back.
    cookie: String,
}

/// Follows "Sign in with Google" as far as the stand-in's page.
fn leave(server: &Server, stand_in: &StandIn, next: &str) -> Leaving {
    let response = browse(server, "GET", &format!("/auth/google{next}"), "").send();
    let to = location(&response);
    let (page, query) = to.split_once('?').expect("a query");
    assert_eq!(page, format!("{}/authorize", stand_in.issuer));
    let kept = &cookies(&response)[AT_GOOGLE];
    assert!(!kept.is_empty());
    Leaving {
        sent: fields(query),
        cookie: format!("{AT_GOOGLE}={kept}"),
    }
}

/// Comes back from the stand-in with a code.
fn come_back(server: &Server, leaving: &Leaving, code: &str) -> Response {
    let back = format!(
        "/auth/google/callback?code={code}&state={}",
        leaving.sent["state"]
    );
    let response = browse(server, "GET", &back, &leaving.cookie).send();
    // Whatever came of it, the browser no longer keeps the sign-in.
    assert_eq!(cookies(&response)[AT_GOOGLE], "", "{response:?}");
    response
}

/// Signs in as `who` from start to end, and returns where the browser is
/// sent and the session it is given, if it is given one.
fn sign_in(server: &Server, stand_in: &StandIn, who: &Who) -> (String, Option<String>) {
    let leaving = leave(server, stand_in, "");
    let code = stand_in.sign_in(who, &leaving.sent);
    let response = come_back(server, &leaving, &code);
    let session = cookies(&response)
        .get(SESSION)
        .map(|session| format!("{SESSION}={session}"));
    (location(&response).to_owned(), session)
}

fn who(server: &Server, cookie: &str) -> Value {
    browse(server, "GET", "/auth/session", cookie).send().ok()
}

/// A server that signs people in with the stand-in, its first account's
/// cookie, and the stand-in.
struct Served {
    server: Server,
    stand_in: StandIn,
    ada: String,
    _dir: tempfile::TempDir,
}

fn arguments<'a>(manifest: &'a str, store: &'a str) -> [&'a str; 9] {
    [
        "serve",
        "--manifest",
        manifest,
        "--store",
        store,
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ]
}

fn served() -> Served {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = support::build(&project);
    let stand_in = StandIn::start();
    let server = Server::start_in(
        &arguments(path(&manifest), path(&dir.path().join("store.duckdb"))),
        &stand_in.environment(),
    )
    .unwrap();
    let said = server.printed("google    ");
    assert_eq!(
        said,
        format!("google    http://{}/auth/google/callback", server.address)
    );
    let link = server.printed("setup     ");
    let code = link.rsplit_once('#').unwrap().1;
    let first = post(
        &server,
        "/auth/setup",
        "",
        json!({ "code": code, "username": "ada", "name": "Ada", "password": PASSWORD }),
    );
    first.ok();
    let ada = format!("{SESSION}={}", cookies(&first)[SESSION]);
    Served {
        server,
        stand_in,
        ada,
        _dir: dir,
    }
}

/// Adds Grace, who signs in with Google and has no password, and returns
/// her ID.
fn add_grace(served: &Served) -> String {
    let grace = json!({ "username": "grace", "name": "Grace", "email": "Grace@Example.com" });
    let added = post(&served.server, "/v1/people", &served.ada, grace).ok();
    assert_eq!(added["email"], "grace@example.com");
    assert_eq!(added["password"], false);
    added["id"].as_str().unwrap().to_owned()
}

fn encoded(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            byte => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
