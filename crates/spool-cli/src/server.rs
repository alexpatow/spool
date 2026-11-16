//! The HTTP API: a thin wrapper around one [`Engine`].
//!
//! The server is the only process that has the store open. Every path under
//! `/v1` needs the bearer token. Small values are JSON; candidates, judgements
//! and rows to ingest can also cross as Arrow IPC streams.
//!
//! The engine does its work on blocking threads. A call that only reads
//! answers from committed state and does not wait for a run in progress.
//! Calls that write take their turn: the engine has one writer.
//!
//! # Calls that start work
//!
//! `POST /v1/ingest` and `POST /v1/runs` say how long they wait with
//! `wait_ms`. If the work is done by then, the response is `200` with the
//! finished run. Otherwise it is `202` with the run as far as it has got, and
//! the caller reads `GET /v1/runs/{run}` until its status is no longer
//! `running`. The work goes on whether or not anyone is waiting.
//!
//! A call that finds the writer busy has no run to report until its own work
//! starts, so it can wait longer than `wait_ms` for that.

use std::future::Future;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatchReader;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde::de::DeserializeOwned;
use spool::core::{Changeset, EntityType, Policy, RunId, Seq};
use spool::{
    ConstraintFilter, Disposition, DraftFilter, DraftStatus, Engine, IdentityFilter, IngestOptions,
    ObservationFilter, ResolveOptions, ReviewFilter, ReviewStatus, Rows, RunFilter, RunKind,
    RunStatus, Scope, StrategyRef, watch_runs,
};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::api::{
    self, ARROW_STREAM, CandidateList, ChangesetList, CommitRequest, ConstraintList,
    DismissRequest, DraftList, ErrorBody, ErrorCode, ErrorResponse, History, IdentitiesOfRequest,
    IdentitiesOfResponse, IdentityList, Info, IngestParams, IngestRequest, JudgementsRequest,
    JudgementsResponse, MintRequest, ObservationList, ReconcileRequest, RejectDraftRequest,
    ReviewList, RunList, RunResponse, SplitRequest, StartRunRequest, UndoRequest,
};

/// How long a call that starts work waits when it does not say.
pub const DEFAULT_WAIT_MS: u64 = 30_000;

/// The largest request body the server reads by default.
pub const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

/// The most entries a list returns in one page.
const MAX_PAGE: u64 = 1_000;
const DEFAULT_PAGE: u64 = 100;

/// The most IDs minted in one call.
const MAX_MINTED: u32 = 10_000;

#[derive(Clone, Debug)]
pub struct ServerOptions {
    /// The bearer token every call under `/v1` must carry.
    pub token: String,
    pub max_body_bytes: usize,
}

impl ServerOptions {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }
}

#[derive(Clone)]
struct App {
    engine: Arc<Engine>,
    token: Arc<str>,
}

/// The API over `engine`.
pub fn router(engine: Arc<Engine>, options: &ServerOptions) -> Router {
    let app = App {
        engine,
        token: Arc::from(options.token.as_str()),
    };
    let v1 = Router::new()
        .route("/info", get(info))
        .route("/ingest", post(ingest))
        .route("/runs", get(list_runs).post(start_run))
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
        .route("/identities/{identity}/split", post(split))
        .route("/constraints", get(list_constraints))
        .route("/observations", get(list_observations))
        .route("/observations/identities", post(identities_of))
        .route("/observations/{observation}", get(read_observation))
        .route("/review", get(list_review))
        .route("/review/{item}", get(read_review))
        .route("/review/{item}/dismiss", post(dismiss_review))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(app.clone(), authorize));
    Router::new()
        .route("/healthz", get(health))
        .nest(&format!("/{}", api::VERSION), v1)
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(options.max_body_bytes))
        .layer(middleware::map_response(describe_failure))
        .with_state(app)
}

/// Serves the API on `listener` until `shutdown` completes. Requests in
/// flight are answered first.
pub async fn serve(
    listener: TcpListener,
    engine: Arc<Engine>,
    options: &ServerOptions,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router(engine, options))
        .with_graceful_shutdown(shutdown)
        .await
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
            | Error::UnknownChangeset(_)
            | Error::UnknownReviewItem(_) => Self::not_found(message),
            Error::Run { .. }
            | Error::Draft { .. }
            | Error::Review { .. }
            | Error::Split { .. }
            | Error::NothingToUndo(_) => {
                Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, message)
            }
            Error::Ingest(_)
            | Error::Judgements(_)
            | Error::Rows(_)
            | Error::Arrow(_)
            | Error::InvalidEntityType(_)
            | Error::UndeclaredType(_)
            | Error::UnknownStrategy { .. } => Self::invalid(message),
            Error::Strategy { .. } => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::StrategyFailed,
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

// Authorisation

async fn authorize(State(app): State<App>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, token) = value.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
        });
    match presented {
        Some(token) if same(token.as_bytes(), app.token.as_bytes()) => next.run(request).await,
        _ => ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "this server needs its bearer token: `Authorization: Bearer <token>`",
        )
        .into_response(),
    }
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

fn arrow_rows(body: Bytes) -> ApiResult<StreamReader<Cursor<Bytes>>> {
    StreamReader::try_new(Cursor::new(body), None)
        .map_err(|error| ApiError::invalid(format!("the body is not an Arrow IPC stream: {error}")))
}

fn page(limit: Option<u64>) -> u64 {
    limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
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
    let finished = tokio::select! {
        finished = &mut working => finished,
        () = tokio::time::sleep(wait) => {
            // Not done in time. The first run the work started is the one
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
    })
}

async fn ingest(
    State(app): State<App>,
    params: Result<Query<IngestParams>, QueryRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    fn options(key: Option<String>, evidence: Option<String>) -> IngestOptions {
        IngestOptions {
            key_column: key,
            evidence_column: evidence,
        }
    }
    if is_arrow(&headers) {
        let params = query(params)?;
        let rows = arrow_rows(body)?;
        let options = options(params.key, params.evidence);
        return job(&app, params.wait_ms, move |engine| {
            let report = engine.ingest(&params.source, &params.entity_type, rows, &options)?;
            Ok(Some(report.run))
        })
        .await;
    }
    let request: IngestRequest = json_body(&body)?;
    let rows = Rows::from_json(&request.rows)?;
    let options = options(request.key, request.evidence);
    job(&app, request.wait_ms, move |engine| {
        let report = engine.ingest(&request.source, &request.entity_type, rows, &options)?;
        Ok(Some(report.run))
    })
    .await
}

async fn start_run(State(app): State<App>, body: Bytes) -> ApiResult {
    let request: StartRunRequest = json_body(&body)?;
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
                let report = engine.resolve(&options)?;
                Ok(report.resolved.first().map(|resolved| resolved.match_run))
            })
            .await
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RunsQuery {
    kind: Option<RunKind>,
    status: Option<RunStatus>,
    entity_type: Option<EntityType>,
    limit: Option<u64>,
}

async fn list_runs(
    State(app): State<App>,
    params: Result<Query<RunsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = RunFilter {
        kind: params.kind,
        status: params.status,
        entity_type: params.entity_type,
        limit: page(params.limit),
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
    loop {
        let record = engine(&app, move |engine| engine.run(run)).await?;
        let left = deadline.saturating_duration_since(Instant::now());
        if record.status != RunStatus::Running || left.is_zero() {
            return ok(RunResponse { run: Some(record) });
        }
        tokio::time::sleep(pause.min(left)).await;
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

async fn reconcile(State(app): State<App>, Path(run): Path<String>, body: Bytes) -> ApiResult {
    let run = run_id(&run)?;
    let request: ReconcileRequest = optional_json_body(&body)?;
    let then = if request.commit.unwrap_or(true) {
        Disposition::Commit
    } else {
        Disposition::LeaveForReview
    };
    let report = engine(&app, move |engine| {
        engine.reconcile_with(run, &Policy::default(), then)
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

async fn commit(State(app): State<App>, body: Bytes) -> ApiResult {
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
        engine.commit(Changeset {
            id,
            base,
            actor: request.actor,
            reason: request.reason,
            run: request.run,
            ops: request.ops,
        })
    })
    .await?)
}

async fn undo(State(app): State<App>, Path(seq): Path<String>, body: Bytes) -> ApiResult {
    let seq = Seq(id(&seq, "a log position")?);
    let request: UndoRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.undo(
            seq,
            request.actor,
            request.reason,
            request.commit.unwrap_or(true),
        )
    })
    .await?)
}

async fn split(State(app): State<App>, Path(identity): Path<String>, body: Bytes) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    let request: SplitRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.split(
            identity,
            &request.observations,
            request.keep_apart.unwrap_or(true),
            request.actor,
            request.reason,
        )
    })
    .await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ConstraintsQuery {
    observation: Option<String>,
    identity: Option<String>,
    removed: Option<bool>,
}

async fn list_constraints(
    State(app): State<App>,
    params: Result<Query<ConstraintsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = ConstraintFilter {
        observation: params
            .observation
            .as_deref()
            .map(|observation| id(observation, "an observation ID"))
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
}

async fn list_changesets(
    State(app): State<App>,
    params: Result<Query<LogQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let after = Seq(params.after.unwrap_or(0));
    let limit = page(params.limit);
    ok(engine(&app, move |engine| {
        Ok(ChangesetList {
            changesets: engine.log_after(after, limit)?,
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
    after: Option<String>,
    limit: Option<u64>,
}

async fn list_identities(
    State(app): State<App>,
    params: Result<Query<IdentitiesQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let filter = IdentityFilter {
        entity_type: params.entity_type,
        after: params
            .after
            .as_deref()
            .map(|after| id(after, "an identity ID"))
            .transpose()?,
        limit,
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

async fn identity_history(State(app): State<App>, Path(identity): Path<String>) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    let history = engine(&app, move |engine| engine.history(identity)).await?;
    ok(History { history })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationsQuery {
    entity_type: Option<EntityType>,
    source: Option<String>,
    key: Option<String>,
    unassigned: Option<bool>,
    after: Option<String>,
    limit: Option<u64>,
}

async fn list_observations(
    State(app): State<App>,
    params: Result<Query<ObservationsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let filter = ObservationFilter {
        entity_type: params.entity_type,
        source: params.source,
        key: params.key,
        unassigned: params.unassigned.unwrap_or(false),
        after: params
            .after
            .as_deref()
            .map(|after| id(after, "an observation ID"))
            .transpose()?,
        limit,
    };
    let observations = engine(&app, move |engine| engine.observations(&filter)).await?;
    let next = (observations.len() as u64 == limit)
        .then(|| observations.last().map(|observation| observation.id))
        .flatten();
    ok(ObservationList { observations, next })
}

async fn read_observation(State(app): State<App>, Path(observation): Path<String>) -> ApiResult {
    let observation = id(&observation, "an observation ID")?;
    match engine(&app, move |engine| engine.observation(observation)).await? {
        Some(record) => ok(record),
        None => Err(ApiError::not_found(format!(
            "there is no observation {observation}"
        ))),
    }
}

async fn identities_of(State(app): State<App>, body: Bytes) -> ApiResult {
    let request: IdentitiesOfRequest = json_body(&body)?;
    let identities = engine(&app, move |engine| {
        engine.identities_of(&request.observations)
    })
    .await?;
    ok(IdentitiesOfResponse { identities })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewQuery {
    status: Option<ReviewStatus>,
    entity_type: Option<EntityType>,
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
    };
    let items = engine(&app, move |engine| engine.review_items(&filter)).await?;
    ok(ReviewList { items })
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

async fn dismiss_review(
    State(app): State<App>,
    Path(item): Path<String>,
    body: Bytes,
) -> ApiResult {
    let item: u64 = id(&item, "a review item ID")?;
    let request: DismissRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.dismiss_review(item, &request.actor, request.reason.as_deref())
    })
    .await?)
}

#[cfg(test)]
mod tests {
    use super::same;

    #[test]
    fn tokens_are_compared_whole() {
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
