//! The studio: a page that shows what the server holds, in a browser, and
//! takes a reviewer's changes to it.
//!
//! It is a few files, built from `packages/studio` and compiled into the
//! binary: the page, its script, its style sheet and its fonts. They are the
//! same for every server and say nothing about its store, so they need no
//! token. The page reads and writes through `/v1` like any other caller, with
//! the token its reader gives it.

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

/// What the page may do: run its own script, use its own style sheet and
/// fonts, and call the server it came from. Nothing it shows can make it load
/// or send anything else, and no other page can put it in a frame.
const POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
                      font-src 'self'; img-src 'self' data:; connect-src 'self'; \
                      base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

const HTML: &str = "text/html; charset=utf-8";
const SCRIPT: &str = "text/javascript; charset=utf-8";
const STYLE: &str = "text/css; charset=utf-8";
const FONT: &str = "font/woff2";

/// Each file of the studio, with the path it is served at. The page names
/// the others from the root.
const SOURCES: [(&str, &str, &[u8]); 7] = [
    ("/", HTML, include_bytes!("studio/index.html")),
    ("/studio.js", SCRIPT, include_bytes!("studio/studio.js")),
    ("/studio.css", STYLE, include_bytes!("studio/studio.css")),
    (
        "/geist-latin-wght-normal.woff2",
        FONT,
        include_bytes!("studio/geist-latin-wght-normal.woff2"),
    ),
    (
        "/geist-latin-ext-wght-normal.woff2",
        FONT,
        include_bytes!("studio/geist-latin-ext-wght-normal.woff2"),
    ),
    (
        "/geist-mono-latin-wght-normal.woff2",
        FONT,
        include_bytes!("studio/geist-mono-latin-wght-normal.woff2"),
    ),
    (
        "/geist-mono-latin-ext-wght-normal.woff2",
        FONT,
        include_bytes!("studio/geist-mono-latin-ext-wght-normal.woff2"),
    ),
];

struct File {
    media_type: &'static str,
    contents: &'static [u8],
    /// Changes when the contents do, so that a browser asks each time and is
    /// told when what it has is still right.
    tag: HeaderValue,
}

impl File {
    fn new(media_type: &'static str, contents: &'static [u8]) -> Self {
        let hash: String = Sha256::digest(contents)
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

static FILES: LazyLock<Vec<(&'static str, File)>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .map(|(path, media_type, contents)| (*path, File::new(media_type, contents)))
        .collect()
});

/// The screens of the studio. The address of a screen is a path that begins
/// with its name, and the page works out the rest, so each is answered with
/// the page.
const SCREENS: [&str; 8] = [
    "review",
    "drafts",
    "identities",
    "records",
    "documents",
    "explain",
    "runs",
    "log",
];

/// The studio's paths.
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    let files = FILES.iter().fold(Router::new(), |router, (path, file)| {
        router.route(
            path,
            get(move |headers: HeaderMap| async move { file.respond(&headers) }),
        )
    });
    let page = &FILES[0].1;
    SCREENS.iter().fold(files, |router, screen| {
        let shown = get(move |headers: HeaderMap| async move { page.respond(&headers) });
        router
            .route(&format!("/{screen}"), shown.clone())
            .route(&format!("/{screen}/{{*rest}}"), shown)
    })
}
