//! Calling the engine: work on a blocking thread, work that is recorded as
//! a run and waited for, and the answer when either is done.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
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

/// Starts work that is recorded as a run and waits for it for `wait_ms`.
/// `work` returns the run that stands for the work, or `None` if there was
/// nothing to do.
pub(super) async fn job(
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

pub(super) fn ok<T: Serialize>(value: T) -> ApiResult {
    Ok(Json(value).into_response())
}
