//! People: the first account, signing in, who may manage accounts, and who a
//! change is recorded under.

mod support;

use serde_json::{Value, json};
use support::{Response, Server, path, write_things};

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
        json!({ "person": null, "setup": true, "dev": false })
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
        json!({ "person": null, "setup": false, "dev": false })
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
    let server =
        Server::start_with(&["dev", path(dir.path()), "--listen", "127.0.0.1:0"], None).unwrap();
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
    let developed = Server::start_with(
        &[
            "dev",
            path(&project),
            "--store",
            path(&store),
            "--listen",
            "127.0.0.1:0",
        ],
        None,
    )
    .unwrap();
    let state = browse(&developed, "GET", "/auth/session", "").send().ok();
    let dev = state["person"]["id"].as_str().unwrap().to_owned();
    developed.stop();

    // `dev` has no password, so nobody could sign in: the link is printed.
    let (server, code) = served(dir.path());
    let state = browse(&server, "GET", "/auth/session", "").send().ok();
    assert_eq!(
        state,
        json!({ "person": null, "setup": true, "dev": false })
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
