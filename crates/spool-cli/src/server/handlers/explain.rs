//! Why records share an identity or do not: what holds an identity
//! together, and what is between two records.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};

use super::records::named_record;
use crate::server::App;
use crate::server::error::ApiResult;
use crate::server::request::{id, query};
use crate::server::work::{engine, ok};

/// What holds an identity together, and the fewest links that would part
/// it. An identity that was merged away is explained as the one it was
/// merged into.
pub(in crate::server) async fn explain_identity(
    State(app): State<App>,
    Path(identity): Path<String>,
) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    ok(engine(&app, move |engine| engine.explain_identity(identity)).await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ExplainQuery {
    pub(in crate::server) a: String,
    pub(in crate::server) b: String,
}

/// Why two records share an identity, or do not. The order in which
/// the two are named does not matter. Each is named as [`named_record`]
/// reads it.
pub(in crate::server) async fn explain(
    State(app): State<App>,
    params: Result<Query<ExplainQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let a = named_record(&app, &params.a).await?;
    let b = named_record(&app, &params.b).await?;
    ok(engine(&app, move |engine| engine.explain(a, b)).await?)
}
