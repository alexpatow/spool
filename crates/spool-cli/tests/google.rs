//! Sign-in with Google, against something that stands in for Google: it
//! says where its sign-in is, hands out codes, and exchanges a code for a
//! token it signed, as Google does. The test is the browser.
//!
//! The keys under `fixtures/` were made for these tests and guard nothing.

mod support;

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
use support::{Response, Server, path, write_things};

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
    let key = key(include_str!("fixtures/stand-in-for-google.pem"));
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
        Some(Wrong::Signer) => key(include_str!("fixtures/someone-else.pem")),
        _ => key(include_str!("fixtures/stand-in-for-google.pem")),
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

#[test]
fn someone_an_admin_added_by_email_signs_in_with_google() {
    let served = served();
    let Served {
        server, stand_in, ..
    } = &served;
    let state = who(server, "");
    assert_eq!(
        state,
        json!({ "person": null, "setup": false, "dev": false, "google": true })
    );
    let grace = add_grace(&served);

    let leaving = leave(server, stand_in, "?next=/review%3Fstatus%3Ddismissed");
    let sent = &leaving.sent;
    assert_eq!(sent["client_id"], CLIENT_ID);
    assert_eq!(sent["response_type"], "code");
    assert_eq!(
        sent["redirect_uri"],
        format!("http://{}/auth/google/callback", server.address)
    );
    assert_eq!(sent["scope"], "openid email");
    assert_eq!(sent["code_challenge_method"], "S256");
    assert_eq!(sent["prompt"], "select_account");
    assert!(sent["state"].len() >= 16 && sent["nonce"].len() >= 16);
    // The secret stays with the server.
    assert!(!sent.values().any(|value| value.contains(CLIENT_SECRET)));

    let code = stand_in.sign_in(&GRACE, sent);
    let back = come_back(server, &leaving, &code);
    assert_eq!(location(&back), "/review?status=dismissed");
    let session = format!("{SESSION}={}", cookies(&back)[SESSION]);
    let state = who(server, &session);
    assert_eq!(state["person"]["id"], grace.as_str());
    assert_eq!(state["person"]["email"], "grace@example.com");

    // What she changes is recorded under her account.
    let minted = server.call("/v1/ids", json!({ "identities": 1 }));
    let create = json!({ "ops": [
        { "op": "create_identity", "identity": minted["identities"][0], "entity_type": "thing" }
    ] });
    post(server, "/v1/changesets", &session, create).ok();
    let log = server.read("/v1/changesets");
    assert_eq!(
        log["changesets"][0]["changeset"]["actor"],
        format!("user:{grace}")
    );

    // A code is good once, and so is the browser's leave to bring one.
    let again = come_back(server, &leaving, &code);
    assert_eq!(location(&again), "/review?status=dismissed&google=failed");
    assert!(!cookies(&again).contains_key(SESSION));
    let without = format!("/auth/google/callback?code={code}&state={}", sent["state"]);
    let stranger = browse(server, "GET", &without, "").send();
    assert_eq!(location(&stranger), "/?google=expired");

    // Signing out ends it.
    post(server, "/auth/sign-out", &session, json!({})).ok();
    assert_eq!(who(server, &session)["person"], Value::Null);
}

#[test]
fn google_says_who_someone_is_and_not_whether_they_have_an_account() {
    let served = served();
    let Served {
        server,
        stand_in,
        ada,
        ..
    } = &served;
    let grace = add_grace(&served);
    let turned_away = |who: &Who, why: &str| {
        let (to, session) = sign_in(server, stand_in, who);
        assert_eq!(to, format!("/?google={why}"), "{who:?}");
        assert_eq!(session, None, "{who:?}");
    };

    // Nobody added this address.
    let stranger = Who {
        subject: "google-2",
        email: "mallory@example.com",
        ..GRACE
    };
    turned_away(&stranger, "unknown");
    // An address Google has not checked could be anyone's.
    let unchecked = Who {
        verified: false,
        ..GRACE
    };
    turned_away(&unchecked, "unverified");
    // Someone with a password and no email is not looked up by username.
    let named = Who {
        subject: "google-3",
        email: "ada",
        ..GRACE
    };
    turned_away(&named, "unknown");

    let (to, session) = sign_in(server, stand_in, &GRACE);
    assert_eq!(to, "/");
    let session = session.expect("a session");
    assert_eq!(who(server, &session)["person"]["id"], grace.as_str());

    // The account is that Google account's now. Another one that comes to
    // have the address is not let in.
    let successor = Who {
        subject: "google-4",
        ..GRACE
    };
    turned_away(&successor, "unknown");
    // And the same one is, whatever its address has become.
    let renamed = Who {
        email: "hopper@example.com",
        ..GRACE
    };
    assert_eq!(sign_in(server, stand_in, &renamed).0, "/");

    // Another email on the account is another Google account's: what began
    // with the first one ends.
    let person = format!("/v1/people/{grace}");
    post(
        server,
        &person,
        ada,
        json!({ "email": "hopper@example.com" }),
    )
    .ok();
    assert_eq!(who(server, &session)["person"], Value::Null);
    turned_away(&GRACE, "unknown");
    // The Google account it was is let in again under the new address, and
    // what it began before stays ended: a cookie from then is good for
    // nothing, whoever has it.
    let (_, again) = sign_in(server, stand_in, &renamed);
    let again = again.expect("a session");
    assert_eq!(who(server, &again)["person"]["id"], grace.as_str());
    assert_eq!(who(server, &session)["person"], Value::Null);
    // The next address is someone else's Google account.
    post(
        server,
        &person,
        ada,
        json!({ "email": "g.hopper@example.com" }),
    )
    .ok();
    assert_eq!(who(server, &again)["person"], Value::Null);
    let (_, session) = sign_in(server, stand_in, &successor_of("g.hopper@example.com"));
    let session = session.expect("a session");
    assert_eq!(who(server, &session)["person"]["id"], grace.as_str());

    // An account that is off lets nobody in, and ends what was begun.
    post(server, &person, ada, json!({ "active": false })).ok();
    assert_eq!(who(server, &session)["person"], Value::Null);
    browse(server, "GET", "/v1/info", &session)
        .send()
        .error(401, "unauthorized");
    turned_away(&successor_of("g.hopper@example.com"), "unknown");

    // Someone who says no at Google's page.
    let leaving = leave(server, stand_in, "?next=/runs");
    let refused = format!(
        "/auth/google/callback?error=access_denied&state={}",
        leaving.sent["state"]
    );
    let response = browse(server, "GET", &refused, &leaving.cookie).send();
    assert_eq!(location(&response), "/runs?google=cancelled");
}

fn successor_of(email: &'static str) -> Who {
    Who {
        subject: "google-5",
        email,
        ..GRACE
    }
}

#[test]
fn an_account_becomes_a_google_accounts_by_an_address_google_answers_for() {
    let served = served();
    let Served {
        server,
        stand_in,
        ada,
        ..
    } = &served;
    let add = |username: &str, email: &str| {
        let person = json!({ "username": username, "name": username, "email": email });
        post(server, "/v1/people", ada, person).ok();
    };
    let entered = |who: &Who| {
        let (to, session) = sign_in(server, stand_in, who);
        session
            .map(|session| self::who(server, &session)["person"]["username"].clone())
            .ok_or(to)
    };
    let turned_away = Err("/?google=unvouched".to_owned());
    add("linus", "linus@elsewhere.example");
    add("ken", "ken@gmail.com");
    add("eve", "eve@notgmail.com");

    // Anyone makes a Google account with an address of any provider. Google
    // checked the address then, and whose it is now is not Google's to say:
    // someone who had it before may still have that Google account.
    let before = Who {
        subject: "google-6",
        email: "linus@elsewhere.example",
        verified: true,
        hosted: None,
    };
    assert_eq!(entered(&before), turned_away);
    // The account is not theirs for having tried. It is the Google account
    // of the organisation that has the address now.
    let linus = Who {
        subject: "google-7",
        hosted: Some("elsewhere.example"),
        ..before.clone()
    };
    assert_eq!(entered(&linus), Ok(json!("linus")));
    assert_eq!(entered(&before), turned_away);

    // Gmail's addresses are Google's own.
    let ken = Who {
        subject: "google-8",
        email: "Ken@Gmail.com",
        ..before.clone()
    };
    assert_eq!(entered(&ken), Ok(json!("ken")));
    // One that only ends alike is not.
    let eve = Who {
        subject: "google-9",
        email: "eve@notgmail.com",
        ..before.clone()
    };
    assert_eq!(entered(&eve), turned_away);

    // Once an account is a Google account's, it is known by what Google
    // calls it, whatever becomes of its address.
    let moved = Who {
        email: "linus@another.example",
        hosted: None,
        ..linus
    };
    assert_eq!(entered(&moved), Ok(json!("linus")));
}

#[test]
fn a_sign_in_comes_back_to_the_browser_it_left_from() {
    let served = served();
    let Served {
        server, stand_in, ..
    } = &served;
    add_grace(&served);
    let expired = |path: &str, cookie: &str| {
        let response = browse(server, "GET", path, cookie).send();
        assert_eq!(location(&response), "/?google=expired", "{path}");
        assert!(!cookies(&response).contains_key(SESSION));
    };

    // Grace's code, in a browser that did not leave for it: one that
    // someone sent a link to.
    let hers = leave(server, stand_in, "");
    let code = stand_in.sign_in(&GRACE, &hers.sent);
    let back = format!(
        "/auth/google/callback?code={code}&state={}",
        hers.sent["state"]
    );
    let asked = stand_in.asked();
    expired(&back, "");
    let other = leave(server, stand_in, "");
    expired(&back, &other.cookie);
    expired("/auth/google/callback", &hers.cookie);
    expired(&format!("/auth/google/callback?code={code}"), &hers.cookie);
    for made_up in ["", "a.b.c.2f", "....", "a.b.c.zz", "a.b.c"] {
        expired(&back, &format!("{AT_GOOGLE}={made_up}"));
    }
    // None of that was worth asking Google about.
    assert_eq!(stand_in.asked(), asked);

    // A code for someone else's sign-in, brought back as one's own: the
    // stand-in knows it was not this browser that asked.
    let theirs = leave(server, stand_in, "");
    let swapped = come_back(server, &theirs, &code);
    assert_eq!(location(&swapped), "/?google=failed");
    assert!(!cookies(&swapped).contains_key(SESSION));

    // Signing in leads to a page of this server and nowhere else.
    for (next, to) in [
        ("?next=/log/3", "/log/3"),
        ("?next=//elsewhere.example/", "/"),
        ("?next=/%5Celsewhere.example/", "/"),
        ("?next=https://elsewhere.example/", "/"),
        ("?next=elsewhere", "/"),
        ("", "/"),
    ] {
        let leaving = leave(server, stand_in, next);
        let code = stand_in.sign_in(&GRACE, &leaving.sent);
        assert_eq!(location(&come_back(server, &leaving, &code)), to, "{next}");
    }
    let long = format!("?next=/{}", "a".repeat(2_000));
    let leaving = leave(server, stand_in, &long);
    let code = stand_in.sign_in(&GRACE, &leaving.sent);
    assert_eq!(location(&come_back(server, &leaving, &code)), "/");
}

/// An authorization carries the assistant's request, and that can be longer
/// than the sign-in cookie. It comes back whole. A long address that is not
/// one still comes back to the first page.
#[test]
fn a_long_authorization_survives_signing_in_with_google() {
    let served = served();
    let Served {
        server, stand_in, ..
    } = &served;
    add_grace(&served);
    let state = "s".repeat(2_000);
    let next = format!("/oauth/authorize?response_type=code&state={state}");
    assert!(next.len() > 1_024, "{}", next.len());

    let leaving = leave(server, stand_in, &format!("?next={}", encoded(&next)));
    assert!(
        leaving.cookie.len() < next.len(),
        "the cookie held the request: {}",
        leaving.cookie.len()
    );
    let code = stand_in.sign_in(&GRACE, &leaving.sent);
    assert_eq!(location(&come_back(server, &leaving, &code)), next);

    // Turning Google away lands back on the same request, to sign in another way.
    let again = leave(server, stand_in, &format!("?next={}", encoded(&next)));
    let denied = browse(
        server,
        "GET",
        &format!(
            "/auth/google/callback?error=access_denied&state={}",
            again.sent["state"]
        ),
        &again.cookie,
    )
    .send();
    assert_eq!(location(&denied), format!("{next}&google=cancelled"));

    let other = format!("/tokens?x={}", "a".repeat(2_000));
    let leaving = leave(server, stand_in, &format!("?next={}", encoded(&other)));
    let code = stand_in.sign_in(&GRACE, &leaving.sent);
    assert_eq!(location(&come_back(server, &leaving, &code)), "/");
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

#[test]
fn the_server_asks_google_about_so_many_sign_ins_at_once() {
    const AT_ONCE: usize = 8;
    let served = served();
    let (server, stand_in) = (&served.server, &served.stand_in);
    add_grace(&served);

    // Nobody has signed in, so the server does not know where the stand-in's
    // sign-in is. Each who starts one has it ask, and the stand-in is slow.
    stand_in.is_slow_to_say(Some(Asked::Where));
    std::thread::scope(|scope| {
        let started: Vec<_> = (0..AT_ONCE)
            .map(|_| scope.spawn(|| leave(server, stand_in, "")))
            .collect();
        stand_in.until_waiting(AT_ONCE);
        // One more is not kept waiting: it is told to come back.
        let beyond = browse(server, "GET", "/auth/google?next=/runs", "").send();
        assert_eq!(location(&beyond), "/runs?google=busy");
        // Nothing else the server does waits for Google.
        assert_eq!(who(server, &served.ada)["person"]["username"], "ada");
        stand_in.is_slow_to_say(None);
        for leaving in started {
            leaving.join().unwrap();
        }
    });

    // The same holds for the codes browsers come back with.
    let leaving: Vec<_> = (0..=AT_ONCE)
        .map(|_| {
            let leaving = leave(server, stand_in, "");
            let code = stand_in.sign_in(&GRACE, &leaving.sent);
            (leaving, code)
        })
        .collect();
    stand_in.is_slow_to_say(Some(Asked::Who));
    std::thread::scope(|scope| {
        let (last, first) = leaving.split_last().unwrap();
        let back: Vec<_> = first
            .iter()
            .map(|(leaving, code)| scope.spawn(move || come_back(server, leaving, code)))
            .collect();
        stand_in.until_waiting(AT_ONCE);
        assert_eq!(
            location(&come_back(server, &last.0, &last.1)),
            "/?google=busy"
        );
        // By now the server knows where the sign-in is, and sends a browser
        // there without asking.
        leave(server, stand_in, "");
        stand_in.is_slow_to_say(None);
        for back in back {
            assert_eq!(location(&back.join().unwrap()), "/");
        }
    });
    // Once they are answered, signing in works as before.
    let (to, session) = sign_in(server, stand_in, &GRACE);
    assert_eq!(to, "/");
    assert_eq!(
        who(server, &session.unwrap())["person"]["username"],
        "grace"
    );
}

#[test]
fn a_token_google_did_not_make_for_this_sign_in_is_not_taken() {
    let served = served();
    let Served {
        server, stand_in, ..
    } = &served;
    add_grace(&served);
    for wrong in [
        Wrong::Signer,
        Wrong::Audience,
        Wrong::Issuer,
        Wrong::Expired,
        Wrong::Unsigned,
    ] {
        stand_in.hands_out(Some(wrong));
        let (to, session) = sign_in(server, stand_in, &GRACE);
        assert_eq!(to, "/?google=failed", "{wrong:?}");
        assert_eq!(session, None, "{wrong:?}");
        // Whoever runs the server is told why.
        server.complained("a sign-in with Google failed");
    }
    stand_in.hands_out(None);

    // A token made for another sign-in: the code is this browser's, and
    // what the token repeats is not what this browser kept.
    let leaving = leave(server, stand_in, "");
    let mut sent = leaving.sent.clone();
    sent.insert("nonce".to_owned(), "another-sign-in".to_owned());
    let code = stand_in.sign_in(&GRACE, &sent);
    let back = come_back(server, &leaving, &code);
    assert_eq!(location(&back), "/?google=failed");
    assert!(!cookies(&back).contains_key(SESSION));

    // And one that is as it should be is taken.
    assert!(sign_in(server, stand_in, &GRACE).1.is_some());
}

#[test]
fn a_person_has_a_way_to_sign_in() {
    let served = served();
    let Served { server, ada, .. } = &served;
    let add = |body: Value| post(server, "/v1/people", ada, body);
    let message = add(json!({ "username": "bob", "name": "Bob" })).error(400, "invalid");
    assert!(message.contains("one of the two"), "{message}");
    add(json!({ "username": "bob", "name": "Bob", "email": "not an address" }))
        .error(400, "invalid");
    let grace = add_grace(&served);
    add(json!({ "username": "other", "name": "Other", "email": "grace@example.com" }))
        .error(409, "conflict");

    // With a password too, either works.
    let both = json!({
        "username": "bob", "name": "Bob", "email": "bob@example.com", "password": PASSWORD
    });
    let bob = add(both).ok();
    assert_eq!(bob["password"], true);
    let entered = post(
        server,
        "/auth/sign-in",
        "",
        json!({ "username": "bob", "password": PASSWORD }),
    );
    entered.ok();
    // Another email ends what began with Google, and not what began with
    // the password.
    let with_password = format!("{SESSION}={}", cookies(&entered)[SESSION]);
    let his = format!("/v1/people/{}", bob["id"].as_str().unwrap());
    post(server, &his, ada, json!({ "email": "robert@example.com" })).ok();
    assert_eq!(who(server, &with_password)["person"]["username"], "bob");

    // Grace has no password to try, to change, or to do without her email.
    post(
        server,
        "/auth/sign-in",
        "",
        json!({ "username": "grace", "password": "" }),
    )
    .error(401, "unauthorized");
    let person = format!("/v1/people/{grace}");
    post(server, &person, ada, json!({ "email": "" })).error(400, "invalid");
    let given = json!({ "email": "", "password": PASSWORD });
    let changed = post(server, &person, ada, given).ok();
    assert_eq!(changed["email"], Value::Null);
    assert_eq!(changed["password"], true);

    // An admin who signs in with Google is one who can be left to manage
    // people.
    let hopper = json!({
        "username": "hopper", "name": "Hopper", "email": "hopper@example.com", "admin": true
    });
    add(hopper).ok();
    let me = who(server, ada)["person"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let off = post(
        server,
        &format!("/v1/people/{me}"),
        ada,
        json!({ "active": false }),
    );
    assert_eq!(off.ok()["active"], false);
}

#[test]
fn a_server_without_google_has_no_such_sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = support::build(&project);
    let store = dir.path().join("store.duckdb");
    let arguments = arguments(path(&manifest), path(&store));

    // Someone signed in with Google, on a server that then runs without it.
    let stand_in = StandIn::start();
    let server = Server::start_in(&arguments, &stand_in.environment()).unwrap();
    let code = server.printed("setup     ");
    let code = code.rsplit_once('#').unwrap().1;
    let first = post(
        &server,
        "/auth/setup",
        "",
        json!({ "code": code, "username": "ada", "name": "Ada", "password": PASSWORD }),
    );
    let ada = format!("{SESSION}={}", cookies(&first)[SESSION]);
    let grace = json!({
        "username": "grace", "name": "Grace", "email": "grace@example.com", "admin": true
    });
    post(&server, "/v1/people", &ada, grace).ok();
    let (_, session) = sign_in(&server, &stand_in, &GRACE);
    let session = session.unwrap();
    assert_eq!(who(&server, &session)["person"]["username"], "grace");
    server.stop();

    let server = Server::start_with(&arguments).unwrap();
    assert_eq!(
        who(&server, &session),
        json!({ "person": null, "setup": false, "dev": false, "google": false })
    );
    for path in ["/auth/google", "/auth/google/callback?code=1&state=2"] {
        browse(&server, "GET", path, "")
            .send()
            .error(404, "not_found");
    }
    // Ada still signs in, and cannot add someone who could not.
    let entered = post(
        &server,
        "/auth/sign-in",
        "",
        json!({ "username": "ada", "password": PASSWORD }),
    );
    let ada = format!("{SESSION}={}", cookies(&entered)[SESSION]);
    let message = post(
        &server,
        "/v1/people",
        &ada,
        json!({ "username": "bob", "name": "Bob", "email": "bob@example.com" }),
    )
    .error(400, "invalid");
    assert!(message.contains("needs a password"), "{message}");
}

#[test]
fn google_is_set_up_whole_or_the_server_does_not_start() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = support::build(&project);
    let store = dir.path().join("store.duckdb");
    let arguments = arguments(path(&manifest), path(&store));

    let half = Server::start_in(&arguments, &[("SPOOL_GOOGLE_CLIENT_ID", CLIENT_ID)]).unwrap_err();
    assert!(
        half.output
            .contains("SPOOL_GOOGLE_CLIENT_ID is set and SPOOL_GOOGLE_CLIENT_SECRET is not"),
        "{}",
        half.output
    );

    // Google sends a browser back to an address, and one the server listens
    // on for every machine is not it.
    let stand_in = StandIn::start();
    let mut everywhere = arguments.to_vec();
    everywhere[6] = "0.0.0.0:0";
    let nowhere = Server::start_in(&everywhere, &stand_in.environment()).unwrap_err();
    assert_eq!(nowhere.code, Some(2));
    assert!(
        nowhere.output.contains("--public-url"),
        "{}",
        nowhere.output
    );
    let not_one = [&arguments[..], &["--public-url", "spool.example.com"][..]].concat();
    let refused = Server::start_in(&not_one, &stand_in.environment()).unwrap_err();
    assert_eq!(refused.code, Some(2));

    everywhere.extend(["--public-url", "https://spool.example.com/"]);
    let server = Server::start_in(&everywhere, &stand_in.environment()).unwrap();
    assert_eq!(
        server.printed("google    "),
        "google    https://spool.example.com/auth/google/callback"
    );
    // The link that makes the first account is at that address too.
    let link = server.printed("setup     ");
    assert!(
        link.starts_with("setup     https://spool.example.com/setup#"),
        "{link}"
    );
    // Over HTTPS, the cookie kept at Google is one only this host can set.
    let response = browse(&server, "GET", "/auth/google", "").send();
    let kept = response.header("set-cookie").unwrap();
    assert!(
        kept.starts_with("__Host-spool_google=") && kept.contains("Secure"),
        "{kept}"
    );
    assert!(
        location(&response)
            .contains("redirect_uri=https%3A%2F%2Fspool.example.com%2Fauth%2Fgoogle%2Fcallback"),
        "{response:?}"
    );
    server.stop();

    // A server starts whether or not Google can be reached, and says so to
    // whoever tries.
    let unreachable = [
        ("SPOOL_GOOGLE_CLIENT_ID", CLIENT_ID),
        ("SPOOL_GOOGLE_CLIENT_SECRET", CLIENT_SECRET),
        ("SPOOL_GOOGLE_ISSUER", "http://127.0.0.1:9"),
    ];
    let server = Server::start_in(&arguments, &unreachable).unwrap();
    let response = browse(&server, "GET", "/auth/google?next=/runs", "").send();
    assert_eq!(location(&response), "/runs?google=unreachable");
    server.complained("a sign-in with Google failed");
}

#[test]
fn a_development_server_signs_nobody_in_with_google_either() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let stand_in = StandIn::start();
    let server = Server::start_in(
        &["dev", path(dir.path()), "--listen", "127.0.0.1:0"],
        &stand_in.environment(),
    )
    .unwrap();
    assert_eq!(who(&server, "")["google"], false);
    browse(&server, "GET", "/auth/google", "")
        .send()
        .error(404, "not_found");
}
