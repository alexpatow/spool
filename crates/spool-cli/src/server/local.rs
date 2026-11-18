//! What keeps a development server to the machine it runs on.
//!
//! `spool dev` listens on a loopback address and asks nobody to sign in. A
//! page of another site can still reach it: the site gives a name of its
//! own this machine's address, and the browser then sends the page's
//! requests here as if to the page's own site, so nothing about them says
//! they come from elsewhere. What does give them away is the name they were
//! sent to. A development server answers only to the names of this machine.

use std::net::IpAddr;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::http::header::HOST;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::ApiError;
use crate::api::ErrorCode;

/// Refuses a request that was sent to a name other than this machine's.
pub(super) async fn this_machine_only(request: Request, next: Next) -> Response {
    let sent_to = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str())
        });
    if sent_to.is_some_and(this_machine) {
        return next.run(request).await;
    }
    ApiError::new(
        StatusCode::FORBIDDEN,
        ErrorCode::Forbidden,
        "a development server answers only on this machine: reach it as `localhost` or by a \
         loopback address such as `127.0.0.1`",
    )
    .into_response()
}

/// Whether a `Host` names this machine: `localhost`, or a loopback address.
/// An address cannot be made to mean another machine, and no other name is
/// known to mean this one.
fn this_machine(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        // An IPv6 address is in brackets, with the port after them.
        Some(rest) => match rest.split_once(']') {
            Some((address, "")) => address,
            Some((address, port)) if port.starts_with(':') => address,
            _ => return false,
        },
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    name.eq_ignore_ascii_case("localhost")
        || name
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::this_machine;

    #[test]
    fn only_the_names_of_this_machine_are_its_own() {
        for host in [
            "localhost",
            "LOCALHOST:7765",
            "127.0.0.1",
            "127.0.0.1:7765",
            "127.8.9.10:80",
            "[::1]",
            "[::1]:7765",
        ] {
            assert!(this_machine(host), "{host}");
        }
        for host in [
            "",
            "elsewhere.example",
            "elsewhere.example:7765",
            "localhost.elsewhere.example",
            "127.0.0.1.elsewhere.example:7765",
            "notlocalhost",
            "192.168.1.5:7765",
            "0.0.0.0:7765",
            "[::1",
            "[::1]elsewhere.example",
            "[2001:db8::1]:7765",
            "::1",
        ] {
            assert!(!this_machine(host), "{host}");
        }
    }
}
