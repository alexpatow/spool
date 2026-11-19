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
//! (`server/oauth.rs`).
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

use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatchReader;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, RawQuery, Request, State};
use axum::http::header::{
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE as COOKIES, HOST, LOCATION, ORIGIN,
    SET_COOKIE, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde::de::DeserializeOwned;
use spool::core::{Actor, Changeset, DocumentId, EntityType, Policy, RecordId, RunId, Seq};
use spool::{
    ActorNames, AttributeFilter, ConstraintFilter, Direction, Disposition, DocumentFilter,
    DocumentSort, DraftFilter, DraftStatus, Engine, ExtractOptions, IdentityFilter, IdentitySort,
    LogSort, RecordFilter, RecordSort, ResolveOptions, ReviewFilter, ReviewStatus, Rows, RunFilter,
    RunKind, RunSort, RunStatus, Scope, Sort, StrategyRef, watch_runs,
};
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tower_sessions::cookie::time::Duration as SessionLength;
use tower_sessions::cookie::{Cookie, SameSite};
use tower_sessions::{Expiry, Session, SessionManagerLayer};

use crate::accounts::{
    ACCESS_PREFIX, Accounts, Change, Entered, GOOGLE, MARK, MAX_USERNAME, New, PERSON, Refused,
};
use crate::api::{
    self, ARROW_STREAM, AddPersonRequest, AddTokenRequest, AddedToken, AttributeList,
    CandidateList, ChangePersonRequest, ChangesetList, CommitRequest, ConstraintList,
    DismissRequest, DocumentList, DocumentsRequest, DraftList, ErrorBody, ErrorCode, ErrorResponse,
    ExtractRequest, History, IdentitiesOfRequest, IdentitiesOfResponse, IdentityList, Info,
    JudgementsRequest, JudgementsResponse, LoadParams, LoadRequest, MatchRequest, MintRequest,
    PasswordRequest, Person, PersonList, ReconcileRequest, RecordList, RejectDraftRequest,
    RetractRequest, ReviewList, RunList, RunResponse, SessionState, SetupRequest, SignInRequest,
    SplitRequest, TokenList, UndoRequest,
};
use crate::delimited::{self, Malformed};
use crate::google::{Failed, Google, Provider, Started};
use crate::studio;

mod local;
mod mcp;
mod newline;
mod oauth;
mod unread;

pub use oauth::issuer_of;

/// How long a call that starts work waits when it does not say.
pub const DEFAULT_WAIT_MS: u64 = 30_000;

/// How long a server that was told to stop goes on answering the requests
/// it has, by default. What is unfinished after that is left unanswered.
pub const STOP_WITHIN: Duration = Duration::from_secs(5);

/// The largest request body the server reads by default.
pub const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

/// The path of the MCP endpoint.
pub const MCP_PATH: &str = "/mcp";

/// The most entries a list returns in one page.
const MAX_PAGE: u64 = 1_000;
const DEFAULT_PAGE: u64 = 100;

/// The most IDs minted in one call.
const MAX_MINTED: u32 = 10_000;

/// How long a person stays signed in, counted from when they signed in: a
/// session is written when it changes, not each time it is used.
const SESSION_HOURS: i64 = 12;

/// The name of the session cookie.
const COOKIE: &str = "spool_session";

/// How many wrong passwords one username may have within [`ATTEMPT_WINDOW`]
/// before sign-in for it is refused until the window has passed.
const MAX_ATTEMPTS: u32 = 10;
const ATTEMPT_WINDOW: Duration = Duration::from_secs(15 * 60);

/// The most usernames whose wrong passwords are counted at once. Anyone can
/// try a username, so what is kept of the tries has to have an end.
const MAX_ATTEMPTED: usize = 10_000;

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

/// The name of the cookie that keeps a sign-in with Google while the
/// browser is at Google.
const GOOGLE_COOKIE: &str = "spool_google";

/// How long someone has at Google before the sign-in has to start again.
const GOOGLE_MINUTES: i64 = 10;

/// The most questions asked of Google at once: who a code is for, and
/// where its sign-in is while the server does not know. Each waits for
/// Google, and anyone can have the server ask one.
const MAX_ASKING: usize = 8;

/// The longest address to go back to after signing in with Google. It
/// travels in a cookie, which has little room.
const MAX_NEXT_BYTES: usize = 1024;

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
    /// its own `/mcp`: see `server/oauth.rs`. Approving an assistant is done
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

/// The wrong passwords lately tried for each username, and when the first of
/// them was.
#[derive(Clone, Default)]
struct Attempts(Arc<Mutex<HashMap<String, (u32, Instant)>>>);

impl Attempts {
    fn held(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u32, Instant)>> {
        self.0.lock().unwrap_or_else(|held| held.into_inner())
    }

    /// Whether a username has had its tries for now.
    fn barred(&self, username: &str) -> bool {
        let mut held = self.held();
        held.retain(|_, (_, first)| first.elapsed() < ATTEMPT_WINDOW);
        held.get(username)
            .is_some_and(|(wrong, _)| *wrong >= MAX_ATTEMPTS)
    }

    fn wrong(&self, username: String) {
        let mut held = self.held();
        if held.len() >= MAX_ATTEMPTED && !held.contains_key(&username) {
            // Counting starts again sooner than it should, which is better
            // than keeping every name anyone ever tried.
            held.clear();
        }
        held.entry(username).or_insert((0, Instant::now())).0 += 1;
    }

    fn forget(&self, username: &str) {
        self.held().remove(username);
    }
}

/// Who is calling, and so who a change they make is recorded under.
#[derive(Clone, Debug)]
struct Caller {
    actor: Actor,
    /// The account, when the caller is a person.
    person: Option<Person>,
}

impl Caller {
    fn of(person: Person) -> Self {
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

    fn admin(&self) -> ApiResult<&Person> {
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
        .route("/session", get(session_state))
        .route("/sign-in", post(sign_in))
        .route("/sign-out", post(sign_out))
        .route("/setup", post(set_up))
        .route("/password", post(change_password))
        .route("/google", get(google_start))
        .route("/google/callback", get(google_callback))
        .route("/oauth", get(oauth::describe).post(oauth::decide))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(
            options.max_body_bytes.min(MAX_AUTH_BODY_BYTES),
        ));
    let v1 = Router::new()
        .route("/info", get(info))
        .route("/load", post(load))
        .route("/retract", post(retract))
        .route("/documents", get(list_documents).post(post_documents))
        .route("/documents/{document}", get(read_document))
        .route("/extractions", post(extract))
        .route("/match", post(start_match))
        .route("/recompute", post(recompute))
        .route("/recompute/apply", post(apply_recompute))
        .route("/runs", get(list_runs))
        .route("/runs/{run}", get(read_run))
        .route("/runs/{run}/candidates", get(candidates))
        .route("/runs/{run}/judgements", post(submit_judgements))
        .route("/runs/{run}/reconcile", post(reconcile))
        .route("/runs/{run}/cancel", post(cancel_run))
        .route("/drafts", get(list_drafts))
        .route("/drafts/{draft}", get(read_draft))
        .route("/drafts/{draft}/approve", post(approve_draft))
        .route("/drafts/{draft}/reject", post(reject_draft))
        .route("/changesets", get(list_changesets).post(commit))
        .route("/changesets/{seq}/undo", post(undo))
        .route("/ids", post(mint_ids))
        .route("/identities", get(list_identities))
        .route("/identities/{identity}", get(read_identity))
        .route("/identities/{identity}/history", get(identity_history))
        .route("/identities/{identity}/explain", get(explain_identity))
        .route("/identities/{identity}/split", post(split))
        .route("/constraints", get(list_constraints))
        .route("/records", get(list_records))
        .route("/records/identities", post(identities_of))
        .route("/records/{record}", get(read_record))
        .route("/attributes", get(list_attributes))
        .route("/explain", get(explain))
        .route("/review", get(list_review))
        .route("/review/{item}", get(read_review))
        .route("/review/{item}/case", get(review_case))
        .route("/review/{item}/dismiss", post(dismiss_review))
        .route("/people", get(list_people).post(add_person))
        .route("/people/{person}", post(change_person))
        .route("/tokens", get(list_tokens).post(add_token))
        .route("/tokens/{token}/revoke", post(revoke_token))
        .route("/grants", get(oauth::list_grants))
        .route("/grants/{grant}/revoke", post(oauth::end_grant))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(app.clone(), authorize));
    let mcp = Router::new()
        .route_service(MCP_PATH, mcp::service(app.clone(), options))
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
        .route("/healthz", get(health))
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

// Errors

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

impl ApiError {
    fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorBody {
                code,
                message: message.into(),
                rejections: None,
            },
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::Invalid, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            message,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(ErrorResponse { error: self.body })).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

impl From<spool::Error> for ApiError {
    fn from(error: spool::Error) -> Self {
        use spool::Error;
        let message = error.to_string();
        match error {
            Error::Rejected(rejections) => Self {
                status: StatusCode::CONFLICT,
                body: ErrorBody {
                    code: ErrorCode::Rejected,
                    message,
                    rejections: Some(rejections),
                },
            },
            Error::UnknownRun(_)
            | Error::UnknownDraft(_)
            | Error::UnknownIdentity(_)
            | Error::UnknownDocument(_)
            | Error::UnknownRecord(_)
            | Error::UnknownChangeset(_)
            | Error::UnknownReviewItem(_) => Self::not_found(message),
            Error::Run { .. }
            | Error::Draft { .. }
            | Error::Review { .. }
            | Error::Split { .. }
            | Error::StalePlan { .. }
            | Error::Retracted(_)
            | Error::NothingToUndo(_) => {
                Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, message)
            }
            Error::Load(_)
            | Error::Retract(_)
            | Error::Documents(_)
            | Error::UnknownExtractor { .. }
            | Error::Judgements(_)
            | Error::Rows(_)
            | Error::Arrow(_)
            | Error::InvalidEntityType(_)
            | Error::UndeclaredType(_)
            | Error::SameRecord(_)
            | Error::Recompute(_)
            | Error::UnknownStrategy { .. } => Self::invalid(message),
            Error::Strategy { .. } => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::StrategyFailed,
                message,
            ),
            Error::Extractor { .. } | Error::Mentions(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::ExtractorFailed,
                message,
            ),
            _ => Self::internal(message),
        }
    }
}

type ApiResult<T = Response> = Result<T, ApiError>;

/// Gives a failure the framework answered itself, such as a body over the
/// limit or a method a path does not take, the same JSON body as the rest.
async fn describe_failure(response: Response) -> Response {
    let status = response.status();
    let described = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    if described || !(status.is_client_error() || status.is_server_error()) {
        return response;
    }
    let (code, message) = match status {
        StatusCode::PAYLOAD_TOO_LARGE => (
            ErrorCode::TooLarge,
            "the body is larger than this server reads",
        ),
        StatusCode::METHOD_NOT_ALLOWED => {
            (ErrorCode::Invalid, "this path does not take this method")
        }
        StatusCode::NOT_FOUND => (ErrorCode::NotFound, "there is nothing at this path"),
        _ if status.is_server_error() => (ErrorCode::Internal, "the server failed"),
        _ => (ErrorCode::Invalid, "the request could not be read"),
    };
    let mut described = ApiError::new(status, code, message).into_response();
    if let Some(allow) = response.headers().get(axum::http::header::ALLOW) {
        described
            .headers_mut()
            .insert(axum::http::header::ALLOW, allow.clone());
    }
    described
}

async fn not_found() -> ApiError {
    ApiError::not_found("there is nothing at this path")
}

impl From<Refused> for ApiError {
    fn from(refused: Refused) -> Self {
        let message = refused.to_string();
        match refused {
            Refused::Invalid(_) | Refused::WrongPassword => Self::invalid(message),
            Refused::Unknown | Refused::UnknownToken | Refused::UnknownGrant => {
                Self::not_found(message)
            }
            Refused::TooMany(_) => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                ErrorCode::TooManyAttempts,
                message,
            ),
            Refused::Taken(_)
            | Refused::EmailTaken(_)
            | Refused::TokenTaken(_)
            | Refused::LastAdmin
            | Refused::SetUp => Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, message),
            Refused::Failed(_) => Self::internal(message),
        }
    }
}

// Who is calling

/// Refuses a write that a page of another site sent. A browser says where a
/// write comes from, and may send this server's cookie with it wherever that
/// is; a caller that is not a browser says nothing and is let through.
async fn own_pages_only(request: Request, next: Next) -> Response {
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

async fn authorize(
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
async fn authorize_mcp(
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

fn unauthorized(message: &str) -> ApiError {
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
async fn signed_in(app: &App, session: &Session) -> ApiResult<Option<Person>> {
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

fn session_failed(error: tower_sessions::session::Error) -> ApiError {
    ApiError::internal(format!("the session could not be read: {error}"))
}

/// Runs work on the accounts on a blocking thread: hashing a password takes
/// a while.
async fn people<T: Send + 'static>(
    app: &App,
    work: impl FnOnce(&Accounts) -> Result<T, Refused> + Send + 'static,
) -> ApiResult<T> {
    let accounts = app.accounts.clone();
    match tokio::task::spawn_blocking(move || work(&accounts)).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(ApiError::internal("the accounts stopped while working")),
    }
}

/// What each person is called, by the actor the log names them as: a list
/// sorted by who did something sorts them as the studio shows them.
async fn names(app: &App) -> ApiResult<ActorNames> {
    let people = people(app, Accounts::list).await?;
    Ok(people
        .into_iter()
        .map(|person| (Actor::User(person.id), person.name))
        .collect())
}

/// Runs work that hashes a password, which `permit` allows: it is held until
/// the work is done, even if whoever asked has gone.
async fn hashing<T: Send + 'static>(
    app: &App,
    permit: OwnedSemaphorePermit,
    work: impl FnOnce(&Accounts) -> Result<T, Refused> + Send + 'static,
) -> ApiResult<T> {
    people(app, move |accounts| {
        let _held = permit;
        work(accounts)
    })
    .await
}

/// Leave to hash a password, for a caller the server has reason to wait
/// for: one who is signed in, or who has the setup code.
async fn turn(app: &App) -> ApiResult<OwnedSemaphorePermit> {
    Arc::clone(&app.hashing)
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("the accounts stopped while working"))
}

/// Starts a session for whoever entered, with an ID nobody has seen before.
/// `with` is what they entered with, and so what the session lasts as long
/// as: [`MARK`] and the mark of their password, or [`GOOGLE`] and the mark
/// of their Google account.
async fn begin(session: &Session, person: &str, with: (&str, &str)) -> ApiResult<()> {
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

async fn session_state(State(app): State<App>, session: Session) -> ApiResult {
    let mut person = signed_in(&app, &session).await?;
    if person.is_none() && app.dev {
        person = Some(people(&app, Accounts::dev).await?);
    }
    let setup = person.is_none() && people(&app, Accounts::needs_setup).await?;
    ok(SessionState {
        person,
        setup,
        dev: app.dev,
        google: app.google.is_some(),
    })
}

/// What a call that signs someone in or out answers with.
fn now(app: &App, person: Option<Person>) -> ApiResult {
    ok(SessionState {
        person,
        setup: false,
        dev: app.dev,
        google: app.google.is_some(),
    })
}

async fn sign_in(State(app): State<App>, session: Session, body: Bytes) -> ApiResult {
    let request: SignInRequest = json_body(&body)?;
    let username = request.username.trim().to_lowercase();
    let wrong = || unauthorized("the username or the password is not right");
    if username.len() > MAX_USERNAME {
        return Err(wrong());
    }
    if app.attempts.barred(&username) {
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
    let entered = hashing(&app, permit, move |accounts| {
        accounts.verify(&attempted, &request.password)
    })
    .await?;
    let Some(Entered { person, mark }) = entered else {
        app.attempts.wrong(username);
        return Err(wrong());
    };
    app.attempts.forget(&username);
    begin(&session, &person.id, (MARK, &mark)).await?;
    now(&app, Some(person))
}

async fn sign_out(State(app): State<App>, session: Session) -> ApiResult {
    session.flush().await.map_err(session_failed)?;
    now(&app, None)
}

/// Makes the first account. The code is the one the server printed when it
/// started with no admin who could sign in, and it works once.
async fn set_up(State(app): State<App>, session: Session, body: Bytes) -> ApiResult {
    let request: SetupRequest = json_body(&body)?;
    let right = {
        let code = app.setup.lock().unwrap_or_else(|held| held.into_inner());
        code.as_deref()
            .is_some_and(|code| same(code.as_bytes(), request.code.trim().as_bytes()))
    };
    if !right {
        return Err(unauthorized(
            "that is not the code in the link this server printed when it started",
        ));
    }
    let Entered { person, mark } = hashing(&app, turn(&app).await?, move |accounts| {
        accounts.first(&request.username, &request.name, &request.password)
    })
    .await?;
    *app.setup.lock().unwrap_or_else(|held| held.into_inner()) = None;
    begin(&session, &person.id, (MARK, &mark)).await?;
    now(&app, Some(person))
}

async fn change_password(State(app): State<App>, session: Session, body: Bytes) -> ApiResult {
    let request: PasswordRequest = json_body(&body)?;
    let Some(person) = signed_in(&app, &session).await? else {
        return Err(unauthorized("sign in first"));
    };
    let id = person.id.clone();
    let mark = hashing(&app, turn(&app).await?, move |accounts| {
        accounts.change_password(&id, &request.current, &request.new)
    })
    .await?;
    // Every session of theirs has ended. This one starts again.
    begin(&session, &person.id, (MARK, &mark)).await?;
    now(&app, Some(person))
}

// Signing in with Google
//
// The browser is sent to Google and comes back with a code. What has to be
// kept in between is kept by the browser, in a cookie: anyone can start a
// sign-in, and a server that kept one for each would keep as many as anyone
// cared to start.

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
fn keep(app: &App, value: Option<&str>) -> HeaderValue {
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
    headers.insert(SET_COOKIE, keep(app, kept));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// An address of this server's own pages to go back to: `next` if it is
/// one, and the first page if it could lead anywhere else.
fn own_page(next: Option<&str>) -> &str {
    let own = |next: &&str| {
        next.len() <= MAX_NEXT_BYTES
            && next.starts_with('/')
            && !next[1..].starts_with(['/', '\\'])
            && !next.contains(|c: char| c.is_control() || c == '\\')
    };
    next.filter(own).unwrap_or("/")
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<String> {
    let digits = text.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let bytes = digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
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
    fn written(started: &Started, next: &str) -> String {
        // The first three are base64 without a `.` in it.
        format!(
            "{}.{}.{}.{}",
            started.state,
            started.nonce,
            started.verifier,
            hex(next.as_bytes())
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
            next: unhex(parts.next()?)?,
        };
        (parts.next().is_none() && !kept.state.is_empty()).then_some(kept)
    }
}

/// Says on the server's own output why a sign-in with Google failed: the
/// person at the browser is told that it did, and whoever runs the server
/// can do something about it.
fn complain(failed: &Failed) {
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr(),
        "spool: a sign-in with Google failed: {failed}"
    );
}

async fn asked<T: Send + 'static>(
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
struct GoogleStart {
    /// The page to come back to.
    next: Option<String>,
}

/// `GET /auth/google`: sends the browser to Google to sign in.
async fn google_start(
    State(app): State<App>,
    start: Result<Query<GoogleStart>, QueryRejection>,
) -> ApiResult {
    let Some(provider) = &app.google else {
        return Err(not_found().await);
    };
    let start = query(start)?;
    let next = own_page(start.next.as_deref());
    // Sending a browser there asks Google nothing once the server knows
    // where its sign-in is. Until then, it is one of the questions the
    // server has only so many of at once.
    let asking = match provider.knows() {
        true => None,
        false => match Arc::clone(&app.asking).try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return Ok(turned_away(&app, next, "busy")),
        },
    };
    let start = move |provider: &Provider| {
        let _held = asking;
        provider.start()
    };
    Ok(match asked(provider, start).await {
        Ok(started) => send_to(&app, &started.url, Some(&Kept::written(&started, next))),
        Err(failed) => {
            complain(&failed);
            turned_away(&app, next, "unreachable")
        }
    })
}

#[derive(serde::Deserialize)]
struct GoogleBack {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /auth/google/callback`: where Google sends the browser back to.
/// Signs in whoever Google says it is, if they have an account, and goes to
/// the page the sign-in started from.
async fn google_callback(
    State(app): State<App>,
    session: Session,
    headers: HeaderMap,
    back: Result<Query<GoogleBack>, QueryRejection>,
) -> ApiResult {
    let Some(provider) = &app.google else {
        return Err(not_found().await);
    };
    let back = query(back)?;
    // The browser that comes back is the one that was sent: it kept what
    // Google repeats.
    let kept = Kept::read(&app, &headers).filter(|kept| {
        back.state
            .as_deref()
            .is_some_and(|state| same(state.as_bytes(), kept.state.as_bytes()))
    });
    let Some(kept) = kept else {
        return Ok(turned_away(&app, "/", "expired"));
    };
    let next = own_page(Some(&kept.next));
    let code = match (back.code, back.error.as_deref()) {
        (Some(code), None) => code,
        (_, Some("access_denied")) => return Ok(turned_away(&app, next, "cancelled")),
        _ => return Ok(turned_away(&app, next, "failed")),
    };
    let Ok(permit) = Arc::clone(&app.asking).try_acquire_owned() else {
        return Ok(turned_away(&app, next, "busy"));
    };
    let Kept {
        nonce, verifier, ..
    } = &kept;
    let (nonce, verifier) = (nonce.clone(), verifier.clone());
    let identity = asked(provider, move |provider| {
        let _held = permit;
        provider.finish(&code, &nonce, &verifier)
    })
    .await;
    let identity = match identity {
        Ok(identity) => identity,
        Err(failed) => {
            complain(&failed);
            let why = match failed {
                Failed::Unverified => "unverified",
                Failed::Unreachable(_) => "unreachable",
                Failed::Refused(_) => "failed",
            };
            return Ok(turned_away(&app, next, why));
        }
    };
    // Without an address Google answers for, nobody is found by it.
    let why = match identity.email {
        Some(_) => "unknown",
        None => "unvouched",
    };
    let entered = people(&app, move |accounts| {
        accounts.enter_with_google(&identity.subject, identity.email.as_deref())
    })
    .await?;
    let Some((person, bound)) = entered else {
        return Ok(turned_away(&app, next, why));
    };
    begin(&session, &person.id, (GOOGLE, &bound)).await?;
    Ok(send_to(&app, next, None))
}

async fn list_people(State(app): State<App>) -> ApiResult {
    ok(PersonList {
        people: people(&app, Accounts::list).await?,
    })
}

async fn add_person(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    caller.admin()?;
    let request: AddPersonRequest = json_body(&body)?;
    let sets_password = request.password.is_some();
    let add = move |accounts: &Accounts| {
        accounts.add(New {
            username: request.username,
            name: request.name,
            password: request.password,
            email: request.email,
            admin: request.admin.unwrap_or(false),
        })
    };
    ok(match sets_password {
        true => hashing(&app, turn(&app).await?, add).await?,
        false => people(&app, add).await?,
    })
}

async fn change_person(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(person): Path<String>,
    body: Bytes,
) -> ApiResult {
    caller.admin()?;
    let request: ChangePersonRequest = json_body(&body)?;
    let change = Change {
        name: request.name,
        password: request.password,
        email: request.email,
        admin: request.admin,
        active: request.active,
    };
    let sets_password = change.password.is_some();
    let changed = move |accounts: &Accounts| accounts.change(&person, change);
    ok(match sets_password {
        true => hashing(&app, turn(&app).await?, changed).await?,
        false => people(&app, changed).await?,
    })
}

async fn list_tokens(State(app): State<App>, Extension(caller): Extension<Caller>) -> ApiResult {
    caller.admin()?;
    ok(TokenList {
        tokens: people(&app, Accounts::tokens).await?,
    })
}

async fn add_token(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let by = caller.admin()?.id.clone();
    let request: AddTokenRequest = json_body(&body)?;
    let (token, secret) = people(&app, move |accounts| {
        accounts.add_token(&request.name, Some(&by))
    })
    .await?;
    ok(AddedToken { token, secret })
}

async fn revoke_token(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(token): Path<String>,
) -> ApiResult {
    caller.admin()?;
    ok(people(&app, move |accounts| accounts.revoke_token(&token)).await?)
}

/// Compares without stopping at the first difference, so that the time taken
/// says nothing about how much of a guess was right.
fn same(a: &[u8], b: &[u8]) -> bool {
    let mut difference = a.len() ^ b.len();
    for index in 0..a.len().max(b.len()) {
        let x = a.get(index).copied().unwrap_or(0);
        let y = b.get(index).copied().unwrap_or(0);
        difference |= usize::from(x ^ y);
    }
    difference == 0
}

// Reading requests

fn json_body<T: DeserializeOwned>(body: &Bytes) -> ApiResult<T> {
    serde_json::from_slice(body)
        .map_err(|error| ApiError::invalid(format!("the body is not the JSON expected: {error}")))
}

/// A body that may be left out, in which case every field has its default.
fn optional_json_body<T: DeserializeOwned + Default>(body: &Bytes) -> ApiResult<T> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    json_body(body)
}

fn query<T>(query: Result<Query<T>, QueryRejection>) -> ApiResult<T> {
    match query {
        Ok(Query(value)) => Ok(value),
        Err(rejection) => Err(ApiError::invalid(format!(
            "the query string is not what this path takes: {}",
            rejection.body_text()
        ))),
    }
}

fn id<T>(text: &str, what: &str) -> ApiResult<T>
where
    T: std::str::FromStr,
{
    text.parse()
        .map_err(|_| ApiError::invalid(format!("`{text}` is not {what}")))
}

fn run_id(text: &str) -> ApiResult<RunId> {
    Ok(RunId(id(text, "a run ID")?))
}

/// Names between backticks, as a sentence lists them: `a`, `b` and `c`.
fn listed(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => quoted.join(""),
    }
}

fn is_text(headers: &HeaderMap) -> bool {
    media_type(headers, CONTENT_TYPE).is_some_and(|value| value.starts_with("text/"))
}

fn is_json(headers: &HeaderMap) -> bool {
    media_type(headers, CONTENT_TYPE).is_some_and(|value| value.starts_with("application/json"))
}

fn is_arrow(headers: &HeaderMap) -> bool {
    media_type(headers, CONTENT_TYPE).is_some_and(|value| value.starts_with(ARROW_STREAM))
}

fn media_type(headers: &HeaderMap, name: axum::http::HeaderName) -> Option<String> {
    Some(
        headers
            .get(name)?
            .to_str()
            .ok()?
            .trim()
            .to_ascii_lowercase(),
    )
}

/// The character between the fields of a body of delimited text, if the
/// body is one.
fn delimiter(headers: &HeaderMap) -> Option<u8> {
    let sent = media_type(headers, CONTENT_TYPE)?;
    if sent.starts_with(api::CSV) {
        Some(b',')
    } else if sent.starts_with(api::TSV) {
        Some(b'\t')
    } else {
        None
    }
}

/// The rows of a body of delimited text. Reading them is work, so it is
/// done off the threads that answer requests, and by no more at once than
/// `reading` has room for: a body waits its turn, and the one being read
/// keeps its place until it is done, even if whoever sent it has gone.
async fn delimited_rows(reading: &Arc<Semaphore>, body: Bytes, delimiter: u8) -> ApiResult<Rows> {
    /// As many as DuckDB hands on at a time.
    const ROWS_AT_A_TIME: usize = 2048;
    let stopped = || ApiError::internal("the server stopped while it was reading the body");
    let turn = Arc::clone(reading)
        .acquire_owned()
        .await
        .map_err(|_| stopped())?;
    let read = move || -> Result<Rows, Malformed> {
        let _turn = turn;
        let mut reader = delimited::Reader::new(body.as_ref(), delimiter)?;
        let mut batches = Vec::new();
        while let Some(batch) = reader.next_batch(ROWS_AT_A_TIME, usize::MAX)? {
            batches.push(batch);
        }
        Ok(Rows::new(reader.schema(), batches))
    };
    match tokio::task::spawn_blocking(read).await {
        Ok(Ok(rows)) => Ok(rows),
        Ok(Err(Malformed(why))) => Err(ApiError::invalid(format!(
            "the body cannot be read as {}: {why}",
            if delimiter == b'\t' {
                "tab-separated text"
            } else {
                "CSV"
            }
        ))),
        Err(_) => Err(stopped()),
    }
}

fn arrow_rows(body: Bytes) -> ApiResult<StreamReader<Cursor<Bytes>>> {
    StreamReader::try_new(Cursor::new(body), None)
        .map_err(|error| ApiError::invalid(format!("the body is not an Arrow IPC stream: {error}")))
}

fn page(limit: Option<u64>) -> u64 {
    limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}

/// The order a list was asked for: `sort` names what it is sorted by, and
/// `order` is `asc` or `desc`. `order` alone turns the list's own order,
/// which is by `own`. Neither leaves the list in its own order.
fn sorted<By>(sort: Option<By>, order: Option<Direction>, own: By) -> Option<Sort<By>> {
    match (sort, order) {
        (None, None) => None,
        (by, direction) => Some(Sort {
            by: by.unwrap_or(own),
            direction: direction.unwrap_or_default(),
        }),
    }
}

// Calling the engine

/// Runs engine work on a blocking thread.
async fn engine<T: Send + 'static>(
    app: &App,
    work: impl FnOnce(&Engine) -> spool::Result<T> + Send + 'static,
) -> ApiResult<T> {
    let engine = Arc::clone(&app.engine);
    match tokio::task::spawn_blocking(move || work(&engine)).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(ApiError::internal(
            "the engine stopped while it was working",
        )),
    }
}

/// Starts work that is recorded as a run and waits for it for `wait_ms`.
/// `work` returns the run that stands for the work, or `None` if there was
/// nothing to do.
async fn job(
    app: &App,
    wait_ms: Option<u64>,
    work: impl FnOnce(&Engine) -> spool::Result<Option<RunId>> + Send + 'static,
) -> ApiResult {
    let (announce, mut announced) = mpsc::unbounded_channel();
    let engine = Arc::clone(&app.engine);
    let mut working = tokio::task::spawn_blocking(move || {
        watch_runs(
            move |run| {
                let _ = announce.send(run);
            },
            || work(&engine),
        )
    });

    let wait = Duration::from_millis(wait_ms.unwrap_or(DEFAULT_WAIT_MS));
    let waited = async {
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = app.stopping.told() => {}
        }
    };
    let finished = tokio::select! {
        finished = &mut working => finished,
        () = waited => {
            // Not done in time, or the server is stopping and waits for
            // nobody. The first run the work started is the one
            // that stands for it; until there is one, there is nothing to
            // hand back.
            tokio::select! {
                finished = &mut working => finished,
                Some(run) = announced.recv() => {
                    return run_response(app, StatusCode::ACCEPTED, Some(run)).await;
                }
            }
        }
    };
    let run =
        finished.map_err(|_| ApiError::internal("the engine stopped while it was working"))??;
    run_response(app, StatusCode::OK, run).await
}

async fn run_response(app: &App, status: StatusCode, run: Option<RunId>) -> ApiResult {
    let run = match run {
        Some(run) => Some(engine(app, move |engine| engine.run(run)).await?),
        None => None,
    };
    Ok((status, Json(RunResponse { run })).into_response())
}

fn ok<T: Serialize>(value: T) -> ApiResult {
    Ok(Json(value).into_response())
}

// Handlers

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn info(State(app): State<App>) -> ApiResult {
    let store = engine(&app, Engine::info).await?;
    ok(Info {
        api: api::VERSION.to_owned(),
        store,
        task_timeout_ms: app
            .engine
            .user_code()
            .map(|user_code| user_code.timeout.as_millis() as u64),
    })
}

async fn load(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    params: Result<Query<LoadParams>, QueryRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    if is_arrow(&headers) {
        let params = query(params)?;
        let rows = arrow_rows(body)?;
        return job(&app, params.wait_ms, move |engine| {
            let report = stored(
                engine,
                caller.actor,
                &params.source,
                &params.entity_type,
                rows,
                params.replace,
            )?;
            Ok(Some(report.run))
        })
        .await;
    }
    if let Some(delimiter) = delimiter(&headers) {
        let params = query(params)?;
        let rows = delimited_rows(&app.reading, body, delimiter).await?;
        return job(&app, params.wait_ms, move |engine| {
            let report = stored(
                engine,
                caller.actor,
                &params.source,
                &params.entity_type,
                rows,
                params.replace,
            )?;
            Ok(Some(report.run))
        })
        .await;
    }
    let request: LoadRequest = json_body(&body)?;
    let rows = Rows::from_json(&request.rows)?;
    job(&app, request.wait_ms, move |engine| {
        let report = stored(
            engine,
            caller.actor,
            &request.source,
            &request.entity_type,
            rows,
            request.replace,
        )?;
        Ok(Some(report.run))
    })
    .await
}

/// Loads rows as the call asked: a load replaces a row the store holds
/// unless the call said `replace` is false.
fn stored(
    engine: &Engine,
    by: Actor,
    source: &str,
    entity_type: &str,
    rows: impl RecordBatchReader,
    replace: Option<bool>,
) -> spool::Result<spool::LoadReport> {
    let engine = engine.acting(by);
    match replace {
        Some(false) => engine.load_without_replacing(source, entity_type, rows),
        Some(true) | None => engine.load(source, entity_type, rows),
    }
}

async fn retract(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let request: RetractRequest = json_body(&body)?;
    job(&app, request.wait_ms, move |engine| {
        let report = engine.retract(
            &request.source,
            &request.entity_type,
            &request.keys,
            caller.actor,
            request.reason,
        )?;
        Ok(Some(report.run))
    })
    .await
}

/// What the query string of `POST /v1/documents` says about a document
/// that is sent as text.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TextDocumentParams {
    source: String,
    key: String,
    uri: Option<String>,
    wait_ms: Option<u64>,
}

/// `POST /v1/documents` as it arrives over HTTP: documents as JSON, or one
/// document as the text it is, named in the query string.
///
/// The query string decides, and not the content type alone: `curl` sends a
/// file as a form unless it is told otherwise, and a browser sends JSON as
/// `text/plain` unless it is told otherwise, and each means what it would
/// mean with the header it left out.
async fn post_documents(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    RawQuery(named): RawQuery,
    params: Result<Query<TextDocumentParams>, QueryRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    const NAMED: &str = "in the query string, as in `/v1/documents?source=notes&key=call.txt`";
    let named = named.is_some_and(|named| !named.is_empty());
    if !named || is_json(&headers) {
        return add_documents(State(app), Extension(caller), body)
            .await
            .map_err(|mut refused| {
                if !named && is_text(&headers) {
                    refused.body.message = format!(
                        "{}. To send one document as text, name it {NAMED}",
                        refused.body.message
                    );
                }
                refused
            });
    }
    // Nothing has to be put into JSON: the body is the file.
    let params = query(params).map_err(|refused| {
        ApiError::invalid(format!(
            "a document sent as text is named {NAMED}: {}",
            refused.body.message
        ))
    })?;
    let text = String::from_utf8(body.to_vec()).map_err(|_| {
        ApiError::invalid(
            "the body is not UTF-8 text: a document is stored as text, so turn a PDF or a web \
             page into text first",
        )
    })?;
    let request = DocumentsRequest {
        source: params.source,
        documents: vec![spool::NewDocument {
            key: params.key,
            uri: params.uri,
            text,
        }],
        wait_ms: params.wait_ms,
    };
    store_documents(&app, caller, request).await
}

async fn add_documents(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    store_documents(&app, caller, json_body(&body)?).await
}

async fn store_documents(app: &App, caller: Caller, request: DocumentsRequest) -> ApiResult {
    job(app, request.wait_ms, move |engine| {
        let report = engine
            .acting(caller.actor)
            .add_documents(&request.source, &request.documents)?;
        Ok(Some(report.run))
    })
    .await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentsQuery {
    source: Option<String>,
    key: Option<String>,
    latest: Option<bool>,
    after: Option<String>,
    limit: Option<u64>,
    sort: Option<DocumentSort>,
    order: Option<Direction>,
}

async fn list_documents(
    State(app): State<App>,
    params: Result<Query<DocumentsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let filter = DocumentFilter {
        source: params.source,
        key: params.key,
        latest: params.latest.unwrap_or(false),
        after: params
            .after
            .as_deref()
            .map(|after| id(after, "a document ID"))
            .transpose()?,
        limit,
        sort: sorted(params.sort, params.order, DocumentSort::Id),
    };
    let documents = engine(&app, move |engine| engine.documents(&filter)).await?;
    let next = (documents.len() as u64 == limit)
        .then(|| documents.last().map(|document| document.id))
        .flatten();
    ok(DocumentList { documents, next })
}

async fn read_document(State(app): State<App>, Path(document): Path<String>) -> ApiResult {
    let document: DocumentId = id(&document, "a document ID")?;
    ok(engine(&app, move |engine| engine.document(document)).await?)
}

async fn extract(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let request: ExtractRequest = optional_json_body(&body)?;
    let extractor = match request.extractor {
        Some(extractor) => extractor,
        // A project with one extractor need not say which.
        None => {
            let manifest = app.engine.manifest();
            let names: Vec<&str> = manifest
                .as_deref()
                .map(|manifest| {
                    manifest
                        .extractors
                        .iter()
                        .map(|extractor| extractor.name.as_str())
                        .collect()
                })
                .unwrap_or_default();
            match names[..] {
                [only] => only.to_owned(),
                [] => {
                    return Err(ApiError::invalid(
                        "the project has no extractors: one is a file under extractors/, and \
                         `spool init <dir> --example companies` makes a project with a sample",
                    ));
                }
                _ => {
                    return Err(ApiError::invalid(format!(
                        "the project has the extractors {}: say which in `extractor`",
                        listed(&names)
                    )));
                }
            }
        }
    };
    let again = request.again.unwrap_or(false);
    let options = ExtractOptions {
        extractor,
        source: request.source,
        again,
        since: request.since,
        limit: request.limit,
    };
    job(&app, request.wait_ms, move |engine| {
        let extracted = engine.acting(caller.actor).extract(&options);
        print_extracted(&options.extractor, &extracted);
        Ok(Some(extracted?.run))
    })
    .await
}

/// Prints, where the server was started, what an extractor wrote to
/// standard error, or why its run failed, so that whoever runs `spool dev`
/// sees it too. Output that nobody reads is no reason to stop.
fn print_extracted(extractor: &str, extracted: &spool::Result<spool::ExtractReport>) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    match extracted {
        Ok(report) if !report.stderr.is_empty() => {
            let _ = writeln!(out, "extractor `{extractor}`, run {}:", report.run);
            for line in &report.stderr {
                let _ = writeln!(out, "          {line}");
            }
        }
        Ok(_) => {}
        Err(error) => {
            let _ = writeln!(out, "extractor `{extractor}`: {error}");
        }
    }
    let _ = out.flush();
}

async fn start_match(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let request: MatchRequest = json_body(&body)?;
    let entity_type = request.entity_type;
    let rematch = request.rematch.unwrap_or(false);
    // A name is the project's strategy if the manifest the server was built
    // with has one by that name. Nothing a caller sends is ever run.
    let built_in = |strategy: &StrategyRef| -> ApiResult<bool> {
        if strategy.name == StrategyRef::RULES {
            return Ok(true);
        }
        let manifest = app.engine.manifest();
        let found = manifest
            .as_deref()
            .and_then(|manifest| manifest.entity_type(entity_type.as_str()))
            .and_then(|entry| entry.strategy(&strategy.name));
        match (found, &strategy.version) {
            (Some(found), Some(version)) if *version != found.hash => {
                Err(ApiError::invalid(format!(
                    "the project's strategy `{}` is version {}, not {version}",
                    found.name, found.hash
                )))
            }
            (found, _) => Ok(found.is_some()),
        }
    };
    match request.strategy {
        Some(strategy) if !built_in(&strategy)? => {
            if request.commit.is_some() {
                return Err(ApiError::invalid(
                    "`commit` goes with a strategy the server runs; a run that waits for judgements is told when it is reconciled",
                ));
            }
            job(&app, request.wait_ms, move |engine| {
                engine
                    .acting(caller.actor)
                    .begin_run(
                        &strategy,
                        &Scope {
                            entity_type,
                            rematch,
                        },
                    )
                    .map(Some)
            })
            .await
        }
        strategy => {
            let options = ResolveOptions {
                entity_type: Some(entity_type),
                leave_for_review: !request.commit.unwrap_or(true),
                strategy: strategy.map(|strategy| strategy.name),
                rematch,
            };
            job(&app, request.wait_ms, move |engine| {
                let report = engine.acting(caller.actor).resolve(&options)?;
                Ok(report.resolved.first().map(|resolved| resolved.match_run))
            })
            .await
        }
    }
}

async fn recompute(State(app): State<App>, body: Bytes) -> ApiResult {
    let request: api::RecomputeRequest = json_body(&body)?;
    let request = spool::RecomputeRequest {
        entity_type: request.entity_type,
        identity: request.identity,
        limit: request
            .limit
            .unwrap_or(spool::DEFAULT_CHANGES)
            .clamp(1, MAX_PAGE),
        offset: request.offset.unwrap_or(0),
    };
    ok(engine(&app, move |engine| engine.recompute(&request)).await?)
}

async fn apply_recompute(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let request: api::ApplyRecomputeRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.acting(caller.actor).apply_recompute(
            request.entity_type.as_ref(),
            &request.plan,
            request.reason,
        )
    })
    .await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RunsQuery {
    kind: Option<RunKind>,
    status: Option<RunStatus>,
    entity_type: Option<EntityType>,
    limit: Option<u64>,
    sort: Option<RunSort>,
    order: Option<Direction>,
}

async fn list_runs(
    State(app): State<App>,
    params: Result<Query<RunsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let sort = sorted(params.sort, params.order, RunSort::Run);
    let names = match sort {
        Some(sort) if sort.by == RunSort::StartedBy => names(&app).await?,
        _ => Vec::new(),
    };
    let filter = RunFilter {
        kind: params.kind,
        status: params.status,
        entity_type: params.entity_type,
        limit: page(params.limit),
        sort,
        names,
    };
    let runs = engine(&app, move |engine| engine.runs(&filter)).await?;
    ok(RunList { runs })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitQuery {
    wait_ms: Option<u64>,
}

/// Reads a run. With `wait_ms`, answers when the engine has stopped working
/// on the run or the time is up, whichever is first.
async fn read_run(
    State(app): State<App>,
    Path(run): Path<String>,
    params: Result<Query<WaitQuery>, QueryRejection>,
) -> ApiResult {
    let run = run_id(&run)?;
    let wait = Duration::from_millis(query(params)?.wait_ms.unwrap_or(0));
    let deadline = Instant::now() + wait;
    let mut pause = Duration::from_millis(20);
    let stopping = app.stopping.told();
    tokio::pin!(stopping);
    let mut told = false;
    loop {
        let record = engine(&app, move |engine| engine.run(run)).await?;
        let left = deadline.saturating_duration_since(Instant::now());
        // A server that is stopping waits for nobody.
        if record.status != RunStatus::Running || left.is_zero() || told {
            return ok(RunResponse { run: Some(record) });
        }
        tokio::select! {
            () = tokio::time::sleep(pause.min(left)) => {}
            () = &mut stopping => told = true,
        }
        pause = (pause * 2).min(Duration::from_millis(500));
    }
}

async fn candidates(
    State(app): State<App>,
    Path(run): Path<String>,
    headers: HeaderMap,
) -> ApiResult {
    let run = run_id(&run)?;
    let rows = engine(&app, move |engine| engine.candidate_rows(run)).await?;
    let as_json = media_type(&headers, ACCEPT).is_some_and(|accept| accept.contains("json"));
    let encoded = tokio::task::spawn_blocking(move || {
        if as_json {
            candidates_as_json(rows)
        } else {
            candidates_as_arrow(rows)
        }
    })
    .await
    .map_err(|_| ApiError::internal("the candidates could not be encoded"))?;
    encoded.map_err(|error| {
        ApiError::internal(format!("the candidates could not be encoded: {error}"))
    })
}

fn candidates_as_arrow(rows: Rows) -> Result<Response, arrow::error::ArrowError> {
    let mut writer = StreamWriter::try_new(Vec::new(), &rows.schema())?;
    for batch in rows {
        writer.write(&batch?)?;
    }
    let body = writer.into_inner()?;
    Ok(([(CONTENT_TYPE, ARROW_STREAM)], body).into_response())
}

fn candidates_as_json(rows: Rows) -> Result<Response, arrow::error::ArrowError> {
    let mut writer = arrow::json::ArrayWriter::new(Vec::new());
    for batch in rows {
        writer.write(&batch?)?;
    }
    writer.finish()?;
    let encoded = writer.into_inner();
    // No batches leaves the writer with nothing written, not an empty array.
    let candidates = if encoded.is_empty() {
        Vec::new()
    } else {
        serde_json::from_slice(&encoded)
            .map_err(|error| arrow::error::ArrowError::JsonError(error.to_string()))?
    };
    Ok(Json(CandidateList { candidates }).into_response())
}

async fn submit_judgements(
    State(app): State<App>,
    Path(run): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let run = run_id(&run)?;
    let stored = if is_arrow(&headers) {
        let rows = arrow_rows(body)?;
        engine(&app, move |engine| engine.submit_judgements(run, rows)).await?
    } else {
        let request: JudgementsRequest = json_body(&body)?;
        let rows: Vec<serde_json::Map<String, serde_json::Value>> = request
            .judgements
            .into_iter()
            .map(|judgement| {
                let mut row = serde_json::Map::new();
                row.insert("left_id".to_owned(), judgement.left_id.to_string().into());
                row.insert("right_id".to_owned(), judgement.right_id.to_string().into());
                row.insert("verdict".to_owned(), judgement.verdict.as_str().into());
                if let Some(score) = judgement.score {
                    row.insert("score".to_owned(), score.into());
                }
                if let Some(detail) = judgement.detail.filter(|detail| !detail.is_null()) {
                    row.insert("detail".to_owned(), detail.to_string().into());
                }
                row
            })
            .collect();
        // No judgements is still a call on the run, which has to be open.
        let rows = if rows.is_empty() {
            Rows::from_text(&["left_id", "right_id", "verdict"], &[])?
        } else {
            Rows::from_json(&rows)?
        };
        engine(&app, move |engine| engine.submit_judgements(run, rows)).await?
    };
    ok(JudgementsResponse { stored })
}

async fn reconcile(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(run): Path<String>,
    body: Bytes,
) -> ApiResult {
    let run = run_id(&run)?;
    let request: ReconcileRequest = optional_json_body(&body)?;
    let then = if request.commit.unwrap_or(true) {
        Disposition::Commit
    } else {
        Disposition::LeaveForReview
    };
    let report = engine(&app, move |engine| {
        engine
            .acting(caller.actor)
            .reconcile_with(run, &Policy::default(), then)
    })
    .await?;
    ok(report)
}

async fn cancel_run(State(app): State<App>, Path(run): Path<String>) -> ApiResult {
    let run = run_id(&run)?;
    let record = engine(&app, move |engine| {
        engine.cancel_run(run)?;
        engine.run(run)
    })
    .await?;
    ok(RunResponse { run: Some(record) })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftsQuery {
    status: Option<DraftStatus>,
    entity_type: Option<EntityType>,
}

async fn list_drafts(
    State(app): State<App>,
    params: Result<Query<DraftsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = DraftFilter {
        status: params.status,
        entity_type: params.entity_type,
    };
    let drafts = engine(&app, move |engine| engine.drafts(&filter)).await?;
    ok(DraftList { drafts })
}

async fn read_draft(State(app): State<App>, Path(draft): Path<String>) -> ApiResult {
    let draft = id(&draft, "a draft ID")?;
    ok(engine(&app, move |engine| engine.draft(draft)).await?)
}

async fn approve_draft(State(app): State<App>, Path(draft): Path<String>) -> ApiResult {
    let draft = id(&draft, "a draft ID")?;
    ok(engine(&app, move |engine| engine.approve_draft(draft)).await?)
}

async fn reject_draft(State(app): State<App>, Path(draft): Path<String>, body: Bytes) -> ApiResult {
    let draft = id(&draft, "a draft ID")?;
    let request: RejectDraftRequest = optional_json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.reject_draft(draft, request.reason.as_deref())
    })
    .await?)
}

async fn commit(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let request: CommitRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        let id = match request.id {
            Some(id) => id,
            None => engine.mint_ids(0, 1).changesets[0],
        };
        let base = match request.base {
            Some(base) => base,
            None => engine.head()?,
        };
        engine.commit_with_versions(Changeset {
            id,
            base,
            actor: caller.actor,
            reason: request.reason,
            run: request.run,
            ops: request.ops,
        })
    })
    .await?)
}

async fn undo(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(seq): Path<String>,
    body: Bytes,
) -> ApiResult {
    let seq = Seq(id(&seq, "a log position")?);
    let request: UndoRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.undo(
            seq,
            caller.actor,
            request.reason,
            request.commit.unwrap_or(true),
        )
    })
    .await?)
}

async fn split(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(identity): Path<String>,
    body: Bytes,
) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    let request: SplitRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.split(
            identity,
            &request.records,
            request.keep_apart.unwrap_or(true),
            request.base,
            caller.actor,
            request.reason,
        )
    })
    .await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ConstraintsQuery {
    record: Option<String>,
    identity: Option<String>,
    removed: Option<bool>,
}

async fn list_constraints(
    State(app): State<App>,
    params: Result<Query<ConstraintsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = ConstraintFilter {
        record: params
            .record
            .as_deref()
            .map(|record| id(record, "a record ID"))
            .transpose()?,
        identity: params
            .identity
            .as_deref()
            .map(|identity| id(identity, "an identity ID"))
            .transpose()?,
        include_removed: params.removed.unwrap_or(false),
    };
    let constraints = engine(&app, move |engine| engine.constraints(&filter)).await?;
    ok(ConstraintList { constraints })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LogQuery {
    after: Option<u64>,
    limit: Option<u64>,
    sort: Option<LogSort>,
    order: Option<Direction>,
}

async fn list_changesets(
    State(app): State<App>,
    params: Result<Query<LogQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let sort = sorted(params.sort, params.order, LogSort::Seq);
    let names = match sort {
        Some(sort) if sort.by == LogSort::Actor => names(&app).await?,
        _ => Vec::new(),
    };
    ok(engine(&app, move |engine| {
        // Sorted, a page continues after the changeset at `after`,
        // wherever that stands in the order.
        let changesets = match sort {
            None => engine.logged_after(Seq(params.after.unwrap_or(0)), limit)?,
            Some(sort) => engine.logged_sorted(sort, params.after.map(Seq), limit, &names)?,
        };
        Ok(ChangesetList {
            changesets,
            head: engine.head()?,
        })
    })
    .await?)
}

async fn mint_ids(State(app): State<App>, body: Bytes) -> ApiResult {
    let request: MintRequest = optional_json_body(&body)?;
    if request.identities > MAX_MINTED || request.changesets > MAX_MINTED {
        return Err(ApiError::invalid(format!(
            "at most {MAX_MINTED} IDs of a kind are minted in one call"
        )));
    }
    ok(engine(&app, move |engine| {
        Ok(engine.mint_ids(request.identities as usize, request.changesets as usize))
    })
    .await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentitiesQuery {
    entity_type: Option<EntityType>,
    q: Option<String>,
    /// Identity IDs with commas between them.
    ids: Option<String>,
    after: Option<String>,
    limit: Option<u64>,
    sort: Option<IdentitySort>,
    order: Option<Direction>,
}

async fn list_identities(
    State(app): State<App>,
    params: Result<Query<IdentitiesQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let ids = params
        .ids
        .as_deref()
        .map(|ids| {
            let ids: Vec<&str> = ids.split(',').filter(|id| !id.is_empty()).collect();
            if ids.len() as u64 > MAX_PAGE {
                return Err(ApiError::invalid(format!(
                    "at most {MAX_PAGE} identities are read by their IDs in one call"
                )));
            }
            ids.into_iter()
                .map(|identity| id(identity, "an identity ID"))
                .collect::<ApiResult<Vec<_>>>()
        })
        .transpose()?;
    let filter = IdentityFilter {
        entity_type: params.entity_type,
        q: params.q,
        ids,
        after: params
            .after
            .as_deref()
            .map(|after| id(after, "an identity ID"))
            .transpose()?,
        limit,
        sort: sorted(params.sort, params.order, IdentitySort::Id),
    };
    let identities = engine(&app, move |engine| engine.identities(&filter)).await?;
    let next = (identities.len() as u64 == limit)
        .then(|| identities.last().map(|identity| identity.id))
        .flatten();
    ok(IdentityList { identities, next })
}

async fn read_identity(State(app): State<App>, Path(identity): Path<String>) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    ok(engine(&app, move |engine| engine.identity(identity)).await?)
}

/// What holds an identity together, and the fewest links that would part
/// it. An identity that was merged away is explained as the one it was
/// merged into.
async fn explain_identity(State(app): State<App>, Path(identity): Path<String>) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    ok(engine(&app, move |engine| engine.explain_identity(identity)).await?)
}

async fn identity_history(State(app): State<App>, Path(identity): Path<String>) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    let history = engine(&app, move |engine| engine.history(identity)).await?;
    ok(History { history })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordsQuery {
    entity_type: Option<EntityType>,
    source: Option<String>,
    key: Option<String>,
    document: Option<String>,
    identity: Option<String>,
    q: Option<String>,
    unassigned: Option<bool>,
    latest: Option<bool>,
    after: Option<String>,
    limit: Option<u64>,
    sort: Option<RecordSort>,
    order: Option<Direction>,
}

async fn list_records(
    State(app): State<App>,
    params: Result<Query<RecordsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let filter = RecordFilter {
        entity_type: params.entity_type,
        source: params.source,
        key: params.key,
        document: params
            .document
            .as_deref()
            .map(|document| id(document, "a document ID"))
            .transpose()?,
        identity: params
            .identity
            .as_deref()
            .map(|identity| id(identity, "an identity ID"))
            .transpose()?,
        q: params.q,
        unassigned: params.unassigned.unwrap_or(false),
        latest: params.latest.unwrap_or(false),
        after: params
            .after
            .as_deref()
            .map(|after| id(after, "a record ID"))
            .transpose()?,
        limit,
        sort: sorted(params.sort, params.order, RecordSort::Source),
    };
    let records = engine(&app, move |engine| engine.records(&filter)).await?;
    let next = (records.len() as u64 == limit)
        .then(|| records.last().map(|record| record.id))
        .flatten();
    ok(RecordList { records, next })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AttributesQuery {
    entity_type: Option<EntityType>,
    source: Option<String>,
}

/// What the attributes of the records that stand hold, for each entity
/// type and source.
async fn list_attributes(
    State(app): State<App>,
    params: Result<Query<AttributesQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = AttributeFilter {
        entity_type: params.entity_type,
        source: params.source,
    };
    let groups = engine(&app, move |engine| engine.attributes(&filter)).await?;
    ok(AttributeList { groups })
}

async fn read_record(State(app): State<App>, Path(record): Path<String>) -> ApiResult {
    let record = id(&record, "a record ID")?;
    match engine(&app, move |engine| engine.record(record)).await? {
        Some(record) => ok(record),
        None => Err(spool::Error::UnknownRecord(record).into()),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplainQuery {
    a: String,
    b: String,
}

/// Why two records share an identity, or do not. The order in which
/// the two are named does not matter. Each is named as [`named_record`]
/// reads it.
async fn explain(
    State(app): State<App>,
    params: Result<Query<ExplainQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let a = named_record(&app, &params.a).await?;
    let b = named_record(&app, &params.b).await?;
    ok(engine(&app, move |engine| engine.explain(a, b)).await?)
}

/// The record a caller names: by its ID, or as `<source>:<key>`, which
/// names the version of the row that nothing has replaced, or a mention as
/// the lists name it, `<source>:<document>[<start>-<end>]`. A source's name
/// has no colon in it, so the first one ends it, and a key may have any. A
/// name more than one record has is refused, with their IDs.
async fn named_record(app: &App, name: &str) -> ApiResult<RecordId> {
    if let Ok(record) = name.parse() {
        return Ok(record);
    }
    let Some((source, key)) = name.split_once(':') else {
        return Err(ApiError::invalid(format!(
            "`{name}` does not name a record: give its ID, or `<source>:<key>`"
        )));
    };
    let (source, key) = (source.to_owned(), key.to_owned());
    let found = engine(app, {
        let (name, source, key) = (name.to_owned(), source.clone(), key.clone());
        move |engine| {
            let rows = current_versions(engine, &source, &key)?;
            match rows.is_empty() {
                true => mentions(engine, &name, &source, &key),
                false => Ok(rows),
            }
        }
    })
    .await?;
    match found.as_slice() {
        [record] => Ok(record.id),
        [] => {
            // The first part is often not a source at all, such as the
            // name of the entity type. The sources there are say what to
            // give.
            let sources: Vec<String> = app
                .engine
                .manifest()
                .iter()
                .flat_map(|manifest| &manifest.sources)
                .map(|declared| format!("`{}`", declared.name))
                .collect();
            let known = sources.is_empty() || sources.contains(&format!("`{source}`"));
            let hint = match known {
                true => String::new(),
                false => format!(": the project's sources are {}", sources.join(", ")),
            };
            Err(ApiError::not_found(format!(
                "there is no record with key `{key}` from source `{source}`{hint}"
            )))
        }
        several => {
            let ids: Vec<String> = several
                .iter()
                .map(|record| format!("{} (a `{}`)", record.id, record.entity_type))
                .collect();
            Err(ApiError::invalid(format!(
                "{} records are named `{name}`: name one by its ID, of {}",
                several.len(),
                ids.join(", ")
            )))
        }
    }
}

/// The versions of a source's row that nothing has replaced. A source that
/// sends rows of more than one entity type may hold a row of each under
/// one key, so there can be more than one.
fn current_versions(
    engine: &Engine,
    source: &str,
    key: &str,
) -> spool::Result<Vec<spool::RecordEntry>> {
    let mut filter = RecordFilter {
        source: Some(source.to_owned()),
        key: Some(key.to_owned()),
        limit: MAX_PAGE,
        ..RecordFilter::default()
    };
    let mut current = Vec::new();
    loop {
        let page = engine.records(&filter)?;
        let full = page.len() as u64 == filter.limit;
        filter.after = page.last().map(|version| version.id);
        current.extend(
            page.into_iter()
                .filter(|version| version.superseded_by.is_none()),
        );
        if !full {
            return Ok(current);
        }
    }
}

/// The records found at a place in a document, named as
/// [`spool::RecordEntry::name`] names a mention. Of the versions of the
/// document, they are those of the latest that has such a record. A
/// mention found again with other values is a version of the same record
/// at the same place, and only the one no other replaced is named.
fn mentions(
    engine: &Engine,
    name: &str,
    source: &str,
    place: &str,
) -> spool::Result<Vec<spool::RecordEntry>> {
    let Some((document, span)) = place
        .strip_suffix(']')
        .and_then(|place| place.rsplit_once('['))
    else {
        return Ok(Vec::new());
    };
    let is_span = span
        .split_once('-')
        .is_some_and(|(start, end)| start.parse::<u64>().is_ok() && end.parse::<u64>().is_ok());
    if !is_span {
        return Ok(Vec::new());
    }
    let mut versions = HashMap::new();
    let mut filter = DocumentFilter {
        source: Some(source.to_owned()),
        key: Some(document.to_owned()),
        limit: MAX_PAGE,
        ..DocumentFilter::default()
    };
    loop {
        let page = engine.documents(&filter)?;
        let full = page.len() as u64 == filter.limit;
        filter.after = page.last().map(|version| version.id);
        versions.extend(page.into_iter().map(|version| (version.id, version)));
        if !full {
            break;
        }
    }
    // The newest version is the one nothing replaced, and then the one
    // each replaced.
    let mut next = versions
        .values()
        .find(|version| version.superseded_by.is_none())
        .map(|version| version.id);
    while let Some(version) = next.and_then(|id| versions.remove(&id)) {
        next = version.supersedes;
        let mut found = Vec::new();
        let mut filter = RecordFilter {
            document: Some(version.id),
            limit: MAX_PAGE,
            ..RecordFilter::default()
        };
        loop {
            let page = engine.records(&filter)?;
            let full = page.len() as u64 == filter.limit;
            filter.after = page.last().map(|record| record.id);
            found.extend(page.into_iter().filter(|record| record.name() == name));
            if !full {
                break;
            }
        }
        let ids: Vec<RecordId> = found.iter().map(|record| record.id).collect();
        found.retain(|record| {
            record
                .superseded_by
                .is_none_or(|later| !ids.contains(&later))
        });
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Ok(Vec::new())
}

async fn identities_of(State(app): State<App>, body: Bytes) -> ApiResult {
    let request: IdentitiesOfRequest = json_body(&body)?;
    let identities = engine(&app, move |engine| engine.identities_of(&request.records)).await?;
    ok(IdentitiesOfResponse { identities })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewQuery {
    status: Option<ReviewStatus>,
    entity_type: Option<EntityType>,
    record: Option<String>,
    after: Option<u64>,
    /// Without it, every item: the review screen lists them all.
    limit: Option<u64>,
}

async fn list_review(
    State(app): State<App>,
    params: Result<Query<ReviewQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = ReviewFilter {
        status: params.status,
        entity_type: params.entity_type,
        id: None,
        record: params
            .record
            .as_deref()
            .map(|record| id(record, "a record ID"))
            .transpose()?,
        after: params.after,
        limit: params.limit.map(|limit| page(Some(limit))),
    };
    let limit = filter.limit;
    let items = engine(&app, move |engine| engine.review_items(&filter)).await?;
    let next = limit
        .filter(|limit| items.len() as u64 == *limit)
        .and_then(|_| items.last().map(|item| item.id));
    ok(ReviewList { items, next })
}

async fn read_review(State(app): State<App>, Path(item): Path<String>) -> ApiResult {
    let item: u64 = id(&item, "a review item ID")?;
    let filter = ReviewFilter {
        id: Some(item),
        ..ReviewFilter::default()
    };
    match engine(&app, move |engine| engine.review_items(&filter))
        .await?
        .pop()
    {
        Some(record) => ok(record),
        None => Err(spool::Error::UnknownReviewItem(item).into()),
    }
}

/// A review item with the records a reviewer reads to decide it.
async fn review_case(State(app): State<App>, Path(item): Path<String>) -> ApiResult {
    let item: u64 = id(&item, "a review item ID")?;
    ok(engine(&app, move |engine| engine.review_case(item)).await?)
}

async fn dismiss_review(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(item): Path<String>,
    body: Bytes,
) -> ApiResult {
    let item: u64 = id(&item, "a review item ID")?;
    let request: DismissRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.dismiss_review(item, &caller.actor, request.reason.as_deref())
    })
    .await?)
}

#[cfg(test)]
mod tests {
    use super::{delimited_rows, same};
    use axum::body::Bytes;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn a_body_of_delimited_text_waits_its_turn_to_be_read() {
        let reading = Arc::new(Semaphore::new(1));
        let body = || Bytes::from_static(b"id,name\n1,Acme\n");
        let taken = Arc::clone(&reading).acquire_owned().await.unwrap();
        let waiting = tokio::spawn({
            let reading = Arc::clone(&reading);
            async move { delimited_rows(&reading, body(), b',').await.is_ok() }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!waiting.is_finished(), "it was read out of turn");
        drop(taken);
        assert!(waiting.await.unwrap());
        // A body that cannot be read gives its turn back too.
        assert!(delimited_rows(&reading, Bytes::new(), b',').await.is_err());
        assert_eq!(reading.available_permits(), 1);
    }

    #[test]
    fn codes_are_compared_whole() {
        assert!(same(b"secret", b"secret"));
        assert!(same(b"", b""));
        for (a, b) in [
            (&b"secret"[..], &b"secreT"[..]),
            (b"secret", b"secret "),
            (b"secret", b"secre"),
            (b"", b"x"),
            (b"a\0", b"a"),
        ] {
            assert!(!same(a, b), "{a:?} and {b:?}");
            assert!(!same(b, a), "{b:?} and {a:?}");
        }
    }
}
