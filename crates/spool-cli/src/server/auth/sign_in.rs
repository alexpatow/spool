//! Signing in with a password and signing out, making the first account,
//! and changing a password: the paths under `/auth` that Google has no
//! part in.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use subtle::ConstantTimeEq;
use tower_sessions::Session;

use super::accounts::{acquire_hashing_permit, run_accounts_work, run_hashing_work};
use super::caller::{session_failed, signed_in, unauthorized_error};
use crate::api::{ErrorCode, PasswordRequest, Person, SessionState, SetupRequest, SignInRequest};
use crate::auth::{Accounts, Entered, GOOGLE, MARK, MAX_USERNAME, PERSON};
use crate::server::App;
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::json_body;
use crate::server::work::ok;

/// How many wrong passwords one username may have within [`ATTEMPT_WINDOW`]
/// before sign-in for it is refused until the window has passed.
const MAX_ATTEMPTS: u32 = 10;
const ATTEMPT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// The most usernames whose wrong passwords are counted at once. Anyone can
/// try a username, so what is kept of the tries has to have an end.
const MAX_ATTEMPTED: usize = 10_000;

/// The wrong passwords lately tried for each username, and when the first of
/// them was.
#[derive(Clone, Default)]
pub(in crate::server) struct Attempts(Arc<Mutex<HashMap<String, (u32, Instant)>>>);

impl Attempts {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u32, Instant)>> {
        self.0.lock().unwrap_or_else(|held| held.into_inner())
    }

    /// Whether a username has had its tries for now.
    fn is_barred(&self, username: &str) -> bool {
        let mut held = self.lock();
        held.retain(|_, (_, first)| first.elapsed() < ATTEMPT_WINDOW);
        held.get(username)
            .is_some_and(|(wrong, _)| *wrong >= MAX_ATTEMPTS)
    }

    fn record_wrong_password(&self, username: String) {
        let mut held = self.lock();
        if held.len() >= MAX_ATTEMPTED && !held.contains_key(&username) {
            // Counting starts again sooner than it should, which is better
            // than keeping every name anyone ever tried.
            held.clear();
        }
        held.entry(username).or_insert((0, Instant::now())).0 += 1;
    }

    fn forget(&self, username: &str) {
        self.lock().remove(username);
    }
}

/// Starts a session for whoever entered, with an ID nobody has seen before.
/// `with` is what they entered with, and so what the session lasts as long
/// as: [`MARK`] and the mark of their password, or [`GOOGLE`] and the mark
/// of their Google account.
pub(super) async fn begin_session(
    session: &Session,
    person: &str,
    with: (&str, &str),
) -> ApiResult<()> {
    session.cycle_id().await.map_err(session_failed)?;
    for key in [MARK, GOOGLE] {
        session
            .remove::<String>(key)
            .await
            .map_err(session_failed)?;
    }
    for (key, value) in [(PERSON, person), with] {
        session.insert(key, value).await.map_err(session_failed)?;
    }
    Ok(())
}

pub(in crate::server) async fn session_state(
    State(app): State<App>,
    session: Session,
) -> ApiResult {
    let mut person = signed_in(&app, &session).await?;
    if person.is_none() && app.dev {
        person = Some(run_accounts_work(&app, Accounts::dev).await?);
    }
    let setup = person.is_none() && run_accounts_work(&app, Accounts::needs_setup).await?;
    ok(SessionState {
        person,
        setup,
        dev: app.dev,
        google: app.google.is_some(),
    })
}

/// What a call that signs someone in or out answers with.
fn respond_session_state(app: &App, person: Option<Person>) -> ApiResult {
    ok(SessionState {
        person,
        setup: false,
        dev: app.dev,
        google: app.google.is_some(),
    })
}

pub(in crate::server) async fn sign_in(
    State(app): State<App>,
    session: Session,
    body: Bytes,
) -> ApiResult {
    let request: SignInRequest = json_body(&body)?;
    let username = request.username.trim().to_lowercase();
    let wrong = || unauthorized_error("the username or the password is not right");
    if username.len() > MAX_USERNAME {
        return Err(wrong());
    }
    if app.attempts.is_barred(&username) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::TooManyAttempts,
            "too many wrong passwords for this username; wait a while and try again",
        ));
    }
    // Anyone can ask, so nobody waits: a request beyond what is being
    // checked is turned away, and costs nothing.
    let Ok(permit) = Arc::clone(&app.hashing).try_acquire_owned() else {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::TooManyAttempts,
            "too many people are signing in at once; try again in a moment",
        ));
    };
    let attempted = username.clone();
    let entered = run_hashing_work(&app, permit, move |accounts| {
        accounts.verify(&attempted, &request.password)
    })
    .await?;
    let Some(Entered { person, mark }) = entered else {
        app.attempts.record_wrong_password(username);
        return Err(wrong());
    };
    app.attempts.forget(&username);
    begin_session(&session, &person.id, (MARK, &mark)).await?;
    respond_session_state(&app, Some(person))
}

pub(in crate::server) async fn sign_out(State(app): State<App>, session: Session) -> ApiResult {
    session.flush().await.map_err(session_failed)?;
    respond_session_state(&app, None)
}

/// Makes the first account. The code is the one the server printed when it
/// started with no admin who could sign in, and it works once.
pub(in crate::server) async fn set_up(
    State(app): State<App>,
    session: Session,
    body: Bytes,
) -> ApiResult {
    let request: SetupRequest = json_body(&body)?;
    let right = {
        let code = app.setup.lock().unwrap_or_else(|held| held.into_inner());
        code.as_deref()
            .is_some_and(|code| code.as_bytes().ct_eq(request.code.trim().as_bytes()).into())
    };
    if !right {
        return Err(unauthorized_error(
            "that is not the code in the link this server printed when it started",
        ));
    }
    let Entered { person, mark } =
        run_hashing_work(&app, acquire_hashing_permit(&app).await?, move |accounts| {
            accounts.first(&request.username, &request.name, &request.password)
        })
        .await?;
    *app.setup.lock().unwrap_or_else(|held| held.into_inner()) = None;
    begin_session(&session, &person.id, (MARK, &mark)).await?;
    respond_session_state(&app, Some(person))
}

pub(in crate::server) async fn change_password(
    State(app): State<App>,
    session: Session,
    body: Bytes,
) -> ApiResult {
    let request: PasswordRequest = json_body(&body)?;
    let Some(person) = signed_in(&app, &session).await? else {
        return Err(unauthorized_error("sign in first"));
    };
    let id = person.id.clone();
    let mark = run_hashing_work(&app, acquire_hashing_permit(&app).await?, move |accounts| {
        accounts.change_password(&id, &request.current, &request.new)
    })
    .await?;
    // Every session of theirs has ended. This one starts again.
    begin_session(&session, &person.id, (MARK, &mark)).await?;
    respond_session_state(&app, Some(person))
}
