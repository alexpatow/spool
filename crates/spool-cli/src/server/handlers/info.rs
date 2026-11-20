//! Whether the server is up, and what its store holds.

use axum::Json;
use axum::extract::State;
use spool::Engine;

use crate::api::{self, Info};
use crate::server::App;
use crate::server::error::ApiResult;
use crate::server::work::{ok_json, run_engine_work};

pub(in crate::server) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

impl App {
    /// What the store holds, and the API this server speaks.
    pub(in crate::server) async fn describe_store(&self) -> ApiResult<Info> {
        let store = run_engine_work(self, Engine::info).await?;
        Ok(Info {
            api: api::VERSION.to_owned(),
            store,
            task_timeout_ms: self
                .engine
                .user_code()
                .map(|user_code| user_code.timeout.as_millis() as u64),
        })
    }
}

pub(in crate::server) async fn info(State(app): State<App>) -> ApiResult {
    ok_json(app.describe_store().await?)
}
