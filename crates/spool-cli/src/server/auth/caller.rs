//! Who is calling: the person a session belongs to, the agent a token is,
//! or at `/mcp` the person who approved an assistant.

use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, HOST, ORIGIN, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use spool::core::Actor;
use tower_sessions::Session;

use super::accounts::people;
use super::oauth;
use crate::api::{ErrorCode, Person};
use crate::auth::{ACCESS_PREFIX, Accounts, GOOGLE, MARK, PERSON};
use crate::server::App;
use crate::server::error::{ApiError, ApiResult};

/// Who is calling, and so who a change they make is recorded under.
#[derive(Clone, Debug)]
pub(in crate::server) struct Caller {
    pub(in crate::server) actor: Actor,
    /// The account, when the caller is a person.
    pub(in crate::server) person: Option<Person>,
}

impl Caller {
    pub(super) fn of(person: Person) -> Self {
        Self {
            actor: Actor::User(person.id.clone()),
            person: Some(person),
        }
    }

    /// The caller's own account.
    fn person(&self) -> ApiResult<&Person> {
        self.person.as_ref().ok_or_else(|| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                ErrorCode::Forbidden,
                "this is for a person who has signed in, not for a token",
            )
        })
    }

    pub(super) fn admin(&self) -> ApiResult<&Person> {
        self.person()
            .ok()
            .filter(|person| person.admin)
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::FORBIDDEN,
                    ErrorCode::Forbidden,
                    "only an admin manages people and tokens",
                )
            })
    }
}

/// Refuses a write that a page of another site sent. A browser says where a
/// write comes from, and may send this server's cookie with it wherever that
/// is; a caller that is not a browser says nothing and is let through.
pub(in crate::server) async fn own_pages_only(request: Request, next: Next) -> Response {
    if !elsewhere(request.method(), request.headers()) {
        return next.run(request).await;
    }
    ApiError::new(
        StatusCode::FORBIDDEN,
        ErrorCode::Forbidden,
        "a change must come from this server's own pages",
    )
    .into_response()
}

/// Whether a request is a write that says it comes from another site.
///
/// A browser of the last few years says so itself, in `Sec-Fetch-Site`, and
/// that holds whatever stands between it and this server. An older one names
/// the page's origin, which is compared with the host the request was sent
/// to.
fn elsewhere(method: &Method, headers: &HeaderMap) -> bool {
    if [Method::GET, Method::HEAD, Method::OPTIONS].contains(method) {
        return false;
    }
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    if let Some(site) = header("sec-fetch-site") {
        return !["same-origin", "none"].contains(&site);
    }
    let Some(origin) = header(ORIGIN.as_str()) else {
        return false;
    };
    let from = origin.split_once("://").map_or(origin, |(_, host)| host);
    header(HOST.as_str()) != Some(from)
}

pub(in crate::server) async fn authorize(
    State(app): State<App>,
    session: Session,
    mut request: Request,
    next: Next,
) -> Response {
    match caller(&app, &session, request.headers(), false).await {
        Ok(caller) => {
            request.extensions_mut().insert(caller);
            next.run(request).await
        }
        Err(error) => error.into_response(),
    }
}

/// As [`authorize`], for `/mcp`, which also takes the access tokens of
/// assistants that signed in with OAuth. A refusal says where to read how
/// to sign in, which is how such an assistant finds out.
pub(in crate::server) async fn authorize_mcp(
    State(app): State<App>,
    session: Session,
    mut request: Request,
    next: Next,
) -> Response {
    match caller(&app, &session, request.headers(), true).await {
        Ok(caller) => {
            request.extensions_mut().insert(caller);
            next.run(request).await
        }
        Err(error) => {
            let mut response = error.into_response();
            if let Some(issuer) = &app.issuer
                && response.status() == StatusCode::UNAUTHORIZED
            {
                let sent = request.headers().contains_key(AUTHORIZATION);
                response
                    .headers_mut()
                    .insert(WWW_AUTHENTICATE, oauth::challenge(issuer, sent));
            }
            response
        }
    }
}

pub(super) fn unauthorized(message: &str) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, message)
}

/// The caller a request is from: the agent whose token it carries, the
/// person its session belongs to, or, at `/mcp`, the person who approved
/// the assistant whose access token it carries.
async fn caller(
    app: &App,
    session: &Session,
    headers: &HeaderMap,
    at_mcp: bool,
) -> ApiResult<Caller> {
    let presented = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, token) = value.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
        });
    if let Some(presented) = presented.filter(|token| token.starts_with(ACCESS_PREFIX)) {
        return match (at_mcp, app.issuer.is_some()) {
            (true, true) => oauth::bearer(app, presented).await,
            (false, true) => Err(unauthorized(
                "an assistant's access token works at /mcp and nowhere else",
            )),
            (_, false) => Err(unauthorized(
                "this server does not sign assistants in with OAuth: it needs the address it \
                 is reached by, with `--public-url`",
            )),
        };
    }
    if let Some(presented) = presented {
        let presented = presented.to_owned();
        return match people(app, move |accounts| accounts.agent(&presented)).await? {
            Some(name) => Ok(Caller {
                actor: Actor::Agent(name),
                person: None,
            }),
            None => Err(unauthorized(
                "that is not one of this server's tokens: an admin makes one on the studio's \
                 Tokens page, and `spool token add` makes one on the server's machine",
            )),
        };
    }
    if let Some(person) = signed_in(app, session).await? {
        return Ok(Caller::of(person));
    }
    if app.dev {
        return Ok(Caller::of(people(app, Accounts::dev).await?));
    }
    Err(unauthorized(
        "sign in, or send a token: `Authorization: Bearer <token>`",
    ))
}

/// The person a session belongs to, if their account is still on and has
/// what the session began with: the password, or the Google account.
pub(super) async fn signed_in(app: &App, session: &Session) -> ApiResult<Option<Person>> {
    let kept = |key| async move { session.get::<String>(key).await.map_err(session_failed) };
    let Some(id) = kept(PERSON).await? else {
        return Ok(None);
    };
    if let Some(mark) = kept(MARK).await? {
        return people(app, move |accounts| accounts.holder(&id, &mark)).await;
    }
    match kept(GOOGLE).await? {
        // A server that no longer signs people in with Google has nobody
        // signed in with it.
        Some(bound) if app.google.is_some() => {
            people(app, move |accounts| {
                accounts.holder_with_google(&id, &bound)
            })
            .await
        }
        _ => Ok(None),
    }
}

pub(super) fn session_failed(error: tower_sessions::session::Error) -> ApiError {
    ApiError::internal(format!("the session could not be read: {error}"))
}
