//! The HTTP API: a thin wrapper around one [`Engine`].
//!
//! The server is the only process that has the store open. Every path under
//! `/v1` needs a caller the server knows: a person who has signed in, whose
//! browser carries a session cookie, or an agent that presents one of the
//! server's tokens. The paths under `/auth` sign a person in and out, with a
//! password or, where the server is a client of Google's, with Google. The
//! studio at `/` needs neither: it is a page that holds nothing, and reads
//! through `/v1` as the person signed in. Small values are JSON; candidates, judgements and rows
//! to load can also cross as Arrow IPC streams.
//!
//! `/mcp` is the same API as tools for an assistant, over the Model Context
//! Protocol. It needs the same caller as `/v1` and runs the same handlers.
//! It also takes an assistant that signed in with OAuth, as the person who
//! approved it: the server is the OAuth server for `/mcp` alone
//! (`server/auth/oauth.rs`).
//!
//! A change is recorded under whoever made it. The server says who that is;
//! a request cannot.
//!
//! The engine does its work on blocking threads. A call that only reads
//! answers from committed state and does not wait for a run in progress.
//! Calls that write take their turn: the engine has one writer.
//!
//! # Calls that start work
//!
//! `POST /v1/load`, `POST /v1/retract`, `POST /v1/documents`,
//! `POST /v1/extractions` and `POST /v1/match` say how long they wait with
//! `wait_ms`. If the work is done by then, the response is `200` with the
//! finished run. Otherwise it is `202` with the run as far as it has got, and
//! the caller reads `GET /v1/runs/{run}` until its status is no longer
//! `running`. The work goes on whether or not anyone is waiting.
//!
//! A call that finds the writer busy has no run to report until its own work
//! starts, so it can wait longer than `wait_ms` for that.
//!
//! # Bodies the server does not read
//!
//! A request is refused as soon as it can be: one without a caller before
//! its body is read, and one over the size limit when the limit is passed.
//! What is left of the body is then in the way of the next request. Up to
//! 64 KB of it is read and dropped, and the connection goes on. When more is
//! left, the answer says `Connection: close`, and the server ends its side of
//! the connection and drops what the client is still sending, within bounds,
//! so that the client finishes writing and reads the answer
//! (`server/unread.rs`).
//!
//! # Stopping
//!
//! A server that is told to stop takes no new connection and closes those
//! with no request on them. It then stops within a fixed time, whatever its
//! clients do:
//!
//! - A request that waits on a run is answered at once, with the run as far
//!   as it has got. `wait_ms` has no upper limit, so the stop cannot wait for
//!   it; the caller reads the run from the server that starts next.
//! - Every other request in flight is answered if that takes no more than
//!   [`STOP_WITHIN`], 5 seconds. A request that has not arrived in full by
//!   then, or whose answer is not ready, gets none.
//! - A connection that is being read out after a refusal has one second
//!   after the last request is answered or given up, so the longest a stop
//!   takes is 6 seconds.
//!
//! The engine's work is not cut off. A run the engine was given goes on in a
//! thread of its own, and `spool serve` does not exit until it has ended.

use std::future::{Future, IntoFuture};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, post};
use spool::Engine;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tower_sessions::cookie::SameSite;
use tower_sessions::cookie::time::Duration as SessionLength;
use tower_sessions::{Expiry, SessionManagerLayer};

use crate::api;
use crate::auth::Accounts;
use crate::auth::google::{Google, Provider};
use crate::studio;

mod auth;
mod error;
mod handlers;
mod local;
mod mcp;
mod newline;
mod request;
mod unread;
mod work;

use auth::{
    Attempts, Held, authorize, authorize_mcp, google, oauth, own_pages_only, people, sign_in,
    tokens,
};
use error::{describe_failure, not_found};
use handlers::{
    changesets, documents, drafts, explain, identities, info, load, recompute, records, review,
    runs,
};

pub use auth::oauth::issuer_of;

/// How long a call that starts work waits when it does not say.
pub const DEFAULT_WAIT_MS: u64 = 30_000;

/// How long a server that was told to stop goes on answering the requests
/// it has, by default. What is unfinished after that is left unanswered.
pub const STOP_WITHIN: Duration = Duration::from_secs(5);

/// The largest request body the server reads by default.
pub const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

/// The path of the MCP endpoint.
pub const MCP_PATH: &str = "/mcp";

/// How long a person stays signed in, counted from when they signed in: a
/// session is written when it changes, not each time it is used.
const SESSION_HOURS: i64 = 12;

/// The name of the session cookie.
const COOKIE: &str = "spool_session";

/// The largest body the paths that sign people in read. Anyone can send one,
/// and what they are about is a few names and a password.
const MAX_AUTH_BODY_BYTES: usize = 64 * 1024;

/// The most passwords hashed at once. A hash is made to cost time and memory,
/// and a sign-in costs one whether or not the account exists, so anyone who
/// reaches the server could otherwise have it make as many as they sent.
const MAX_HASHING: usize = 4;

/// The most bodies of delimited text read into rows at once. Reading one
/// takes a thread and as much memory again as the body, so the ones past
/// these wait their turn rather than each taking a thread of its own.
const MAX_READING: usize = 4;

/// The most questions asked of Google at once: who a code is for, and
/// where its sign-in is while the server does not know. Each waits for
/// Google, and anyone can have the server ask one.
const MAX_ASKING: usize = 8;

#[derive(Clone, Debug)]
pub struct ServerOptions {
    /// The people who may sign in, and their sessions.
    pub accounts: Accounts,
    /// A caller that presents nothing is the `dev` account, an admin. For
    /// `spool dev`, which refuses to listen anywhere but on this machine.
    pub dev: bool,
    /// The code that makes the first account, while no admin can sign in.
    pub setup: Option<String>,
    /// Whether browsers reach the server over HTTPS, and so whether the
    /// session cookie is sent only over HTTPS.
    pub https: bool,
    /// The client this server is to Google, if it signs people in with
    /// Google. The accounts are told with [`Accounts::with_google`].
    pub google: Option<Google>,
    pub max_body_bytes: usize,
    /// Whether the server answers a browser at `/` with the studio.
    pub studio: bool,
    /// The address the server is reached by, if it is the OAuth server of
    /// its own `/mcp`: see `server/auth/oauth.rs`. Approving an assistant is done
    /// in the studio, so a server without the studio has none.
    pub issuer: Option<String>,
    /// How long the server goes on answering the requests it has once it
    /// is told to stop.
    pub stop_within: Duration,
}

impl ServerOptions {
    pub fn new(accounts: Accounts) -> Self {
        Self {
            accounts,
            dev: false,
            setup: None,
            https: true,
            google: None,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            studio: true,
            issuer: None,
            stop_within: STOP_WITHIN,
        }
    }
}

#[derive(Clone)]
struct App {
    engine: Arc<Engine>,
    accounts: Accounts,
    dev: bool,
    setup: Arc<Mutex<Option<String>>>,
    attempts: Attempts,
    hashing: Arc<Semaphore>,
    reading: Arc<Semaphore>,
    https: bool,
    google: Option<Arc<Provider>>,
    asking: Arc<Semaphore>,
    /// Authorization addresses that do not fit in the Google sign-in cookie.
    held: Held,
    issuer: Option<Arc<str>>,
    stopping: Stopping,
}

/// Whether the server has been told to stop. A request that waits on a run
/// stops waiting then, and answers with the run as far as it has got.
#[derive(Clone)]
struct Stopping(Arc<watch::Sender<bool>>);

impl Stopping {
    fn new() -> Self {
        Self(Arc::new(watch::Sender::new(false)))
    }

    fn tell(&self) {
        self.0.send_replace(true);
    }

    /// Completes once the server has been told to stop.
    async fn told(&self) {
        // The sender is held here, so the wait ends only with the word.
        let _ = self.0.subscribe().wait_for(|told| *told).await;
    }
}

/// The API over `engine`.
pub fn router(engine: Arc<Engine>, options: &ServerOptions) -> Router {
    router_until(engine, options, Stopping::new())
}

/// The API over `engine`, whose requests are told through `stopping` when
/// the server is.
fn router_until(engine: Arc<Engine>, options: &ServerOptions, stopping: Stopping) -> Router {
    let app = App {
        engine,
        accounts: options.accounts.clone(),
        dev: options.dev,
        setup: Arc::new(Mutex::new(options.setup.clone())),
        attempts: Attempts::default(),
        hashing: Arc::new(Semaphore::new(MAX_HASHING)),
        reading: Arc::new(Semaphore::new(MAX_READING)),
        https: options.https,
        google: options.google.clone().map(Provider::new).map(Arc::new),
        asking: Arc::new(Semaphore::new(MAX_ASKING)),
        held: Held::default(),
        issuer: options
            .issuer
            .as_deref()
            .filter(|_| options.studio && !options.dev)
            .map(Arc::from),
        stopping,
    };
    let sessions = SessionManagerLayer::new(options.accounts.clone())
        .with_name(COOKIE)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_secure(options.https)
        .with_expiry(Expiry::OnInactivity(SessionLength::hours(SESSION_HOURS)));
    let auth = Router::new()
        .route("/session", get(sign_in::session_state))
        .route("/sign-in", post(sign_in::sign_in))
        .route("/sign-out", post(sign_in::sign_out))
        .route("/setup", post(sign_in::set_up))
        .route("/password", post(sign_in::change_password))
        .route("/google", get(google::google_start))
        .route("/google/callback", get(google::google_callback))
        .route("/oauth", get(oauth::describe).post(oauth::decide))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(
            options.max_body_bytes.min(MAX_AUTH_BODY_BYTES),
        ));
    let v1 = api_routes()
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(app.clone(), authorize));
    let mcp = Router::new()
        .route_service(MCP_PATH, mcp::build_mcp_service(app.clone(), options))
        .layer(middleware::map_response(mcp::describe_refusal))
        .layer(middleware::from_fn_with_state(app.clone(), authorize_mcp));
    // Clients send these, and anyone may: what they read is a few fields.
    let oauth = Router::new()
        .route(oauth::RESOURCE_METADATA, get(oauth::resource_metadata))
        .route(
            &format!("{}{MCP_PATH}", oauth::RESOURCE_METADATA),
            get(oauth::resource_metadata),
        )
        .route(oauth::SERVER_METADATA, get(oauth::server_metadata))
        .route(oauth::REGISTER, post(oauth::register))
        .route(oauth::AUTHORIZE, get(oauth::authorize))
        .route(oauth::TOKEN, post(oauth::token))
        .route(oauth::REVOKE, post(oauth::revoke))
        .layer(DefaultBodyLimit::max(
            options.max_body_bytes.min(MAX_AUTH_BODY_BYTES),
        ));
    let mut router = Router::new()
        .route("/healthz", get(info::health))
        .nest("/auth", auth)
        .nest(&format!("/{}", api::VERSION), v1)
        .merge(mcp)
        .merge(oauth);
    if options.studio {
        router = router.merge(studio::routes());
    }
    let router = router
        .fallback(not_found)
        .layer(sessions)
        .layer(middleware::from_fn(own_pages_only))
        .layer(DefaultBodyLimit::max(options.max_body_bytes))
        .layer(middleware::map_response(describe_failure))
        .with_state(app);
    let router = match options.dev {
        true => router.layer(middleware::from_fn(local::this_machine_only)),
        false => router,
    };
    router
        .layer(middleware::from_fn(newline::end_with_a_newline))
        .layer(middleware::from_fn(unread::finish_the_body))
}

/// The routes of the API, each to the handler of its operation.
fn api_routes() -> Router<App> {
    Router::new()
        .route("/info", get(info::info))
        .route("/load", post(load::load))
        .route("/retract", post(load::retract))
        .route(
            "/documents",
            get(documents::list_documents).post(documents::post_documents),
        )
        .route("/documents/add", post(documents::add_document))
        .route("/documents/{document}", get(documents::read_document))
        .route("/extractions", post(documents::extract))
        .route("/match", post(runs::start_match))
        .route("/recompute", post(recompute::recompute))
        .route("/recompute/apply", post(recompute::apply_recompute))
        .route("/runs", get(runs::list_runs))
        .route("/runs/{run}", get(runs::read_run))
        .route("/runs/{run}/candidates", get(runs::candidates))
        .route("/runs/{run}/judgements", post(runs::submit_judgements))
        .route("/runs/{run}/reconcile", post(runs::reconcile))
        .route("/runs/{run}/cancel", post(runs::cancel_run))
        .route("/drafts", get(drafts::list_drafts))
        .route("/drafts/{draft}", get(drafts::read_draft))
        .route("/drafts/{draft}/approve", post(drafts::approve_draft))
        .route("/drafts/{draft}/reject", post(drafts::reject_draft))
        .route(
            "/changesets",
            get(changesets::list_changesets).post(changesets::commit),
        )
        .route("/changesets/{seq}/undo", post(changesets::undo))
        .route("/ids", post(changesets::mint_ids))
        .route("/identities", get(identities::list_identities))
        .route("/identities/{identity}", get(identities::read_identity))
        .route("/identities/{identity}/at", get(identities::identity_at))
        .route(
            "/identities/{identity}/history",
            get(identities::identity_history),
        )
        .route(
            "/identities/{identity}/explain",
            get(explain::explain_identity),
        )
        .route("/identities/{identity}/split", post(changesets::split))
        .route("/constraints", get(changesets::list_constraints))
        .route(
            "/records",
            get(records::list_records).post(load::add_record),
        )
        .route("/records/identities", post(records::identities_of))
        .route("/records/{record}", get(records::read_record))
        .route("/attributes", get(records::list_attributes))
        .route("/explain", get(explain::explain))
        .route("/review", get(review::list_review))
        .route("/review/{item}", get(review::read_review))
        .route("/review/{item}/case", get(review::review_case))
        .route("/review/{item}/dismiss", post(review::dismiss_review))
        .route("/people", get(people::list_people).post(people::add_person))
        .route("/people/{person}", post(people::change_person))
        .route("/tokens", get(tokens::list_tokens).post(tokens::add_token))
        .route("/tokens/{token}/revoke", post(tokens::revoke_token))
        .route("/grants", get(oauth::list_grants))
        .route("/grants/{grant}/revoke", post(oauth::end_grant))
}

/// Serves the API on `listener` until `shutdown` completes, and then stops
/// as the module says under "Stopping": requests in flight have
/// [`ServerOptions::stop_within`] to be answered, and this returns whether
/// or not they were.
///
/// The connections that were given up, and the work the engine was given,
/// outlive this. They belong to the runtime it runs on: dropping the runtime
/// ends the connections, and waits for the work.
pub async fn serve(
    listener: TcpListener,
    engine: Arc<Engine>,
    options: &ServerOptions,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let (connections, read_out) = unread::Connections::of(listener);
    let stopping = Stopping::new();
    let router = router_until(engine, options, stopping.clone())
        .into_make_service_with_connect_info::<unread::Unread>();
    let told = stopping.clone();
    let serving = axum::serve(connections, router)
        .with_graceful_shutdown(async move {
            shutdown.await;
            told.tell();
        })
        .into_future();
    let out_of_time = async {
        stopping.told().await;
        tokio::time::sleep(options.stop_within).await;
    };
    tokio::select! {
        served = serving => {
            served?;
            read_out.wait().await;
        }
        // The connections that are left end with the runtime.
        () = out_of_time => read_out.wait_for_the_answered().await,
    }
    Ok(())
}
