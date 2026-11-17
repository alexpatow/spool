//! The studio: a page that shows what the server holds, in a browser, and
//! takes a reviewer's changes to it.
//!
//! It is three files, built from `packages/studio` and compiled into the
//! binary: the page, its script and its style sheet. They are the same for
//! every server and say nothing about its store, so they need no token. The
//! page reads and writes through `/v1` like any other caller, with the token
//! its reader gives it.

use std::sync::LazyLock;

use axum::Router;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, IF_NONE_MATCH, REFERRER_POLICY,
    X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sha2::{Digest, Sha256};

/// What the page may do: run its own script, use its own style sheet, and
/// call the server it came from. Nothing it shows can make it load or send
/// anything else, and no other page can put it in a frame.
const POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
                      img-src 'self' data:; connect-src 'self'; base-uri 'none'; \
                      form-action 'none'; frame-ancestors 'none'";

struct File {
    media_type: &'static str,
    contents: &'static str,
    /// Changes when the contents do, so that a browser asks each time and is
    /// told when what it has is still right.
    tag: HeaderValue,
}

impl File {
    fn new(media_type: &'static str, contents: &'static str) -> Self {
        let hash: String = Sha256::digest(contents.as_bytes())
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        File {
            media_type,
            contents,
            tag: HeaderValue::from_str(&format!("\"{hash}\"")).expect("hex is a header value"),
        }
    }

    fn respond(&self, headers: &HeaderMap) -> Response {
        let unchanged = headers
            .get(IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.split(',').map(str::trim).any(|tag| {
                    // A weak tag is as good as a strong one for a read.
                    tag == "*" || tag.trim_start_matches("W/") == self.tag
                })
            });
        let mut response = match unchanged {
            true => StatusCode::NOT_MODIFIED.into_response(),
            false => self.contents.into_response(),
        };
        let headers = response.headers_mut();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(self.media_type));
        headers.insert(ETAG, self.tag.clone());
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(POLICY));
        headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        response
    }
}

static PAGE: LazyLock<File> = LazyLock::new(|| {
    File::new(
        "text/html; charset=utf-8",
        include_str!("studio/index.html"),
    )
});
static SCRIPT: LazyLock<File> = LazyLock::new(|| {
    File::new(
        "text/javascript; charset=utf-8",
        include_str!("studio/studio.js"),
    )
});
static STYLE: LazyLock<File> =
    LazyLock::new(|| File::new("text/css; charset=utf-8", include_str!("studio/studio.css")));

/// The studio's three paths. The page names the other two relative to itself.
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(
            "/",
            get(|headers: HeaderMap| async move { PAGE.respond(&headers) }),
        )
        .route(
            "/studio.js",
            get(|headers: HeaderMap| async move { SCRIPT.respond(&headers) }),
        )
        .route(
            "/studio.css",
            get(|headers: HeaderMap| async move { STYLE.respond(&headers) }),
        )
}
