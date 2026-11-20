//! Why records share an identity or do not: what holds an identity
//! together, and what is between two records.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use spool::Explanation;

use super::records::named_record;
use crate::server::App;
use crate::server::error::ApiResult;
use crate::server::request::{parse_id, parse_query};
use crate::server::work::{ok_json, run_engine_work};

/// What holds an identity together, and the fewest links that would part
/// it. An identity that was merged away is explained as the one it was
/// merged into.
pub(in crate::server) async fn explain_identity(
    State(app): State<App>,
    Path(identity): Path<String>,
) -> ApiResult {
    let identity = parse_id(&identity, "an identity ID")?;
    ok_json(run_engine_work(&app, move |engine| engine.explain_identity(identity)).await?)
}

impl App {
    /// Why two records share an identity, or do not. The order in which
    /// the two are named does not matter. Each is named as [`named_record`]
    /// reads it.
    pub(in crate::server) async fn explain_pair(&self, a: &str, b: &str) -> ApiResult<Explanation> {
        let a = named_record(self, a).await?;
        let b = named_record(self, b).await?;
        run_engine_work(self, move |engine| engine.explain(a, b)).await
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ExplainQuery {
    a: String,
    b: String,
}

pub(in crate::server) async fn explain(
    State(app): State<App>,
    params: Result<Query<ExplainQuery>, QueryRejection>,
) -> ApiResult {
    let params = parse_query(params)?;
    ok_json(app.explain_pair(&params.a, &params.b).await?)
}
