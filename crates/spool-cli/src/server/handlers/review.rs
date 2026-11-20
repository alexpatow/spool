//! Review items: what the server could not decide by itself, the case for
//! each, and dismissing one.

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Path, Query, State};
use spool::core::EntityType;
use spool::{ReviewCase, ReviewEntry, ReviewFilter, ReviewStatus};

use crate::api::{DismissRequest, ReviewList};
use crate::server::App;
use crate::server::auth::Caller;
use crate::server::error::ApiResult;
use crate::server::request::{id, json_body, page, query};
use crate::server::work::{engine, ok};

impl App {
    /// The review items a filter finds: all of them, or with `filter.limit`
    /// a page of them and where the next page starts.
    pub(in crate::server) async fn list_review(
        &self,
        filter: ReviewFilter,
    ) -> ApiResult<ReviewList> {
        let limit = filter.limit;
        let items = engine(self, move |engine| engine.review_items(&filter)).await?;
        let next = limit
            .filter(|limit| items.len() as u64 == *limit)
            .and_then(|_| items.last().map(|item| item.id));
        Ok(ReviewList { items, next })
    }

    /// A review item with the records a reviewer reads to decide it.
    pub(in crate::server) async fn review_case(&self, item: u64) -> ApiResult<ReviewCase> {
        engine(self, move |engine| engine.review_case(item)).await
    }

    pub(in crate::server) async fn dismiss_review(
        &self,
        caller: Caller,
        item: u64,
        request: DismissRequest,
    ) -> ApiResult<ReviewEntry> {
        engine(self, move |engine| {
            engine.dismiss_review(item, &caller.actor, request.reason.as_deref())
        })
        .await
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ReviewQuery {
    status: Option<ReviewStatus>,
    entity_type: Option<EntityType>,
    record: Option<String>,
    after: Option<u64>,
    /// Without it, every item: the review screen lists them all.
    limit: Option<u64>,
}

pub(in crate::server) async fn list_review(
    State(app): State<App>,
    params: Result<Query<ReviewQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = ReviewFilter {
        status: params.status,
        entity_type: params.entity_type,
        id: None,
        record: params
            .record
            .as_deref()
            .map(|record| id(record, "a record ID"))
            .transpose()?,
        after: params.after,
        limit: params.limit.map(|limit| page(Some(limit))),
    };
    ok(app.list_review(filter).await?)
}

pub(in crate::server) async fn read_review(
    State(app): State<App>,
    Path(item): Path<String>,
) -> ApiResult {
    let item: u64 = id(&item, "a review item ID")?;
    let filter = ReviewFilter {
        id: Some(item),
        ..ReviewFilter::default()
    };
    match engine(&app, move |engine| engine.review_items(&filter))
        .await?
        .pop()
    {
        Some(record) => ok(record),
        None => Err(spool::Error::UnknownReviewItem(item).into()),
    }
}

pub(in crate::server) async fn review_case(
    State(app): State<App>,
    Path(item): Path<String>,
) -> ApiResult {
    let item: u64 = id(&item, "a review item ID")?;
    ok(app.review_case(item).await?)
}

pub(in crate::server) async fn dismiss_review(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(item): Path<String>,
    body: Bytes,
) -> ApiResult {
    let item: u64 = id(&item, "a review item ID")?;
    ok(app.dismiss_review(caller, item, json_body(&body)?).await?)
}
