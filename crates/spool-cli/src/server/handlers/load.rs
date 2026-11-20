//! Rows in and out: loading rows of a source, adding one record, and
//! retracting rows.

use arrow::record_batch::RecordBatchReader;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Query, State};
use axum::http::HeaderMap;
use spool::{AddRecordReport, Rows};

use crate::api::{AddRecordRequest, LoadParams, LoadRequest, RetractRequest};
use crate::server::App;
use crate::server::auth::Caller;
use crate::server::error::ApiResult;
use crate::server::request::{arrow_rows, delimited_rows, delimiter, is_arrow, json_body, query};
use crate::server::work::{Started, engine, job, ok};

impl App {
    /// Loads rows that came as JSON.
    pub(in crate::server) async fn load_rows(
        &self,
        caller: Caller,
        request: LoadRequest,
    ) -> ApiResult<Started> {
        let rows = Rows::from_json(&request.rows)?;
        let params = LoadParams {
            source: request.source,
            entity_type: request.entity_type,
            replace: request.replace,
            wait_ms: request.wait_ms,
        };
        store_rows(self, caller, params, rows).await
    }

    pub(in crate::server) async fn add_record(
        &self,
        caller: Caller,
        request: AddRecordRequest,
    ) -> ApiResult<AddRecordReport> {
        engine(self, move |engine| {
            engine.acting(caller.actor).add_record(
                &request.source,
                &request.entity_type,
                &request.row,
                request.replace.unwrap_or(false),
            )
        })
        .await
    }

    pub(in crate::server) async fn retract_rows(
        &self,
        caller: Caller,
        request: RetractRequest,
    ) -> ApiResult<Started> {
        job(self, request.wait_ms, move |engine| {
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
}

/// Loads rows as the call asked: a load replaces a row the store holds
/// unless the call said `replace` is false.
async fn store_rows(
    app: &App,
    caller: Caller,
    params: LoadParams,
    rows: impl RecordBatchReader + Send + 'static,
) -> ApiResult<Started> {
    job(app, params.wait_ms, move |engine| {
        let engine = engine.acting(caller.actor);
        let report = match params.replace {
            Some(false) => engine.load_without_replacing(&params.source, &params.entity_type, rows),
            Some(true) | None => engine.load(&params.source, &params.entity_type, rows),
        }?;
        Ok(Some(report.run))
    })
    .await
}

/// `POST /v1/load` as it arrives over HTTP: rows as an Arrow stream or as
/// delimited text, named in the query string, or as JSON.
pub(in crate::server) async fn load(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    params: Result<Query<LoadParams>, QueryRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Started> {
    if is_arrow(&headers) {
        let params = query(params)?;
        let rows = arrow_rows(body)?;
        return store_rows(&app, caller, params, rows).await;
    }
    if let Some(delimiter) = delimiter(&headers) {
        let params = query(params)?;
        let rows = delimited_rows(&app.reading, body, delimiter).await?;
        return store_rows(&app, caller, params, rows).await;
    }
    app.load_rows(caller, json_body(&body)?).await
}

pub(in crate::server) async fn add_record(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    ok(app.add_record(caller, json_body(&body)?).await?)
}

pub(in crate::server) async fn retract(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult<Started> {
    app.retract_rows(caller, json_body(&body)?).await
}
