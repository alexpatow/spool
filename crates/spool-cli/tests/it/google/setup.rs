//! Which servers sign people in with Google, and the way each person has to sign in.

use super::*;

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
