//! The server as the OAuth authorization server of its own `/mcp`.
//!
//! Claude and ChatGPT on the web connect to an MCP server that asks who
//! they are only through OAuth 2.1, as the Model Context Protocol's
//! authorization specification says: `/mcp` answers `401` with where its
//! metadata is, the client registers itself (RFC 7591), sends the person to
//! `/oauth/authorize` with a PKCE challenge, and exchanges the code it gets
//! back at `/oauth/token`. The person signs in to the studio as always, and
//! approves the client there.
//!
//! The address this server is reached by is the issuer, and `/mcp` under
//! it the one resource its tokens are for: an access token works at `/mcp`
//! and is refused everywhere else. A server that does not know its address
//! has no issuer, and none of this.
//!
//! Each response to the client says `iss` (RFC 9207), so that ChatGPT uses
//! the one redirect URI it has for every server that does.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, LOCATION, PRAGMA};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use serde::Serialize;
use serde_json::json;
use tower_sessions::Session;
use url::{Host, Url};

use super::accounts::run_accounts_work;
use super::caller::{Caller, session_failed, signed_in, unauthorized_error};
use crate::api::{GrantList, OAuthDecided, OAuthDecision, OAuthRequest};
use crate::auth::{
    ACCESS_SECONDS, Accounts, Approval, Client, GOOGLE, Issued, MARK, Refused, SCOPE,
};
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::json_body;
use crate::server::work::ok;
use crate::server::{App, MCP_PATH};
use crate::studio;

mod clients;

pub(in crate::server) use clients::register;
use clients::same_redirect;

/// Where the metadata of `/mcp` as a protected resource is (RFC 9728). The
/// path with the resource's own after it is the one clients try first.
pub(in crate::server) const RESOURCE_METADATA: &str = "/.well-known/oauth-protected-resource";

/// Where the metadata of the authorization server is (RFC 8414).
pub(in crate::server) const SERVER_METADATA: &str = "/.well-known/oauth-authorization-server";

pub(in crate::server) const AUTHORIZE: &str = "/oauth/authorize";
pub(in crate::server) const TOKEN: &str = "/oauth/token";
pub(in crate::server) const REGISTER: &str = "/oauth/register";
pub(in crate::server) const REVOKE: &str = "/oauth/revoke";

/// The address a server is reached by, as the issuer of its tokens: HTTPS,
/// or HTTP on this machine. `None` with why, for one that cannot be.
pub fn issuer_of(address: &str) -> Result<String, String> {
    let address = address.trim_end_matches('/');
    let url = Url::parse(address).map_err(|error| format!("`{address}` is not a URL: {error}"))?;
    let local = url.host().is_some_and(|host| is_loopback(&host));
    match url.scheme() {
        "https" => Ok(address.to_owned()),
        "http" if local => Ok(address.to_owned()),
        _ => Err(format!(
            "`{address}` is not HTTPS, which an assistant needs to sign in with OAuth"
        )),
    }
}

fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    }
}

/// The issuer, or `404` on a server that has none.
fn require_issuer(app: &App) -> ApiResult<&str> {
    app.issuer
        .as_deref()
        .ok_or_else(|| ApiError::not_found("there is nothing at this path"))
}

fn format_resource_url(issuer: &str) -> String {
    format!("{issuer}{MCP_PATH}")
}

/// What `WWW-Authenticate` says when `/mcp` is refused: where to read how
/// to sign in, and whether the token that was sent is no good.
pub(super) fn build_bearer_challenge(issuer: &str, sent: bool) -> HeaderValue {
    let invalid = match sent {
        true => ", error=\"invalid_token\"",
        false => "",
    };
    HeaderValue::from_str(&format!(
        "Bearer resource_metadata=\"{issuer}{RESOURCE_METADATA}{MCP_PATH}\", scope=\"{SCOPE}\"{invalid}"
    ))
    .expect("an address is a header value")
}

fn current_unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// The caller an access token acts as, at `/mcp`.
pub(super) async fn find_access_token_caller(app: &App, token: &str) -> ApiResult<Caller> {
    let token = token.to_owned();
    match run_accounts_work(app, move |accounts| {
        accounts.bearer(&token, current_unix_time())
    })
    .await?
    {
        Some(person) => Ok(Caller::of(person)),
        None => Err(unauthorized_error(
            "that access token has expired or was revoked: the assistant refreshes it or \
             signs in again",
        )),
    }
}

// Metadata

pub(in crate::server) async fn resource_metadata(State(app): State<App>) -> ApiResult {
    let issuer = require_issuer(&app)?;
    ok(json!({
        "resource": format_resource_url(issuer),
        "authorization_servers": [issuer],
        "scopes_supported": [SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "Spool",
    }))
}

pub(in crate::server) async fn server_metadata(State(app): State<App>) -> ApiResult {
    let issuer = require_issuer(&app)?;
    ok(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}{AUTHORIZE}"),
        "token_endpoint": format!("{issuer}{TOKEN}"),
        "registration_endpoint": format!("{issuer}{REGISTER}"),
        "revocation_endpoint": format!("{issuer}{REVOKE}"),
        "scopes_supported": [SCOPE],
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
        "authorization_response_iss_parameter_supported": true,
    }))
}

// Errors, as OAuth gives them

/// An error of the registration or the token endpoint: a JSON body with
/// `error` and `error_description`, which a client reads and Spool's own
/// error body would not be.
struct OAuthError {
    status: StatusCode,
    error: &'static str,
    description: String,
}

impl OAuthError {
    fn new(error: &'static str, description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error,
            description: description.into(),
        }
    }

    fn invalid_client(description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            ..Self::new("invalid_client", description)
        }
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let body = json!({ "error": self.error, "error_description": self.description });
        no_store((self.status, Json(body)).into_response())
    }
}

impl From<ApiError> for OAuthError {
    fn from(error: ApiError) -> Self {
        Self {
            status: error.status,
            error: match error.status {
                StatusCode::TOO_MANY_REQUESTS => "temporarily_unavailable",
                _ => "server_error",
            },
            description: error.body.message,
        }
    }
}

fn no_store(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// The fields of a form or a query, each once. OAuth refuses a request that
/// gives one twice.
fn parse_form_fields(encoded: &str) -> Result<HashMap<String, String>, String> {
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(encoded)
        .map_err(|error| format!("the request could not be read: {error}"))?;
    let mut fields = HashMap::new();
    for (name, value) in pairs {
        if fields.insert(name.clone(), value).is_some() {
            return Err(format!("`{name}` is given more than once"));
        }
    }
    Ok(fields)
}

// Approval

/// What a client asks for at `/oauth/authorize`, once it has been checked.
struct Asked {
    client: Client,
    redirect_uri: String,
    state: Option<String>,
    challenge: String,
}

/// Why a request to `/oauth/authorize` gives nothing.
enum Wrong {
    /// The client or where to send it is not known, so it is told to the
    /// person and to nobody else.
    Shown(String),
    /// Sent back to the client, as RFC 6749 says.
    Sent {
        redirect_uri: String,
        state: Option<String>,
        error: &'static str,
        description: String,
    },
}

async fn check_authorize_request(app: &App, query: &str) -> ApiResult<Result<Asked, Wrong>> {
    let issuer = require_issuer(app)?.to_owned();
    let fields = match parse_form_fields(query) {
        Ok(fields) => fields,
        Err(why) => return Ok(Err(Wrong::Shown(why))),
    };
    let field = |name: &str| {
        fields
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    };
    let Some(client_id) = field("client_id").map(str::to_owned) else {
        return Ok(Err(Wrong::Shown(
            "the address the assistant sent has no `client_id`".to_owned(),
        )));
    };
    let Some(client) = run_accounts_work(app, move |accounts| accounts.client(&client_id)).await?
    else {
        return Ok(Err(Wrong::Shown(
            "the assistant is not registered with this server: remove it and add it again"
                .to_owned(),
        )));
    };
    let redirect_uri = match (field("redirect_uri"), client.redirect_uris.as_slice()) {
        (Some(asked), registered) => match registered.iter().any(|uri| same_redirect(uri, asked)) {
            true => asked.to_owned(),
            false => {
                return Ok(Err(Wrong::Shown(format!(
                    "the assistant asked for the approval to go to {asked}, which it did \
                         not register"
                ))));
            }
        },
        (None, [only]) => only.clone(),
        (None, _) => {
            return Ok(Err(Wrong::Shown(
                "the address the assistant sent has no `redirect_uri`".to_owned(),
            )));
        }
    };
    let state = field("state").map(str::to_owned);
    let sent = |error, description: &str| {
        Ok(Err(Wrong::Sent {
            redirect_uri: redirect_uri.clone(),
            state: state.clone(),
            error,
            description: description.to_owned(),
        }))
    };
    if field("response_type") != Some("code") {
        return sent(
            "unsupported_response_type",
            "this server gives codes only: `response_type=code`",
        );
    }
    let challenge = field("code_challenge").unwrap_or_default();
    let base64url = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    if challenge.len() != 43 || !challenge.chars().all(base64url) {
        return sent(
            "invalid_request",
            "PKCE is required: send `code_challenge`, the S256 of the verifier",
        );
    }
    if field("code_challenge_method") != Some("S256") {
        return sent(
            "invalid_request",
            "PKCE is with `code_challenge_method=S256` only",
        );
    }
    if let Some(asked) = field("resource")
        && !is_own_resource(&issuer, asked)
    {
        return sent(
            "invalid_target",
            &format!(
                "this server gives tokens for {} only",
                format_resource_url(&issuer)
            ),
        );
    }
    Ok(Ok(Asked {
        client,
        redirect_uri: redirect_uri.clone(),
        state: state.clone(),
        challenge: challenge.to_owned(),
    }))
}

/// Whether a `resource` a client names is `/mcp` of this server: as the
/// metadata says it, without the path, or with a slash after either.
fn is_own_resource(issuer: &str, asked: &str) -> bool {
    let asked = asked.trim_end_matches('/');
    let lowered = |text: &str| match text.split_once("://") {
        Some((scheme, rest)) => {
            let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            format!("{}://{}{path}", scheme.to_lowercase(), host.to_lowercase())
        }
        None => text.to_owned(),
    };
    let asked = lowered(asked);
    asked == lowered(issuer) || asked == lowered(&format_resource_url(issuer))
}

/// The address to send the browser to with the answer: the client's, with
/// what it is told, `state` as it sent it, and who answers.
fn build_answer_url(
    issuer: &str,
    redirect_uri: &str,
    state: Option<&str>,
    said: &[(&str, &str)],
) -> String {
    let mut url = Url::parse(redirect_uri).expect("a redirect URI was checked when registered");
    {
        let mut query = url.query_pairs_mut();
        for (name, value) in said {
            query.append_pair(name, value);
        }
        if let Some(state) = state {
            query.append_pair("state", state);
        }
        query.append_pair("iss", issuer);
    }
    url.into()
}

fn sent_back(issuer: &str, wrong: &Wrong) -> Option<String> {
    match wrong {
        Wrong::Shown(_) => None,
        Wrong::Sent {
            redirect_uri,
            state,
            error,
            description,
        } => Some(build_answer_url(
            issuer,
            redirect_uri,
            state.as_deref(),
            &[("error", error), ("error_description", description)],
        )),
    }
}

/// Where a client sends the person. A request that can be answered to the
/// client is checked here and sent back if it is wrong; otherwise the
/// studio shows it, to sign in if need be and to approve or turn it away.
pub(in crate::server) async fn authorize(
    State(app): State<App>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> ApiResult {
    let issuer = require_issuer(&app)?.to_owned();
    if let Err(wrong) = check_authorize_request(&app, query.as_deref().unwrap_or_default()).await?
        && let Some(to) = sent_back(&issuer, &wrong)
    {
        return Ok(see_other(&to));
    }
    let mut page = studio::page(&headers);
    page.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(page)
}

fn see_other(to: &str) -> Response {
    let mut response = no_store(StatusCode::SEE_OTHER.into_response());
    if let Ok(to) = HeaderValue::from_str(to) {
        response.headers_mut().insert(LOCATION, to);
    }
    response
}

/// What the studio shows before someone approves a client.
pub(in crate::server) async fn describe(
    State(app): State<App>,
    RawQuery(query): RawQuery,
) -> ApiResult {
    match check_authorize_request(&app, query.as_deref().unwrap_or_default()).await? {
        Ok(asked) => {
            let url = Url::parse(&asked.redirect_uri).expect("checked when registered");
            ok(OAuthRequest {
                client_name: asked.client.name,
                host: url.host_str().unwrap_or_default().to_owned(),
                loopback: url.host().is_some_and(|host| is_loopback(&host)),
                redirect_uri: asked.redirect_uri,
            })
        }
        Err(Wrong::Shown(why)) => Err(ApiError::invalid(why)),
        Err(Wrong::Sent { description, .. }) => Err(ApiError::invalid(description)),
    }
}

/// The person signed in approves a client, or turns it away, and the
/// studio sends the browser where this says.
pub(in crate::server) async fn decide(
    State(app): State<App>,
    session: Session,
    body: Bytes,
) -> ApiResult {
    let decision: OAuthDecision = json_body(&body)?;
    let issuer = require_issuer(&app)?.to_owned();
    let Some(person) = signed_in(&app, &session).await? else {
        return Err(unauthorized_error("sign in first"));
    };
    let began_with = began_with(&session)
        .await?
        .ok_or_else(|| unauthorized_error("sign in first"))?;
    let asked = match check_authorize_request(&app, &decision.request).await? {
        Ok(asked) => asked,
        Err(Wrong::Shown(why)) => return Err(ApiError::invalid(why)),
        Err(wrong) => {
            let to = sent_back(&issuer, &wrong).expect("one that is sent has somewhere to go");
            return ok(OAuthDecided { to });
        }
    };
    let state = asked.state.as_deref();
    if !decision.approve {
        let said = [
            ("error", "access_denied"),
            ("error_description", "the person turned the assistant away"),
        ];
        return ok(OAuthDecided {
            to: build_answer_url(&issuer, &asked.redirect_uri, state, &said),
        });
    }
    let (key, mark) = began_with;
    let client = asked.client.id.clone();
    let redirect_uri = asked.redirect_uri.clone();
    let challenge = asked.challenge.clone();
    let code = run_accounts_work(&app, move |accounts| {
        let approval = Approval {
            client: &client,
            person: &person.id,
            began_with: (key, &mark),
            redirect_uri: &redirect_uri,
            challenge: &challenge,
        };
        accounts.approve(&approval, current_unix_time())
    })
    .await?;
    ok(OAuthDecided {
        to: build_answer_url(&issuer, &asked.redirect_uri, state, &[("code", &code)]),
    })
}

/// What the session began with, which an approval given in it lasts as
/// long as.
async fn began_with(session: &Session) -> ApiResult<Option<(&'static str, String)>> {
    for key in [MARK, GOOGLE] {
        if let Some(mark) = session.get::<String>(key).await.map_err(session_failed)? {
            return Ok(Some((key, mark)));
        }
    }
    Ok(None)
}

// Tokens

#[derive(Serialize)]
struct Tokens {
    access_token: String,
    token_type: &'static str,
    expires_in: i64,
    refresh_token: String,
    scope: &'static str,
}

impl From<Issued> for Tokens {
    fn from(issued: Issued) -> Self {
        Self {
            access_token: issued.access,
            token_type: "Bearer",
            expires_in: ACCESS_SECONDS,
            refresh_token: issued.refresh,
            scope: SCOPE,
        }
    }
}

/// Exchanges a code, or a refresh token, for tokens.
pub(in crate::server) async fn token(
    State(app): State<App>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match exchange_for_tokens(&app, &headers, &body).await {
        Ok(tokens) => no_store(Json(tokens).into_response()),
        Err(error) => error.into_response(),
    }
}

async fn exchange_for_tokens(
    app: &App,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<Tokens, OAuthError> {
    let issuer = require_issuer(app)?.to_owned();
    let form = std::str::from_utf8(body)
        .map_err(|_| OAuthError::new("invalid_request", "the form is not UTF-8"))
        .and_then(|form| {
            parse_form_fields(form).map_err(|why| OAuthError::new("invalid_request", why))
        })?;
    let field = |name: &str| {
        form.get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    };
    let needed = |name: &str| {
        field(name)
            .ok_or_else(|| OAuthError::new("invalid_request", format!("`{name}` is missing")))
    };
    let client = client_of(field("client_id"), headers)?;
    let known = {
        let client = client.clone();
        run_accounts_work(app, move |accounts| accounts.client(&client)).await?
    };
    if known.is_none() {
        return Err(OAuthError::invalid_client(
            "the client is not registered with this server: register again",
        ));
    }
    if let Some(asked) = field("resource")
        && !is_own_resource(&issuer, asked)
    {
        return Err(OAuthError::new(
            "invalid_target",
            format!(
                "this server gives tokens for {} only",
                format_resource_url(&issuer)
            ),
        ));
    }
    let given = match needed("grant_type")? {
        "authorization_code" => {
            let code = needed("code")?.to_owned();
            let verifier = needed("code_verifier")?.to_owned();
            let redirect_uri = field("redirect_uri").map(str::to_owned);
            run_accounts_work(app, move |accounts| {
                accounts.exchange(
                    &code,
                    &client,
                    redirect_uri.as_deref(),
                    &verifier,
                    current_unix_time(),
                )
            })
            .await?
        }
        "refresh_token" => {
            let refresh = needed("refresh_token")?.to_owned();
            run_accounts_work(app, move |accounts| {
                accounts.refresh(&refresh, &client, current_unix_time())
            })
            .await?
        }
        other => {
            return Err(OAuthError::new(
                "unsupported_grant_type",
                format!(
                    "this server takes `authorization_code` and `refresh_token`, not `{other}`"
                ),
            ));
        }
    };
    given
        .map(Tokens::from)
        .map_err(|why| OAuthError::new("invalid_grant", why))
}

/// The client a request to the token endpoint is from: its `client_id`, or
/// the user of HTTP Basic, which a client may send with an empty password.
fn client_of(field: Option<&str>, headers: &HeaderMap) -> Result<String, OAuthError> {
    if let Some(client) = field {
        return Ok(client.to_owned());
    }
    let basic = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("basic"))
        .and_then(|(_, encoded)| {
            // Padding is taken off, so a client that leaves it out is read too.
            let encoded = encoded.trim().trim_end_matches('=');
            String::from_utf8(STANDARD_NO_PAD.decode(encoded).ok()?).ok()
        })
        .and_then(|decoded| decoded.split_once(':').map(|(user, _)| user.to_owned()))
        .and_then(|user| {
            serde_urlencoded::from_str::<Vec<(String, String)>>(&format!("u={user}")).ok()
        })
        .and_then(|mut pairs| pairs.pop().map(|(_, user)| user))
        .filter(|user| !user.is_empty());
    basic.ok_or_else(|| OAuthError::invalid_client("`client_id` is missing"))
}

/// Takes back a token a client was given (RFC 7009). The answer is the same
/// whether or not there was such a token.
pub(in crate::server) async fn revoke(
    State(app): State<App>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let revoked = async {
        require_issuer(&app)?;
        let form = std::str::from_utf8(&body)
            .map_err(|_| OAuthError::new("invalid_request", "the form is not UTF-8"))
            .and_then(|form| {
                parse_form_fields(form).map_err(|why| OAuthError::new("invalid_request", why))
            })?;
        let field = |name: &str| {
            form.get(name)
                .map(String::as_str)
                .filter(|value| !value.is_empty())
        };
        let client = client_of(field("client_id"), &headers)?;
        let token = field("token")
            .ok_or_else(|| OAuthError::new("invalid_request", "`token` is missing"))?
            .to_owned();
        run_accounts_work(&app, move |accounts| {
            accounts.revoke_issued(&token, &client)
        })
        .await?;
        Ok::<_, OAuthError>(())
    };
    match revoked.await {
        Ok(()) => no_store(StatusCode::OK.into_response()),
        Err(error) => error.into_response(),
    }
}

// What an admin sees

pub(in crate::server) async fn list_grants(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
) -> ApiResult {
    caller.admin()?;
    let grants = run_accounts_work(&app, Accounts::grants).await?;
    ok(GrantList { grants })
}

pub(in crate::server) async fn end_grant(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(grant): Path<String>,
) -> ApiResult {
    caller.admin()?;
    ok(run_accounts_work(&app, move |accounts| accounts.end_grant(&grant)).await?)
}

impl From<Refused> for OAuthError {
    fn from(refused: Refused) -> Self {
        ApiError::from(refused).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issuer_is_https_or_on_this_machine() {
        assert!(issuer_of("https://spool.example.com/").is_ok());
        assert_eq!(
            issuer_of("https://spool.example.com/").unwrap(),
            "https://spool.example.com"
        );
        for local in [
            "http://127.0.0.1:7765",
            "http://localhost:7765",
            "http://[::1]:7765",
        ] {
            assert!(issuer_of(local).is_ok(), "{local}");
        }
        assert!(issuer_of("http://spool.example.com").is_err());
        assert!(issuer_of("http://0.0.0.0:7765").is_err());
    }

    #[test]
    fn the_resource_is_mcp_however_it_is_written() {
        let issuer = "https://spool.example.com";
        for good in [
            "https://spool.example.com/mcp",
            "https://spool.example.com/mcp/",
            "HTTPS://Spool.Example.com/mcp",
            "https://spool.example.com",
        ] {
            assert!(is_own_resource(issuer, good), "{good}");
        }
        for bad in [
            "https://spool.example.com/v1",
            "https://other.example.com/mcp",
            "https://spool.example.com/MCP",
        ] {
            assert!(!is_own_resource(issuer, bad), "{bad}");
        }
    }

    #[test]
    fn the_answer_says_who_answers() {
        let to = build_answer_url(
            "https://spool.example.com",
            "https://claude.ai/api/mcp/auth_callback?keep=1",
            Some("a b"),
            &[("code", "c")],
        );
        assert_eq!(
            to,
            "https://claude.ai/api/mcp/auth_callback?keep=1&code=c&state=a+b&iss=https%3A%2F%2Fspool.example.com"
        );
    }

    #[test]
    fn basic_names_the_client() {
        let mut headers = HeaderMap::new();
        // `client-1:` in base64.
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Basic Y2xpZW50LTE6"),
        );
        assert_eq!(client_of(None, &headers).ok().as_deref(), Some("client-1"));
        assert_eq!(
            client_of(Some("given"), &headers).ok().as_deref(),
            Some("given")
        );
        assert!(client_of(None, &HeaderMap::new()).is_err());
    }
}
