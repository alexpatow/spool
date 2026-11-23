//! Between leaving for Google and coming back: the browser, the request, and the token Google sends.

use super::*;

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
