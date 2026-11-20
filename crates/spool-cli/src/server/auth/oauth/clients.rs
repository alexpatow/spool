//! Registering a client: what an assistant sends before it asks a person
//! to approve it, and where an approval of it may be sent.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use super::{OAuthError, issuer, loopback, no_store, now};
use crate::auth::SCOPE;
use crate::server::App;
use crate::server::auth::accounts::people;

/// The most redirect URIs a client registers, and the longest each is.
const MAX_REDIRECTS: usize = 10;
const MAX_URI_BYTES: usize = 2048;

/// The longest name a client gives itself that is kept.
const MAX_CLIENT_NAME: usize = 200;

#[derive(Deserialize)]
struct Registration {
    #[serde(default)]
    redirect_uris: Vec<String>,
    client_name: Option<String>,
}

/// Registers a client (RFC 7591). Anyone may: a client is only a name and
/// where its approvals may be sent, and nothing is approved until a person
/// who can sign in does it. Every client is a public one, which proves
/// itself with PKCE rather than a secret, whatever it asked to be.
pub(in crate::server) async fn register(State(app): State<App>, body: Bytes) -> Response {
    match registered(&app, &body).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn registered(app: &App, body: &Bytes) -> Result<Response, OAuthError> {
    issuer(app)?;
    let asked: Registration = serde_json::from_slice(body).map_err(|error| {
        OAuthError::new(
            "invalid_client_metadata",
            format!("the registration is not JSON this server reads: {error}"),
        )
    })?;
    if asked.redirect_uris.is_empty() || asked.redirect_uris.len() > MAX_REDIRECTS {
        return Err(OAuthError::new(
            "invalid_redirect_uri",
            format!("a client registers 1 to {MAX_REDIRECTS} `redirect_uris`"),
        ));
    }
    for uri in &asked.redirect_uris {
        redirect_of(uri).map_err(|why| OAuthError::new("invalid_redirect_uri", why))?;
    }
    let name = asked
        .client_name
        .map(|name| {
            name.trim()
                .chars()
                .take(MAX_CLIENT_NAME)
                .collect::<String>()
        })
        .filter(|name| !name.is_empty());
    let uris = asked.redirect_uris.clone();
    let client = people(app, move |accounts| {
        accounts.register_client(name.as_deref(), &uris, now())
    })
    .await?;
    let mut answer = json!({
        "client_id": client.id,
        "client_id_issued_at": now(),
        "redirect_uris": client.redirect_uris,
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "scope": SCOPE,
    });
    if let Some(name) = client.name {
        answer["client_name"] = Value::String(name);
    }
    Ok(no_store(
        (StatusCode::CREATED, Json(answer)).into_response(),
    ))
}

/// A redirect URI a client may register: HTTPS, or HTTP to a program on the
/// person's own machine, and no fragment.
fn redirect_of(uri: &str) -> Result<Url, String> {
    if uri.len() > MAX_URI_BYTES {
        return Err(format!("a redirect URI is at most {MAX_URI_BYTES} bytes"));
    }
    let url = Url::parse(uri).map_err(|error| format!("`{uri}` is not a URL: {error}"))?;
    let local = url.host().is_some_and(|host| loopback(&host));
    match url.scheme() {
        _ if url.fragment().is_some() => Err(format!("`{uri}` has a fragment")),
        "https" => Ok(url),
        "http" if local => Ok(url),
        _ => Err(format!(
            "`{uri}` is neither HTTPS nor HTTP to localhost or a loopback address"
        )),
    }
}

/// Whether `asked` is the redirect URI `registered`, or the same one on
/// another port when both are to a program on this machine: such a program
/// listens on whatever port is free (RFC 8252, section 7.3).
pub(super) fn same_redirect(registered: &str, asked: &str) -> bool {
    if registered == asked {
        return true;
    }
    let (Ok(registered), Ok(asked)) = (Url::parse(registered), Url::parse(asked)) else {
        return false;
    };
    let local =
        registered.scheme() == "http" && registered.host().is_some_and(|host| loopback(&host));
    local
        && asked.scheme() == "http"
        && registered.host() == asked.host()
        && registered.path() == asked.path()
        && registered.query() == asked.query()
        && asked.fragment().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redirect_uri_is_https_or_to_this_machine() {
        for good in [
            "https://claude.ai/api/mcp/auth_callback",
            "https://chatgpt.com/connector_platform_oauth_redirect",
            "http://localhost:3118/callback",
            "http://127.0.0.1/callback",
        ] {
            assert!(redirect_of(good).is_ok(), "{good}");
        }
        for bad in [
            "http://claude.ai/callback",
            "https://claude.ai/callback#here",
            "javascript:alert(1)",
            "claude://callback",
            "callback",
        ] {
            assert!(redirect_of(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_loopback_redirect_is_the_same_on_any_port() {
        assert!(same_redirect(
            "http://localhost/callback",
            "http://localhost:3118/callback"
        ));
        assert!(same_redirect(
            "http://127.0.0.1:1/callback",
            "http://127.0.0.1:2/callback"
        ));
        assert!(!same_redirect(
            "http://localhost/callback",
            "http://127.0.0.1:3118/callback"
        ));
        assert!(!same_redirect(
            "http://localhost/callback",
            "http://localhost:3118/other"
        ));
        assert!(!same_redirect(
            "https://claude.ai/api/mcp/auth_callback",
            "https://claude.ai:8443/api/mcp/auth_callback"
        ));
    }
}
