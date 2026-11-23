//! Which account a sign-in with Google is: by the email an admin added, and the address Google answers for.

use super::*;

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
