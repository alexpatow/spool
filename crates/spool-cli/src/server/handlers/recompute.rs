//! What matching again would change, and making the change.

use axum::body::Bytes;
use axum::extract::{Extension, State};

use crate::api;
use crate::server::App;
use crate::server::auth::Caller;
use crate::server::error::ApiResult;
use crate::server::request::{MAX_PAGE, json_body};
use crate::server::work::{ok_json, run_engine_work};

pub(in crate::server) async fn recompute(State(app): State<App>, body: Bytes) -> ApiResult {
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
    ok_json(run_engine_work(&app, move |engine| engine.recompute(&request)).await?)
}

pub(in crate::server) async fn apply_recompute(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let request: api::ApplyRecomputeRequest = json_body(&body)?;
    ok_json(
        run_engine_work(&app, move |engine| {
            engine.acting(caller.actor).apply_recompute(
                request.entity_type.as_ref(),
                &request.plan,
                request.reason,
            )
        })
        .await?,
    )
}
