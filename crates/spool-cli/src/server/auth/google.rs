//! Signing in with Google.
//!
//! The browser is sent to Google and comes back with a code. What has to be
//! kept in between is kept by the browser, in a cookie: anyone can start a
//! sign-in, and a server that kept one for each would keep as many as anyone
//! cared to start.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, COOKIE as COOKIES, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use subtle::ConstantTimeEq;
use tower_sessions::Session;
use tower_sessions::cookie::time::Duration as SessionLength;
use tower_sessions::cookie::{Cookie, SameSite};

use super::accounts::run_accounts_work;
use super::oauth;
use super::sign_in::begin_session;
use crate::auth::GOOGLE;
use crate::auth::google::{Failed, Provider, Started};
use crate::server::App;
use crate::server::error::{ApiError, ApiResult, not_found};
use crate::server::request::parse_query;

/// The name of the cookie that keeps a sign-in with Google while the
/// browser is at Google.
const GOOGLE_COOKIE: &str = "spool_google";

/// How long someone has at Google before the sign-in has to start again.
const GOOGLE_MINUTES: i64 = 10;

/// The longest address to go back to after signing in with Google. It
/// travels in a cookie, which has little room. An authorization that does
/// not fit is kept on the server instead, for as long as that cookie lasts.
const MAX_NEXT_BYTES: usize = 1024;

/// The longest authorization address kept while a browser is at Google, and
/// how many are kept. Past these, the sign-in comes back to the first page.
const MAX_HELD_BYTES: usize = 8 * 1024;
const MAX_HELD: usize = 256;

/// The name of the cookie that keeps a sign-in while the browser is at
/// Google. Over HTTPS it is one only this host can have set.
fn google_cookie(app: &App) -> String {
    match app.https {
        true => format!("__Host-{GOOGLE_COOKIE}"),
        false => GOOGLE_COOKIE.to_owned(),
    }
}

/// The header that has the browser keep `value` for a sign-in at Google, or
/// with none, forget it.
fn build_google_cookie_header(app: &App, value: Option<&str>) -> HeaderValue {
    let minutes = value.map_or(0, |_| GOOGLE_MINUTES);
    let cookie = Cookie::build((google_cookie(app), value.unwrap_or("").to_owned()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(app.https)
        .max_age(SessionLength::minutes(minutes))
        .build();
    HeaderValue::from_str(&cookie.to_string()).expect("a cookie of hex and base64 is a header")
}

/// Sends the browser to `to`, and has it keep or forget a sign-in at Google.
fn send_to(app: &App, to: &str, kept: Option<&str>) -> Response {
    let Ok(to) = HeaderValue::from_str(to) else {
        return ApiError::internal("the address to go to is not one").into_response();
    };
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    headers.insert(LOCATION, to);
    headers.insert(SET_COOKIE, build_google_cookie_header(app, kept));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// An address of this server's own pages: one path, with no way out of it.
fn is_own_path(next: &str) -> bool {
    next.starts_with('/')
        && !next[1..].starts_with(['/', '\\'])
        && !next.contains(|c: char| c.is_control() || c == '\\')
}

/// An address of this server's own pages to go back to: `next` if it is
/// one, and the first page if it could lead anywhere else.
fn own_page(next: Option<&str>) -> &str {
    next.filter(|next| next.len() <= MAX_NEXT_BYTES && is_own_path(next))
        .unwrap_or("/")
}

/// Where a browser goes back to after Google. An authorization carries the
/// assistant's whole request, which can be longer than the sign-in cookie,
/// and that one is kept whole up to a limit. Anything else too long to keep
/// in the cookie, or that could lead off this server, is the first page.
fn return_page(next: Option<&str>) -> String {
    match next {
        Some(next)
            if is_authorize_page(next) && is_own_path(next) && next.len() <= MAX_HELD_BYTES =>
        {
            next.to_owned()
        }
        other => own_page(other).to_owned(),
    }
}

/// Whether `next` is the studio page where an assistant is approved. The
/// query is the request, and it is what has to survive the sign-in.
fn is_authorize_page(next: &str) -> bool {
    let path = next
        .split_once(['?', '#'])
        .map(|(path, _)| path)
        .unwrap_or(next);
    path == oauth::AUTHORIZE && next.contains('?')
}

/// Authorization addresses waiting out a sign-in at Google. The cookie
/// carries `held:<id>` and the address stays here, so a long `state` or
/// redirect URI is not cut down to the first page.
#[derive(Clone, Default)]
pub(in crate::server) struct Held(Arc<Mutex<Vec<(String, String, Instant)>>>);

impl Held {
    fn keep(&self, next: &str) -> String {
        let mut held = self.0.lock().unwrap_or_else(|held| held.into_inner());
        let now = Instant::now();
        let for_so_long = Duration::from_secs(GOOGLE_MINUTES as u64 * 60);
        held.retain(|(_, _, at)| now.saturating_duration_since(*at) < for_so_long);
        if held.len() >= MAX_HELD {
            held.remove(0);
        }
        let id = crate::auth::code();
        held.push((id.clone(), next.to_owned(), now));
        format!("held:{id}")
    }

    fn take(&self, next: &str) -> Option<String> {
        let id = next.strip_prefix("held:")?;
        let mut held = self.0.lock().unwrap_or_else(|held| held.into_inner());
        let now = Instant::now();
        let for_so_long = Duration::from_secs(GOOGLE_MINUTES as u64 * 60);
        let index = held.iter().position(|(found, _, at)| {
            found == id && now.saturating_duration_since(*at) < for_so_long
        })?;
        Some(held.remove(index).1)
    }
}

/// Sends the browser back to the page it came from, which says why nobody
/// was signed in: the studio reads `google` from its address.
fn turned_away(app: &App, next: &str, why: &str) -> Response {
    let (page, fragment) = next.split_once('#').unwrap_or((next, ""));
    let joint = if page.contains('?') { '&' } else { '?' };
    let fragment = match fragment {
        "" => String::new(),
        fragment => format!("#{fragment}"),
    };
    send_to(app, &format!("{page}{joint}google={why}{fragment}"), None)
}

/// A sign-in at Google as the browser keeps it: what Google sends back,
/// what its token repeats, what proves who asked, and where to go after.
struct Kept {
    state: String,
    nonce: String,
    verifier: String,
    next: String,
}

impl Kept {
    fn write_value(started: &Started, next: &str) -> String {
        // The first three are base64 without a `.` in it.
        format!(
            "{}.{}.{}.{}",
            started.state,
            started.nonce,
            started.verifier,
            hex::encode(next)
        )
    }

    fn read(app: &App, headers: &HeaderMap) -> Option<Self> {
        let name = google_cookie(app);
        let value = headers
            .get_all(COOKIES)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(Cookie::split_parse)
            .filter_map(Result::ok)
            .find(|cookie| cookie.name() == name)?
            .value()
            .to_owned();
        let mut parts = value.split('.');
        let kept = Kept {
            state: parts.next()?.to_owned(),
            nonce: parts.next()?.to_owned(),
            verifier: parts.next()?.to_owned(),
            next: String::from_utf8(hex::decode(parts.next()?).ok()?).ok()?,
        };
        (parts.next().is_none() && !kept.state.is_empty()).then_some(kept)
    }
}

/// Says on the server's own output why a sign-in with Google failed: the
/// person at the browser is told that it did, and whoever runs the server
/// can do something about it.
fn report_google_failure(failed: &Failed) {
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr(),
        "spool: a sign-in with Google failed: {failed}"
    );
}

async fn ask_google<T: Send + 'static>(
    provider: &Arc<Provider>,
    ask: impl FnOnce(&Provider) -> Result<T, Failed> + Send + 'static,
) -> Result<T, Failed> {
    let provider = Arc::clone(provider);
    tokio::task::spawn_blocking(move || ask(&provider))
        .await
        .unwrap_or_else(|_| {
            Err(Failed::Unreachable(
                "the request to Google stopped".to_owned(),
            ))
        })
}

#[derive(serde::Deserialize)]
pub(in crate::server) struct GoogleStart {
    /// The page to come back to.
    next: Option<String>,
}

/// `GET /auth/google`: sends the browser to Google to sign in.
pub(in crate::server) async fn google_start(
    State(app): State<App>,
    start: Result<Query<GoogleStart>, QueryRejection>,
) -> ApiResult {
    let Some(provider) = &app.google else {
        return Err(not_found().await);
    };
    let start = parse_query(start)?;
    let next = return_page(start.next.as_deref());
    // Sending a browser there asks Google nothing once the server knows
    // where its sign-in is. Until then, it is one of the questions the
    // server has only so many of at once.
    let asking = match provider.knows() {
        true => None,
        false => match Arc::clone(&app.asking).try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return Ok(turned_away(&app, &next, "busy")),
        },
    };
    let start = move |provider: &Provider| {
        let _held = asking;
        provider.start()
    };
    Ok(match ask_google(provider, start).await {
        Ok(started) => {
            // The cookie has room for a short address. A long authorization
            // stays on the server, and the cookie carries only its reference.
            let kept = if next.len() <= MAX_NEXT_BYTES {
                next.clone()
            } else {
                app.held.keep(&next)
            };
            send_to(
                &app,
                &started.url,
                Some(&Kept::write_value(&started, &kept)),
            )
        }
        Err(failed) => {
            report_google_failure(&failed);
            turned_away(&app, &next, "unreachable")
        }
    })
}

#[derive(serde::Deserialize)]
pub(in crate::server) struct GoogleBack {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /auth/google/callback`: where Google sends the browser back to.
/// Signs in whoever Google says it is, if they have an account, and goes to
/// the page the sign-in started from.
pub(in crate::server) async fn google_callback(
    State(app): State<App>,
    session: Session,
    headers: HeaderMap,
    back: Result<Query<GoogleBack>, QueryRejection>,
) -> ApiResult {
    let Some(provider) = &app.google else {
        return Err(not_found().await);
    };
    let back = parse_query(back)?;
    // The browser that comes back is the one that was sent: it kept what
    // Google repeats.
    let kept = Kept::read(&app, &headers).filter(|kept| {
        back.state
            .as_deref()
            .is_some_and(|state| state.as_bytes().ct_eq(kept.state.as_bytes()).into())
    });
    let Some(kept) = kept else {
        return Ok(turned_away(&app, "/", "expired"));
    };
    let next = match app.held.take(&kept.next) {
        Some(address)
            if is_authorize_page(&address)
                && is_own_path(&address)
                && address.len() <= MAX_HELD_BYTES =>
        {
            address
        }
        Some(_) => "/".to_owned(),
        None => own_page(Some(&kept.next)).to_owned(),
    };
    let code = match (back.code, back.error.as_deref()) {
        (Some(code), None) => code,
        (_, Some("access_denied")) => return Ok(turned_away(&app, &next, "cancelled")),
        _ => return Ok(turned_away(&app, &next, "failed")),
    };
    let Ok(permit) = Arc::clone(&app.asking).try_acquire_owned() else {
        return Ok(turned_away(&app, &next, "busy"));
    };
    let Kept {
        nonce, verifier, ..
    } = &kept;
    let (nonce, verifier) = (nonce.clone(), verifier.clone());
    let identity = ask_google(provider, move |provider| {
        let _held = permit;
        provider.finish(&code, &nonce, &verifier)
    })
    .await;
    let identity = match identity {
        Ok(identity) => identity,
        Err(failed) => {
            report_google_failure(&failed);
            let why = match failed {
                Failed::Unverified => "unverified",
                Failed::Unreachable(_) => "unreachable",
                Failed::Refused(_) => "failed",
            };
            return Ok(turned_away(&app, &next, why));
        }
    };
    // Without an address Google answers for, nobody is found by it.
    let why = match identity.email {
        Some(_) => "unknown",
        None => "unvouched",
    };
    let entered = run_accounts_work(&app, move |accounts| {
        accounts.enter_with_google(&identity.subject, identity.email.as_deref())
    })
    .await?;
    let Some((person, bound)) = entered else {
        return Ok(turned_away(&app, &next, why));
    };
    begin_session(&session, &person.id, (GOOGLE, &bound)).await?;
    Ok(send_to(&app, &next, None))
}
