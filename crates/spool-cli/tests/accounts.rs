//! People and tokens: the first account, signing in, who may manage accounts
//! and tokens, and who a change is recorded under.

mod support;

use serde_json::{Value, json};
use support::{Response, Server, path, run, succeed, text, write_things};

const PASSWORD: &str = "a long enough one";

/// What a browser keeps from a response that starts or ends a session.
fn cookie(response: &Response) -> String {
    let set = response.header("set-cookie").expect("a session cookie");
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Lax"),
        "{set}"
    );
    set.split(';').next().unwrap().to_owned()
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

/// A server with no accounts, and the code of the link it printed.
fn served(dir: &std::path::Path) -> (Server, String) {
    let project = dir.join("project");
    write_things(&project);
    let server = Server::serve(&project, dir, 1);
    let link = server.printed("setup");
    let code = link
        .rsplit_once('#')
        .expect("a code in the link")
        .1
        .to_owned();
    (server, code)
}

/// Makes the first account and returns its cookie and its ID.
fn set_up(server: &Server, code: &str) -> (String, String) {
    let first = post(
        server,
        "/auth/setup",
        "",
        json!({ "code": code, "username": "ada", "name": "Ada", "password": PASSWORD }),
    );
    let id = first.ok()["person"]["id"].as_str().unwrap().to_owned();
    (cookie(&first), id)
}

#[test]
fn the_first_account_is_made_with_the_link_the_server_printed_and_only_once() {
    let dir = tempfile::tempdir().unwrap();
    let (server, code) = served(dir.path());
    let state = browse(&server, "GET", "/auth/session", "").send().ok();
    assert_eq!(
        state,
        json!({ "person": null, "setup": true, "dev": false, "google": false })
    );

    let account =
        json!({ "code": "not-it", "username": "eve", "name": "Eve", "password": PASSWORD });
    post(&server, "/auth/setup", "", account).error(401, "unauthorized");

    let (ada, id) = set_up(&server, &code);
    let state = browse(&server, "GET", "/auth/session", &ada).send().ok();
    assert_eq!(state["person"]["id"], id.as_str());
    assert_eq!(state["person"]["admin"], true);
    assert_eq!(state["setup"], false);

    // The code worked once.
    let again = json!({ "code": code, "username": "eve", "name": "Eve", "password": PASSWORD });
    post(&server, "/auth/setup", "", again).error(401, "unauthorized");

    // A server that has accounts prints no link when it starts again.
    let lines = server.stopped();
    assert!(
        lines.iter().all(|line| !line.contains("setup")),
        "{lines:?}"
    );
    let project = dir.path().join("project");
    let server = Server::serve(&project, dir.path(), 1);
    let state = browse(&server, "GET", "/auth/session", "").send().ok();
    assert_eq!(
        state,
        json!({ "person": null, "setup": false, "dev": false, "google": false })
    );
}

#[test]
fn a_change_is_recorded_under_whoever_made_it() {
    let dir = tempfile::tempdir().unwrap();
    let (server, code) = served(dir.path());
    let (ada, id) = set_up(&server, &code);
    let minted = server.call("/v1/ids", json!({ "identities": 2 }));
    let create = |index: usize| {
        json!({ "ops": [
            { "op": "create_identity", "identity": minted["identities"][index], "entity_type": "thing" }
        ] })
    };

    // A request does not say who its author is.
    let mut named = create(0);
    named["actor"] = json!("user:someone-else");
    post(&server, "/v1/changesets", &ada, named).error(400, "invalid");

    post(&server, "/v1/changesets", &ada, create(0)).ok();
    server.call("/v1/changesets", create(1));
    let log = server.read("/v1/changesets");
    let actors: Vec<&str> = log["changesets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["changeset"]["actor"].as_str().unwrap())
        .collect();
    assert_eq!(actors, [format!("user:{id}").as_str(), "agent:api"]);
}

#[test]
fn an_admin_adds_people_and_nobody_else_does() {
    let dir = tempfile::tempdir().unwrap();
    let (server, code) = served(dir.path());
    let (ada, _) = set_up(&server, &code);
    let bob = json!({ "username": "bob", "name": "Bob", "password": "another long one" });

    // Not without signing in, and not with the token.
    post(&server, "/v1/people", "", bob.clone()).error(401, "unauthorized");
    server
        .post("/v1/people", bob.clone())
        .error(403, "forbidden");

    let added = post(&server, "/v1/people", &ada, bob.clone()).ok();
    assert_eq!(added["admin"], false);
    post(&server, "/v1/people", &ada, bob).error(409, "conflict");
    let weak = json!({ "username": "cy", "name": "Cy", "password": "short" });
    post(&server, "/v1/people", &ada, weak).error(400, "invalid");

    let sign_in = json!({ "username": "bob", "password": "another long one" });
    let signed_in = post(&server, "/auth/sign-in", "", sign_in.clone());
    signed_in.ok();
    let bob = cookie(&signed_in);
    let cy = json!({ "username": "cy", "name": "Cy", "password": PASSWORD });
    post(&server, "/v1/people", &bob, cy).error(403, "forbidden");
    // Everyone who is signed in can read who the people are: the log names them.
    let people = browse(&server, "GET", "/v1/people", &bob).send().ok();
    assert_eq!(people["people"].as_array().unwrap().len(), 2);

    // An account that is turned off is signed out, and cannot sign in.
    let id = added["id"].as_str().unwrap();
    let off = post(
        &server,
        &format!("/v1/people/{id}"),
        &ada,
        json!({ "active": false }),
    );
    assert_eq!(off.ok()["active"], false);
    browse(&server, "GET", "/v1/info", &bob)
        .send()
        .error(401, "unauthorized");
    post(&server, "/auth/sign-in", "", sign_in).error(401, "unauthorized");

    // Someone must be left to manage people.
    let ada_id = people["people"][0]["id"].as_str().unwrap();
    post(
        &server,
        &format!("/v1/people/{ada_id}"),
        &ada,
        json!({ "admin": false }),
    )
    .error(409, "conflict");
}

#[test]
fn signing_out_and_a_new_password_end_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let (server, code) = served(dir.path());
    let (first, _) = set_up(&server, &code);
    let sign_in = json!({ "username": "ada", "password": PASSWORD });
    let second = cookie(&post(&server, "/auth/sign-in", "", sign_in.clone()));

    post(
        &server,
        "/auth/sign-in",
        "",
        json!({ "username": "ada", "password": "not this one" }),
    )
    .error(401, "unauthorized");

    post(&server, "/auth/sign-out", &second, json!({})).ok();
    browse(&server, "GET", "/v1/info", &second)
        .send()
        .error(401, "unauthorized");

    let third = cookie(&post(&server, "/auth/sign-in", "", sign_in.clone()));
    let wrong = json!({ "current": "not this one", "new": "the next long one" });
    post(&server, "/auth/password", &third, wrong).error(400, "invalid");
    let changed = post(
        &server,
        "/auth/password",
        &third,
        json!({ "current": PASSWORD, "new": "the next long one" }),
    );
    changed.ok();
    // The session that changed it goes on under a new ID. The others ended.
    browse(&server, "GET", "/v1/info", &cookie(&changed))
        .send()
        .ok();
    for ended in [&first, &third] {
        browse(&server, "GET", "/v1/info", ended)
            .send()
            .error(401, "unauthorized");
    }
    post(&server, "/auth/sign-in", "", sign_in).error(401, "unauthorized");
}

#[test]
fn a_write_from_another_site_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (server, code) = served(dir.path());
    let (ada, _) = set_up(&server, &code);
    let own = format!("http://{}", server.address);
    for (origin, status) in [(own.as_str(), 409), ("http://elsewhere.example", 403)] {
        let response = browse(&server, "POST", "/v1/changesets", &ada)
            .header("Origin", origin)
            .json(&json!({ "ops": [] }))
            .send();
        assert_eq!(response.status, status, "{origin}");
    }
    browse(&server, "POST", "/auth/sign-in", "")
        .header("Origin", "http://elsewhere.example")
        .json(&json!({ "username": "ada", "password": PASSWORD }))
        .send()
        .error(403, "forbidden");

    // What the browser says of the page goes before the address it names:
    // a server behind another may not be sent the host its readers typed.
    for (site, status) in [
        ("same-origin", 409),
        ("same-site", 403),
        ("cross-site", 403),
    ] {
        let response = browse(&server, "POST", "/v1/changesets", &ada)
            .header("Origin", "https://spool.example")
            .header("Sec-Fetch-Site", site)
            .json(&json!({ "ops": [] }))
            .send();
        assert_eq!(response.status, status, "{site}");
    }
}

#[test]
fn nobody_signs_in_to_a_development_server() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let server = Server::start_with(&["dev", path(dir.path()), "--listen", "127.0.0.1:0"]).unwrap();
    let state = browse(&server, "GET", "/auth/session", "").send().ok();
    assert_eq!(state["person"]["username"], "dev");
    assert_eq!(state["setup"], false);
    assert_eq!(state["dev"], true);
    let dev = state["person"]["id"].as_str().unwrap();

    let minted = post(&server, "/v1/ids", "", json!({ "identities": 1 })).ok();
    let create = json!({ "ops": [
        { "op": "create_identity", "identity": minted["identities"][0], "entity_type": "thing" }
    ] });
    let committed = post(&server, "/v1/changesets", "", create).ok();
    assert_eq!(committed["seq"], 1);
    let log = server.read("/v1/changesets");
    assert_eq!(
        log["changesets"][0]["changeset"]["actor"],
        format!("user:{dev}").as_str()
    );
}

#[test]
fn a_store_that_was_developed_on_still_gets_its_first_account() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let store = dir.path().join("store.duckdb");
    let developed = Server::start_with(&[
        "dev",
        path(&project),
        "--store",
        path(&store),
        "--listen",
        "127.0.0.1:0",
    ])
    .unwrap();
    let state = browse(&developed, "GET", "/auth/session", "").send().ok();
    let dev = state["person"]["id"].as_str().unwrap().to_owned();
    developed.stop();

    // `dev` has no password, so nobody could sign in: the link is printed.
    let (server, code) = served(dir.path());
    let state = browse(&server, "GET", "/auth/session", "").send().ok();
    assert_eq!(
        state,
        json!({ "person": null, "setup": true, "dev": false, "google": false })
    );
    let (ada, id) = set_up(&server, &code);

    // What `dev` changed keeps its name.
    let people = browse(&server, "GET", "/v1/people", &ada).send().ok();
    let ids: Vec<&str> = people["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|person| person["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [dev.as_str(), id.as_str()]);

    // An admin nobody can sign in as is not one who is left to manage people.
    post(
        &server,
        &format!("/v1/people/{id}"),
        &ada,
        json!({ "admin": false }),
    )
    .error(409, "conflict");
}

#[test]
fn only_so_many_passwords_are_checked_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (server, code) = served(dir.path());
    set_up(&server, &code);

    // Nobody the server knows, all at once: each would cost it a hash.
    let answers: Vec<Response> = std::thread::scope(|scope| {
        let tries: Vec<_> = (0..32)
            .map(|index| {
                let server = &server;
                scope.spawn(move || {
                    let guess =
                        json!({ "username": format!("guess-{index}"), "password": "not this one" });
                    post(server, "/auth/sign-in", "", guess)
                })
            })
            .collect();
        tries
            .into_iter()
            .map(|tried| tried.join().unwrap())
            .collect()
    });
    let (checked, turned_away): (Vec<Response>, Vec<Response>) =
        answers.into_iter().partition(|answer| answer.status == 401);
    assert!(!checked.is_empty());
    assert!(!turned_away.is_empty());
    for answer in turned_away {
        answer.error(429, "too_many_attempts");
    }

    // Someone who has an account signs in once the rush is over.
    let sign_in = json!({ "username": "ada", "password": PASSWORD });
    post(&server, "/auth/sign-in", "", sign_in).ok();
}

#[test]
fn a_body_too_large_to_be_about_an_account_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let (server, code) = served(dir.path());
    // Over what is read by the few bytes around it, so that the server has
    // taken all of it in by the time it answers.
    let long = "x".repeat(64 * 1024);
    // Before anyone has an account, and so from anyone at all.
    for (path, body) in [
        (
            "/auth/sign-in",
            json!({ "username": "ada", "password": long }),
        ),
        (
            "/auth/setup",
            json!({ "code": code, "username": "ada", "name": "Ada", "password": long }),
        ),
        (
            "/auth/password",
            json!({ "current": long, "new": PASSWORD }),
        ),
    ] {
        post(&server, path, "", body).error(413, "too_large");
    }
    // The code was not used up.
    set_up(&server, &code);
}

/// A request that presents `token` and nothing else.
fn present<'a>(server: &'a Server, method: &str, path: &str, token: &str) -> support::Request<'a> {
    server
        .request(method, path)
        .header("Authorization", &format!("Bearer {token}"))
}

/// Commits a changeset that creates one identity as whoever presents
/// `token`, and returns who the log says made it.
fn author(server: &Server, token: &str) -> String {
    let minted = present(server, "POST", "/v1/ids", token)
        .json(&json!({ "identities": 1 }))
        .send()
        .ok();
    let create = json!({ "ops": [
        { "op": "create_identity", "identity": minted["identities"][0], "entity_type": "thing" }
    ] });
    let committed = present(server, "POST", "/v1/changesets", token)
        .json(&create)
        .send()
        .ok();
    let log = server.read("/v1/changesets");
    let entry = log["changesets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["seq"] == committed["seq"])
        .unwrap();
    entry["changeset"]["actor"].as_str().unwrap().to_owned()
}

fn token_names(list: &Value) -> Vec<&str> {
    list["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|token| token["name"].as_str().unwrap())
        .collect()
}

#[test]
fn an_admin_makes_tokens_and_each_writes_under_its_own_name() {
    let dir = tempfile::tempdir().unwrap();
    let (server, code) = served(dir.path());
    let (ada, id) = set_up(&server, &code);
    let bob = json!({ "username": "bob", "name": "Bob", "password": "another long one" });
    post(&server, "/v1/people", &ada, bob).ok();
    let sign_in = json!({ "username": "bob", "password": "another long one" });
    let bob = cookie(&post(&server, "/auth/sign-in", "", sign_in));

    let added = post(&server, "/v1/tokens", &ada, json!({ "name": "Ingest" })).ok();
    let secret = added["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("spool_"), "{secret}");
    assert_eq!(added["token"]["name"], "ingest");
    assert_eq!(added["token"]["created_by"], id.as_str());
    assert_eq!(added["token"]["last_used_at"], Value::Null);
    let token_id = added["token"]["id"].as_str().unwrap();

    // A name is one token's, and is what the log will say.
    post(&server, "/v1/tokens", &ada, json!({ "name": "ingest" })).error(409, "conflict");
    for name in ["", "two words", "agent:ingest"] {
        post(&server, "/v1/tokens", &ada, json!({ "name": name })).error(400, "invalid");
    }

    // What it changes is recorded under its name.
    let wrote = author(&server, &secret);
    assert_eq!(wrote, "agent:ingest");

    // The list says it was used, and never what the token is. The token the
    // tests call with was made on this machine, by nobody.
    let listed = browse(&server, "GET", "/v1/tokens", &ada).send();
    assert!(!text(&listed.body).contains(&secret));
    let listed = listed.ok();
    assert_eq!(token_names(&listed), ["api", "ingest"]);
    assert_eq!(listed["tokens"][0]["created_by"], Value::Null);
    assert!(listed["tokens"][1]["last_used_at"].is_string(), "{listed}");

    // Only an admin sees or manages tokens: not someone else who is signed
    // in, not a token, and not a stranger.
    let revoke = format!("/v1/tokens/{token_id}/revoke");
    let another = json!({ "name": "another" });
    browse(&server, "GET", "/v1/tokens", &bob)
        .send()
        .error(403, "forbidden");
    post(&server, "/v1/tokens", &bob, another.clone()).error(403, "forbidden");
    post(&server, &revoke, &bob, json!({})).error(403, "forbidden");
    present(&server, "GET", "/v1/tokens", &secret)
        .send()
        .error(403, "forbidden");
    present(&server, "POST", "/v1/tokens", &secret)
        .json(&another)
        .send()
        .error(403, "forbidden");
    present(&server, "POST", &revoke, &secret)
        .send()
        .error(403, "forbidden");
    browse(&server, "GET", "/v1/tokens", "")
        .send()
        .error(401, "unauthorized");
    post(&server, "/v1/tokens", "", another).error(401, "unauthorized");

    // A revoked token is refused from then on, and the others are not.
    let revoked = post(&server, &revoke, &ada, json!({})).ok();
    assert_eq!(revoked["name"], "ingest");
    present(&server, "GET", "/v1/info", &secret)
        .send()
        .error(401, "unauthorized");
    server.read("/v1/info");
    post(&server, &revoke, &ada, json!({})).error(404, "not_found");
    let listed = browse(&server, "GET", "/v1/tokens", &ada).send().ok();
    assert_eq!(token_names(&listed), ["api"]);
}

#[test]
fn tokens_are_made_and_revoked_on_the_servers_machine_while_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = served(dir.path());
    let store = dir.path().join("store.duckdb");
    let store = path(&store);

    // The token is all that is printed where a script reads it.
    let added = run(&["token", "add", "ci", "--store", store]);
    assert!(added.status.success(), "{}", text(&added.stderr));
    let printed = text(&added.stdout);
    let secret = printed.trim_end();
    assert!(
        secret.starts_with("spool_") && printed == format!("{secret}\n") && !secret.contains(' '),
        "{printed}"
    );
    assert!(text(&added.stderr).contains("agent:ci"));
    let wrote = author(&server, secret);
    assert_eq!(wrote, "agent:ci");

    let listed = succeed(&["token", "list", "--store", store]);
    let lines: Vec<&str> = listed.lines().collect();
    assert_eq!(lines.len(), 3, "{listed}");
    assert!(lines[0].starts_with("token"), "{listed}");
    assert!(
        lines[1].starts_with("api ") && lines[2].starts_with("ci "),
        "{listed}"
    );
    assert!(!listed.contains(secret));
    let listed: Value =
        serde_json::from_str(&succeed(&["token", "list", "--json", "--store", store])).unwrap();
    assert_eq!(token_names(&listed), ["api", "ci"]);

    // The store can be named in the environment, as it is in a container.
    let named = support::spool()
        .args(["token", "add", "ci"])
        .env("SPOOL_STORE", store)
        .output()
        .unwrap();
    assert_eq!(named.status.code(), Some(1));
    assert!(
        text(&named.stderr).contains("`ci` already"),
        "{}",
        text(&named.stderr)
    );
    assert_eq!(text(&named.stdout), "");

    let revoked = succeed(&["token", "revoke", "ci", "--store", store]);
    assert!(revoked.contains("revoked `ci`"), "{revoked}");
    present(&server, "GET", "/v1/info", secret)
        .send()
        .error(401, "unauthorized");
    let again = run(&["token", "revoke", "ci", "--store", store]);
    assert_eq!(again.status.code(), Some(1));
    assert!(text(&again.stderr).contains("no such token"));

    // A token beside a store that is not there would open nothing.
    let nowhere = dir.path().join("typo.duckdb");
    let missing = run(&["token", "add", "ci", "--store", path(&nowhere)]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(text(&missing.stderr).contains("there is no store at"));
    assert!(!dir.path().join("typo.auth.sqlite").exists());

    for (arguments, complaint) in [
        (&["token"][..], "say what to do"),
        (&["token", "add"], "needs the token's name"),
        (&["token", "frob", "ci"], "not `frob`"),
        (&["token", "list", "ci"], "unexpected argument `ci`"),
        (&["token", "add", "ci"], "SPOOL_STORE"),
    ] {
        let refused = run(arguments);
        assert_eq!(refused.status.code(), Some(2), "{arguments:?}");
        assert!(
            text(&refused.stderr).contains(complaint),
            "{arguments:?}: {}",
            text(&refused.stderr)
        );
    }
}

#[test]
fn a_development_server_asks_for_no_token_and_takes_no_made_up_one() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let server = Server::start_with(&["dev", path(dir.path()), "--listen", "127.0.0.1:0"]).unwrap();
    server.read("/v1/info");
    // What a caller presents is checked, here as anywhere.
    for made_up in ["dev", "spool_dev", "anything"] {
        present(&server, "GET", "/v1/info", made_up)
            .send()
            .error(401, "unauthorized");
    }
    // `dev` is an admin, and a token it makes writes under its own name.
    let added = server.call("/v1/tokens", json!({ "name": "review-bot" }));
    let secret = added["secret"].as_str().unwrap();
    let wrote = author(&server, secret);
    assert_eq!(wrote, "agent:review-bot");
}

#[test]
fn a_development_server_answers_only_to_the_names_of_its_machine() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let server = Server::start_with(&["dev", path(dir.path()), "--listen", "127.0.0.1:0"]).unwrap();
    let port = server.address.rsplit_once(':').unwrap().1;
    let reads = ["/v1/info", "/auth/session", "/healthz", "/", "/studio.js"];
    let minted = json!({ "identities": 1 });
    let listed = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let writes = [("/v1/ids", &minted), ("/mcp", &listed)];
    let write = |at: &str, body: &Value, host: &str| {
        browse(&server, "POST", at, "")
            .header("Accept", "application/json, text/event-stream")
            .header("Host", host)
            .json(body)
            .send()
    };

    // A page of another site reaches this machine by giving it a name of
    // its own, and nothing else about what it sends says where it is from.
    // Nobody signs in here, so that name is all there is to refuse it by.
    for name in [
        "elsewhere.example",
        "localhost.elsewhere.example",
        "127.0.0.1.elsewhere.example",
        "192.168.1.5",
    ] {
        let host = format!("{name}:{port}");
        for at in reads {
            let refused = browse(&server, "GET", at, "").header("Host", &host).send();
            let said = refused.error(403, "forbidden");
            assert!(said.contains("localhost"), "{at}: {said}");
        }
        for (at, body) in writes {
            write(at, body, &host).error(403, "forbidden");
        }
    }
    // Nor is a request that names nothing.
    browse(&server, "GET", "/v1/info", "")
        .header("Host", "")
        .send()
        .error(403, "forbidden");

    for name in ["localhost", "LocalHost", "127.0.0.1", "[::1]"] {
        let host = format!("{name}:{port}");
        for at in reads {
            let answered = browse(&server, "GET", at, "").header("Host", &host).send();
            assert_eq!(answered.status, 200, "{at} as {host}");
        }
        for (at, body) in writes {
            assert_eq!(write(at, body, &host).status, 200, "{at} as {host}");
        }
    }
}

#[test]
fn a_server_people_sign_in_to_answers_by_whatever_name_it_is_reached() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = served(dir.path());
    let reached = server
        .request("GET", "/v1/info")
        .header("Host", "spool.example.com");
    reached.send().ok();
    let reached = browse(&server, "GET", "/auth/session", "").header("Host", "spool.example.com");
    reached.send().ok();
}
