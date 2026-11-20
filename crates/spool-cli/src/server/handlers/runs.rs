//! Runs: starting a match, reading a run and its candidates, taking
//! judgements, reconciling and cancelling.

use std::time::{Duration, Instant};

use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatchReader;
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use spool::core::{EntityType, Policy, RunId};
use spool::{
    Direction, Disposition, ResolveOptions, Rows, RunFilter, RunKind, RunSort, RunStatus, Scope,
    StrategyRef,
};

use crate::api::{
    ARROW_STREAM, CandidateList, JudgementsRequest, JudgementsResponse, MatchRequest,
    ReconcileRequest, RunList, RunResponse,
};
use crate::server::App;
use crate::server::auth::{Caller, list_actor_names};
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::{
    arrow_rows, build_sort, clamp_page_size, is_arrow, json_body, media_type, optional_json_body,
    parse_query, run_id,
};
use crate::server::work::{Started, ok_json, run_engine_work, start_job};

impl App {
    /// Starts a match run of one entity type and waits for it.
    pub(in crate::server) async fn start_match(
        &self,
        caller: Caller,
        request: MatchRequest,
    ) -> ApiResult<Started> {
        let entity_type = request.entity_type;
        let rematch = request.rematch.unwrap_or(false);
        // A name is the project's strategy if the manifest the server was built
        // with has one by that name. Nothing a caller sends is ever run.
        let built_in = |strategy: &StrategyRef| -> ApiResult<bool> {
            if strategy.name == StrategyRef::RULES {
                return Ok(true);
            }
            let manifest = self.engine.manifest();
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
                start_job(self, request.wait_ms, move |engine| {
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
                start_job(self, request.wait_ms, move |engine| {
                    let report = engine.acting(caller.actor).resolve(&options)?;
                    Ok(report.resolved.first().map(|resolved| resolved.match_run))
                })
                .await
            }
        }
    }

    /// Reads a run. With `wait_ms`, answers when the engine has stopped
    /// working on the run or the time is up, whichever is first.
    pub(in crate::server) async fn read_run(
        &self,
        run: RunId,
        wait_ms: Option<u64>,
    ) -> ApiResult<RunResponse> {
        let wait = Duration::from_millis(wait_ms.unwrap_or(0));
        let deadline = Instant::now() + wait;
        let mut pause = Duration::from_millis(20);
        let stopping = self.stopping.told();
        tokio::pin!(stopping);
        let mut told = false;
        loop {
            let record = run_engine_work(self, move |engine| engine.run(run)).await?;
            let left = deadline.saturating_duration_since(Instant::now());
            // A server that is stopping waits for nobody.
            if record.status != RunStatus::Running || left.is_zero() || told {
                return Ok(RunResponse { run: Some(record) });
            }
            tokio::select! {
                () = tokio::time::sleep(pause.min(left)) => {}
                () = &mut stopping => told = true,
            }
            pause = (pause * 2).min(Duration::from_millis(500));
        }
    }
}

pub(in crate::server) async fn start_match(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult<Started> {
    app.start_match(caller, json_body(&body)?).await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct RunsQuery {
    kind: Option<RunKind>,
    status: Option<RunStatus>,
    entity_type: Option<EntityType>,
    limit: Option<u64>,
    sort: Option<RunSort>,
    order: Option<Direction>,
}

pub(in crate::server) async fn list_runs(
    State(app): State<App>,
    params: Result<Query<RunsQuery>, QueryRejection>,
) -> ApiResult {
    let params = parse_query(params)?;
    let sort = build_sort(params.sort, params.order, RunSort::Run);
    let names = match sort {
        Some(sort) if sort.by == RunSort::StartedBy => list_actor_names(&app).await?,
        _ => Vec::new(),
    };
    let filter = RunFilter {
        kind: params.kind,
        status: params.status,
        entity_type: params.entity_type,
        limit: clamp_page_size(params.limit),
        sort,
        names,
    };
    let runs = run_engine_work(&app, move |engine| engine.runs(&filter)).await?;
    ok_json(RunList { runs })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct WaitQuery {
    wait_ms: Option<u64>,
}

pub(in crate::server) async fn read_run(
    State(app): State<App>,
    Path(run): Path<String>,
    params: Result<Query<WaitQuery>, QueryRejection>,
) -> ApiResult {
    let run = run_id(&run)?;
    let wait_ms = parse_query(params)?.wait_ms;
    ok_json(app.read_run(run, wait_ms).await?)
}

pub(in crate::server) async fn candidates(
    State(app): State<App>,
    Path(run): Path<String>,
    headers: HeaderMap,
) -> ApiResult {
    let run = run_id(&run)?;
    let rows = run_engine_work(&app, move |engine| engine.candidate_rows(run)).await?;
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

pub(in crate::server) async fn submit_judgements(
    State(app): State<App>,
    Path(run): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let run = run_id(&run)?;
    let stored = if is_arrow(&headers) {
        let rows = arrow_rows(body)?;
        run_engine_work(&app, move |engine| engine.submit_judgements(run, rows)).await?
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
        run_engine_work(&app, move |engine| engine.submit_judgements(run, rows)).await?
    };
    ok_json(JudgementsResponse { stored })
}

pub(in crate::server) async fn reconcile(
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
    let report = run_engine_work(&app, move |engine| {
        engine
            .acting(caller.actor)
            .reconcile_with(run, &Policy::default(), then)
    })
    .await?;
    ok_json(report)
}

pub(in crate::server) async fn cancel_run(
    State(app): State<App>,
    Path(run): Path<String>,
) -> ApiResult {
    let run = run_id(&run)?;
    let record = run_engine_work(&app, move |engine| {
        engine.cancel_run(run)?;
        engine.run(run)
    })
    .await?;
    ok_json(RunResponse { run: Some(record) })
}
