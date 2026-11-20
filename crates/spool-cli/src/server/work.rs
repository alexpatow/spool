//! Calling the engine: work on a blocking thread, work that is recorded as
//! a run and waited for, and the answer when either is done.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use spool::core::RunId;
use spool::{Engine, watch_runs};
use tokio::sync::mpsc;

use super::error::{ApiError, ApiResult};
use super::{App, DEFAULT_WAIT_MS};
use crate::api::RunResponse;

/// Runs engine work on a blocking thread.
pub(super) async fn engine<T: Send + 'static>(
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

/// What a call that starts work answers: the run that stands for the work,
/// and whether the work was done when the call stopped waiting for it.
pub(super) struct Started {
    /// `200` with the finished run, `202` with the run in progress.
    pub(super) status: StatusCode,
    pub(super) run: RunResponse,
}

impl IntoResponse for Started {
    fn into_response(self) -> Response {
        (self.status, Json(self.run)).into_response()
    }
}

/// Starts work that is recorded as a run and waits for it for `wait_ms`.
/// `work` returns the run that stands for the work, or `None` if there was
/// nothing to do.
pub(super) async fn job(
    app: &App,
    wait_ms: Option<u64>,
    work: impl FnOnce(&Engine) -> spool::Result<Option<RunId>> + Send + 'static,
) -> ApiResult<Started> {
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
                    return started(app, StatusCode::ACCEPTED, Some(run)).await;
                }
            }
        }
    };
    let run =
        finished.map_err(|_| ApiError::internal("the engine stopped while it was working"))??;
    started(app, StatusCode::OK, run).await
}

async fn started(app: &App, status: StatusCode, run: Option<RunId>) -> ApiResult<Started> {
    let run = match run {
        Some(run) => Some(engine(app, move |engine| engine.run(run)).await?),
        None => None,
    };
    Ok(Started {
        status,
        run: RunResponse { run },
    })
}

pub(super) fn ok<T: Serialize>(value: T) -> ApiResult {
    Ok(Json(value).into_response())
}
