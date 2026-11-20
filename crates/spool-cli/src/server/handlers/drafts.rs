//! Drafts: changesets that wait for someone to approve or reject them.

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use spool::core::EntityType;
use spool::{DraftFilter, DraftStatus};

use crate::api::{DraftList, RejectDraftRequest};
use crate::server::App;
use crate::server::error::ApiResult;
use crate::server::request::{id, optional_json_body, query};
use crate::server::work::{engine, ok};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct DraftsQuery {
    status: Option<DraftStatus>,
    entity_type: Option<EntityType>,
}

pub(in crate::server) async fn list_drafts(
    State(app): State<App>,
    params: Result<Query<DraftsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = DraftFilter {
        status: params.status,
        entity_type: params.entity_type,
    };
    let drafts = engine(&app, move |engine| engine.drafts(&filter)).await?;
    ok(DraftList { drafts })
}

pub(in crate::server) async fn read_draft(
    State(app): State<App>,
    Path(draft): Path<String>,
) -> ApiResult {
    let draft = id(&draft, "a draft ID")?;
    ok(engine(&app, move |engine| engine.draft(draft)).await?)
}

pub(in crate::server) async fn approve_draft(
    State(app): State<App>,
    Path(draft): Path<String>,
) -> ApiResult {
    let draft = id(&draft, "a draft ID")?;
    ok(engine(&app, move |engine| engine.approve_draft(draft)).await?)
}

pub(in crate::server) async fn reject_draft(
    State(app): State<App>,
    Path(draft): Path<String>,
    body: Bytes,
) -> ApiResult {
    let draft = id(&draft, "a draft ID")?;
    let request: RejectDraftRequest = optional_json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.reject_draft(draft, request.reason.as_deref())
    })
    .await?)
}
