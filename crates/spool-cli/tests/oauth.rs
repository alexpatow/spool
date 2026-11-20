//! An assistant that signs in with OAuth, as Claude and ChatGPT on the web
//! do: it finds the server's metadata after a `401` at `/mcp`, registers
//! itself, has a person approve it in the studio, exchanges the code for
//! tokens and calls `/mcp` with them. The test is both the assistant and
//! the person's browser.

mod support;

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{Request, Response, Server, path, write_things};

const PASSWORD: &str = "a long enough one";
const CLAUDE: &str = "https://claude.ai/api/mcp/auth_callback";
const VERIFIER: &str = "the-verifier-an-assistant-keeps-to-itself-until-it-has-a-code";
const ACCEPT: &str = "application/json, text/event-stream";

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier))
}

/// Fields as a query or a form writes them.
fn encoded(fields: &[(&str, &str)]) -> String {
    let escape = |text: &str| {
        text.bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (byte as char).to_string()
                }
                _ => format!("%{byte:02X}"),
            })
            .collect::<String>()
    };
    fields
        .iter()
        .map(|(name, value)| format!("{}={}", escape(name), escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// What a query says.
fn query(address: &str) -> HashMap<String, String> {
    let query = address.split_once('?').map_or("", |(_, query)| query);
    query
        .split('&')
        .filter_map(|field| field.split_once('='))
        .map(|(name, value)| {
            let mut bytes = Vec::new();
            let raw = value.replace('+', " ");
            let raw = raw.as_bytes();
            let mut at = 0;
            while at < raw.len() {
                if raw[at] == b'%' {
                    let pair = std::str::from_utf8(&raw[at + 1..at + 3]).unwrap();
                    bytes.push(u8::from_str_radix(pair, 16).unwrap());
                    at += 3;
                } else {
                    bytes.push(raw[at]);
                    at += 1;
                }
            }
            (name.to_owned(), String::from_utf8(bytes).unwrap())
        })
        .collect()
}

/// A request as a caller with nothing to present sends it.
fn bare<'a>(server: &'a Server, method: &str, at: &str) -> Request<'a> {
    server.request(method, at).without("Authorization")
}

fn form(server: &Server, at: &str, fields: &[(&str, &str)]) -> Response {
    bare(server, "POST", at)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(encoded(fields).as_bytes())
        .send()
}

/// A server on a new store, with Ada as its first account, signed in.
struct Setup {
    server: Server,
    issuer: String,
    /// Ada's session cookie.
    ada: String,
    /// Ada's ID.
    id: String,
}

fn set_up(dir: &std::path::Path) -> Setup {
    let project = dir.join("project");
    write_things(&project);
    let server = Server::serve(&project, dir, 1);
    let link = server.printed("setup");
    let code = link.rsplit_once('#').unwrap().1;
    let account = json!({ "code": code, "username": "ada", "name": "Ada", "password": PASSWORD });
    let first = bare(&server, "POST", "/auth/setup").json(&account).send();
    let id = first.ok()["person"]["id"].as_str().unwrap().to_owned();
    let ada = first
        .header("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let issuer = format!("http://{}", server.address);
    Setup {
        server,
        issuer,
        ada,
        id,
    }
}

/// Registers a client the way Claude does, and returns its ID.
fn register(server: &Server, name: &str, redirect: &str) -> String {
    let registration = json!({
        "client_name": name,
        "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let registered = bare(server, "POST", "/oauth/register")
        .json(&registration)
        .send();
    let registered = registered.expect(201);
    assert_eq!(registered["token_endpoint_auth_method"], "none");
    assert_eq!(registered["redirect_uris"], json!([redirect]));
    registered["client_id"].as_str().unwrap().to_owned()
}

/// The query of `/oauth/authorize` as a client sends the person there.
fn authorization(setup: &Setup, client: &str, redirect: &str, state: &str) -> String {
    encoded(&[
        ("response_type", "code"),
        ("client_id", client),
        ("redirect_uri", redirect),
        ("code_challenge", &challenge(VERIFIER)),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("scope", "spool"),
        ("resource", &format!("{}/mcp", setup.issuer)),
    ])
}

/// Ada approves, or turns away, a client in the studio, and this is where
/// the studio sends her browser.
fn decide(setup: &Setup, request: &str, approve: bool) -> String {
    let decision = json!({ "request": request, "approve": approve });
    let decided = bare(&setup.server, "POST", "/auth/oauth")
        .header("Cookie", &setup.ada)
        .json(&decision)
        .send()
        .ok();
    decided["to"].as_str().unwrap().to_owned()
}

/// The tokens a code is exchanged for.
fn exchange(setup: &Setup, client: &str, code: &str) -> Value {
    let resource = format!("{}/mcp", setup.issuer);
    let fields = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", CLAUDE),
        ("client_id", client),
        ("code_verifier", VERIFIER),
        ("resource", resource.as_str()),
    ];
    let tokens = form(&setup.server, "/oauth/token", &fields);
    assert_eq!(tokens.header("cache-control"), Some("no-store"));
    tokens.ok()
}

fn mcp<'a>(server: &'a Server, token: &str, body: &Value) -> Request<'a> {
    bare(server, "POST", "/mcp")
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", ACCEPT)
        .json(body)
}

/// Calls a tool with an access token, and returns what it answered.
fn call(server: &Server, token: &str, name: &str, arguments: Value) -> Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let answer = mcp(server, token, &body).send().ok();
    let result = &answer["result"];
    assert_ne!(result["isError"], true, "{answer}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// The whole of it: from the first `401` to a change in the log under the
/// person who approved, and on to refreshing, a stolen refresh token, and
/// an admin taking the approval back.
#[test]
#[expect(clippy::too_many_lines)]
fn an_assistant_signs_in_and_works_as_the_person_who_approved_it() {
    let dir = tempfile::tempdir().unwrap();
    let setup = set_up(dir.path());
    let server = &setup.server;
    let issuer = setup.issuer.as_str();

    // The assistant is refused, and told where to read how to sign in.
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let refused = bare(server, "POST", "/mcp")
        .header("Accept", ACCEPT)
        .json(&list)
        .send();
    refused.error(401, "unauthorized");
    let challenge_said = refused.header("www-authenticate").unwrap().to_owned();
    let metadata = format!("{issuer}/.well-known/oauth-protected-resource/mcp");
    assert!(
        challenge_said.contains(&format!("resource_metadata=\"{metadata}\"")),
        "{challenge_said}"
    );

    // The resource's metadata names this server as the one to sign in with,
    // and the server's says how.
    let path_of = |url: &str| url.strip_prefix(issuer).unwrap().to_owned();
    for at in [
        &metadata,
        &format!("{issuer}/.well-known/oauth-protected-resource"),
    ] {
        let resource = bare(server, "GET", &path_of(at)).send().ok();
        assert_eq!(resource["resource"], format!("{issuer}/mcp"));
        assert_eq!(resource["authorization_servers"], json!([issuer]));
    }
    let said = bare(server, "GET", "/.well-known/oauth-authorization-server")
        .send()
        .ok();
    assert_eq!(said["issuer"], issuer);
    assert_eq!(said["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(
        said["token_endpoint_auth_methods_supported"],
        json!(["none"])
    );
    assert_eq!(said["authorization_response_iss_parameter_supported"], true);
    for (endpoint, at) in [
        ("authorization_endpoint", "/oauth/authorize"),
        ("token_endpoint", "/oauth/token"),
        ("registration_endpoint", "/oauth/register"),
        ("revocation_endpoint", "/oauth/revoke"),
    ] {
        assert_eq!(said[endpoint], format!("{issuer}{at}"));
    }

    let client = register(server, "Claude", CLAUDE);

    // The person's browser is sent to the studio, which asks them to sign
    // in and shows what is asked.
    let request = authorization(&setup, &client, CLAUDE, "the-state");
    let page = bare(server, "GET", &format!("/oauth/authorize?{request}")).send();
    assert_eq!(page.status, 200);
    assert!(
        page.header("content-type")
            .unwrap()
            .starts_with("text/html")
    );
    let shown = bare(server, "GET", &format!("/auth/oauth?{request}"))
        .header("Cookie", &setup.ada)
        .send()
        .ok();
    assert_eq!(
        shown,
        json!({ "client_name": "Claude", "redirect_uri": CLAUDE, "host": "claude.ai", "loopback": false })
    );

    // Only someone signed in approves, and only on the studio's own page.
    let decision = json!({ "request": request, "approve": true });
    let nobody = bare(server, "POST", "/auth/oauth").json(&decision).send();
    nobody.error(401, "unauthorized");
    let elsewhere = bare(server, "POST", "/auth/oauth")
        .header("Cookie", &setup.ada)
        .header("Sec-Fetch-Site", "cross-site")
        .json(&decision)
        .send();
    elsewhere.error(403, "forbidden");

    let to = decide(&setup, &request, true);
    assert!(to.starts_with(&format!("{CLAUDE}?")), "{to}");
    let back = query(&to);
    assert_eq!(back["state"], "the-state");
    assert_eq!(back["iss"], issuer);
    let code = back["code"].clone();

    // The code is no good without the verifier.
    let resource = format!("{issuer}/mcp");
    let wrong = form(
        server,
        "/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", CLAUDE),
            ("client_id", &client),
            ("code_verifier", &"x".repeat(43)),
            ("resource", &resource),
        ],
    );
    assert_eq!(wrong.expect(400)["error"], "invalid_grant");

    let tokens = exchange(&setup, &client, &code);
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["expires_in"], 3600);
    assert_eq!(tokens["scope"], "spool");
    let access = tokens["access_token"].as_str().unwrap().to_owned();

    // The assistant works at `/mcp` as Ada, and what it changes is hers.
    let minted = call(server, &access, "mint_identity_ids", json!({ "count": 1 }));
    let change = json!({
        "ops": [{ "op": "create_identity", "identity": minted["identities"][0], "entity_type": "thing" }],
        "reason": "asked to in a conversation",
    });
    let committed = call(server, &access, "commit_changeset", change);
    let after = committed["seq"].as_u64().unwrap() - 1;
    let log = bare(
        server,
        "GET",
        &format!("/v1/changesets?after={after}&limit=1"),
    )
    .header("Cookie", &setup.ada)
    .send()
    .ok();
    let entry = &log["changesets"][0]["changeset"];
    assert_eq!(entry["actor"], format!("user:{}", setup.id));
    assert_eq!(entry["reason"], "asked to in a conversation");

    // The token is for `/mcp`, and nowhere else.
    let elsewhere = bare(server, "GET", "/v1/info")
        .header("Authorization", &format!("Bearer {access}"))
        .send();
    elsewhere.error(401, "unauthorized");

    // A code is exchanged once.
    let again = form(
        server,
        "/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", CLAUDE),
            ("client_id", &client),
            ("code_verifier", VERIFIER),
        ],
    );
    assert_eq!(again.expect(400)["error"], "invalid_grant");
    // And a second try ended what the first gave, in case the first was not
    // the assistant's.
    mcp(server, &access, &list)
        .send()
        .error(401, "unauthorized");

    // Again, and this time refresh.
    let code = query(&decide(&setup, &request, true))["code"].clone();
    let tokens = exchange(&setup, &client, &code);
    let refresh_with = |token: &str| {
        form(
            server,
            "/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", token),
                ("client_id", &client),
                ("resource", &resource),
            ],
        )
    };
    let first = tokens["refresh_token"].as_str().unwrap().to_owned();
    let refreshed = refresh_with(&first).ok();
    let access = refreshed["access_token"].as_str().unwrap().to_owned();
    assert_ne!(refreshed["refresh_token"], tokens["refresh_token"]);
    call(server, &access, "describe_store", json!({}));

    // A refresh token that was used already is someone else's copy: the
    // approval ends, for both of them.
    assert_eq!(refresh_with(&first).expect(400)["error"], "invalid_grant");
    let ended = mcp(server, &access, &list).send();
    ended.error(401, "unauthorized");
    assert!(
        ended
            .header("www-authenticate")
            .unwrap()
            .contains("invalid_token")
    );
    let next = refreshed["refresh_token"].as_str().unwrap();
    assert_eq!(refresh_with(next).expect(400)["error"], "invalid_grant");

    // An admin sees every approval, and takes one back.
    let code = query(&decide(&setup, &request, true))["code"].clone();
    let access = exchange(&setup, &client, &code)["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    call(server, &access, "describe_store", json!({}));
    let grants = bare(server, "GET", "/v1/grants")
        .header("Cookie", &setup.ada)
        .send()
        .ok();
    let grants = grants["grants"].as_array().unwrap().clone();
    assert_eq!(grants.len(), 1, "{grants:?}");
    assert_eq!(grants[0]["person"], setup.id);
    assert_eq!(grants[0]["client_name"], "Claude");
    assert_eq!(grants[0]["redirect_uri"], CLAUDE);
    assert!(grants[0]["last_used_at"].is_string());
    // A token cannot see them.
    server.get("/v1/grants").error(403, "forbidden");
    let at = format!("/v1/grants/{}/revoke", grants[0]["id"].as_str().unwrap());
    bare(server, "POST", &at)
        .header("Cookie", &setup.ada)
        .json(&json!({}))
        .send()
        .ok();
    mcp(server, &access, &list)
        .send()
        .error(401, "unauthorized");

    // A client takes back its own tokens.
    let code = query(&decide(&setup, &request, true))["code"].clone();
    let tokens = exchange(&setup, &client, &code);
    let revoked = form(
        server,
        "/oauth/revoke",
        &[
            ("token", tokens["refresh_token"].as_str().unwrap()),
            ("client_id", &client),
        ],
    );
    assert_eq!(revoked.status, 200);
    let access = tokens["access_token"].as_str().unwrap();
    mcp(server, access, &list).send().error(401, "unauthorized");
}

#[test]
fn a_person_who_turns_the_assistant_away_sends_it_back_with_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let setup = set_up(dir.path());
    let client = register(&setup.server, "Claude", CLAUDE);
    let request = authorization(&setup, &client, CLAUDE, "s");
    let back = query(&decide(&setup, &request, false));
    assert_eq!(back["error"], "access_denied");
    assert_eq!(back["state"], "s");
    assert_eq!(back["iss"], setup.issuer);
    assert!(!back.contains_key("code"));
}

#[test]
fn a_request_that_cannot_be_approved_is_answered_where_it_can_be() {
    let dir = tempfile::tempdir().unwrap();
    let setup = set_up(dir.path());
    let server = &setup.server;
    let client = register(server, "Claude", CLAUDE);
    let challenge = challenge(VERIFIER);

    // Wrong in a way the client can be told: the browser goes back to it.
    for (wrong, error) in [
        (
            vec![("response_type", "token")],
            "unsupported_response_type",
        ),
        (vec![("code_challenge_method", "plain")], "invalid_request"),
        (vec![("code_challenge", "")], "invalid_request"),
        (
            vec![("resource", "https://elsewhere.example/mcp")],
            "invalid_target",
        ),
    ] {
        let mut fields = vec![
            ("response_type", "code"),
            ("client_id", client.as_str()),
            ("redirect_uri", CLAUDE),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "s"),
        ];
        for (name, value) in &wrong {
            fields.retain(|(existing, _)| existing != name);
            fields.push((name, value));
        }
        let sent = bare(
            server,
            "GET",
            &format!("/oauth/authorize?{}", encoded(&fields)),
        )
        .send();
        assert_eq!(sent.status, 303, "{wrong:?}");
        let back = query(sent.header("location").unwrap());
        assert_eq!(back["error"], error, "{wrong:?}");
        assert_eq!(back["iss"], setup.issuer);
        assert_eq!(back["state"], "s");
    }

    // Wrong in a way that leaves nowhere safe to send it: the studio shows
    // it, and nobody else is told.
    let elsewhere = authorization(&setup, &client, "https://attacker.example/callback", "s");
    let unknown = authorization(&setup, "not-registered", CLAUDE, "s");
    for request in [elsewhere, unknown] {
        let page = bare(server, "GET", &format!("/oauth/authorize?{request}")).send();
        assert_eq!(page.status, 200);
        let shown = bare(server, "GET", &format!("/auth/oauth?{request}"))
            .header("Cookie", &setup.ada)
            .send();
        shown.error(400, "invalid");
        let decision = json!({ "request": request, "approve": true });
        let refused = bare(server, "POST", "/auth/oauth")
            .header("Cookie", &setup.ada)
            .json(&decision)
            .send();
        refused.error(400, "invalid");
    }

    // A registration that would send an approval somewhere unsafe is refused.
    for redirect in ["http://claude.ai/callback", "claude://callback"] {
        let registration = json!({ "redirect_uris": [redirect] });
        let refused = bare(server, "POST", "/oauth/register")
            .json(&registration)
            .send();
        assert_eq!(refused.expect(400)["error"], "invalid_redirect_uri");
    }
}

#[test]
fn claude_code_is_approved_on_whatever_port_it_listens() {
    let dir = tempfile::tempdir().unwrap();
    let setup = set_up(dir.path());
    let client = register(&setup.server, "Claude Code", "http://localhost/callback");
    let request = authorization(&setup, &client, "http://localhost:51234/callback", "s");
    let shown = bare(&setup.server, "GET", &format!("/auth/oauth?{request}"))
        .header("Cookie", &setup.ada)
        .send()
        .ok();
    assert_eq!(shown["loopback"], true);
    let to = decide(&setup, &request, true);
    assert!(
        to.starts_with("http://localhost:51234/callback?code="),
        "{to}"
    );
}

#[test]
fn an_approval_ends_when_its_password_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let setup = set_up(dir.path());
    let server = &setup.server;
    let client = register(server, "Claude", CLAUDE);
    let request = authorization(&setup, &client, CLAUDE, "s");
    let code = query(&decide(&setup, &request, true))["code"].clone();
    let access = exchange(&setup, &client, &code)["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    mcp(server, &access, &list).send().ok();
    let new = json!({ "current": PASSWORD, "new": "the next long one" });
    bare(server, "POST", "/auth/password")
        .header("Cookie", &setup.ada)
        .json(&new)
        .send()
        .ok();
    mcp(server, &access, &list)
        .send()
        .error(401, "unauthorized");
}

/// A server that does not know an HTTPS address it is reached by has no
/// OAuth, and says so when it starts.
#[test]
fn a_server_without_an_https_address_does_not_sign_assistants_in() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = support::build(&project);
    let store = dir.path().join("store.duckdb");
    let server = Server::start(&[
        "serve",
        "--manifest",
        path(&manifest),
        "--store",
        path(&store),
        "--listen",
        "127.0.0.1:0",
        "--public-url",
        "http://spool.example.com",
    ]);
    let said = server.printed("oauth");
    assert!(said.contains("off") && said.contains("HTTPS"), "{said}");
    for at in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-authorization-server",
        "/oauth/authorize",
    ] {
        bare(&server, "GET", at).send().error(404, "not_found");
    }
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let refused = bare(&server, "POST", "/mcp")
        .header("Accept", ACCEPT)
        .json(&list)
        .send();
    refused.error(401, "unauthorized");
    assert_eq!(refused.header("www-authenticate"), Some("Bearer"));
}

/// Behind a proxy that ends TLS, the metadata names the address the operator
/// set, not the one the server listens on.
#[test]
fn the_issuer_is_the_public_address() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = support::build(&project);
    let store = dir.path().join("store.duckdb");
    let server = Server::start(&[
        "serve",
        "--manifest",
        path(&manifest),
        "--store",
        path(&store),
        "--listen",
        "127.0.0.1:0",
        "--public-url",
        "https://spool.example.com/",
    ]);
    let resource = bare(&server, "GET", "/.well-known/oauth-protected-resource/mcp")
        .send()
        .ok();
    assert_eq!(resource["resource"], "https://spool.example.com/mcp");
    assert_eq!(
        resource["authorization_servers"],
        json!(["https://spool.example.com"])
    );
    let said = bare(&server, "GET", "/.well-known/oauth-authorization-server")
        .send()
        .ok();
    assert_eq!(
        said["token_endpoint"],
        "https://spool.example.com/oauth/token"
    );
}
