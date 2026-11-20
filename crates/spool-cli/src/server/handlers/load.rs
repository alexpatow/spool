//! Rows in and out: loading rows of a source, adding one record, and
//! retracting rows.

use arrow::record_batch::RecordBatchReader;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Query, State};
use axum::http::HeaderMap;
use spool::core::Actor;
use spool::{Engine, Rows};

use crate::api::{AddRecordRequest, LoadParams, LoadRequest, RetractRequest};
use crate::server::App;
use crate::server::auth::Caller;
use crate::server::error::ApiResult;
use crate::server::request::{arrow_rows, delimited_rows, delimiter, is_arrow, json_body, query};
use crate::server::work::{engine, job, ok};

pub(in crate::server) async fn load(
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

pub(in crate::server) async fn add_record(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let request: AddRecordRequest = json_body(&body)?;
    let report = engine(&app, move |engine| {
        engine.acting(caller.actor).add_record(
            &request.source,
            &request.entity_type,
            &request.row,
            request.replace.unwrap_or(false),
        )
    })
    .await?;
    ok(report)
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

pub(in crate::server) async fn retract(
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
