//! Whether the server is up, and what its store holds.

use axum::Json;
use axum::extract::State;
use spool::Engine;

use crate::api::{self, Info};
use crate::server::App;
use crate::server::error::ApiResult;
use crate::server::work::{engine, ok};

pub(in crate::server) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

pub(in crate::server) async fn info(State(app): State<App>) -> ApiResult {
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
