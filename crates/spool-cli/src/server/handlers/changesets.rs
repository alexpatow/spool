//! Changesets: committing one, undoing one, splitting an identity, the
//! constraints that stand, the log, and IDs to commit with.

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Path, Query, State};
use spool::core::{Changeset, Seq};
use spool::{ConstraintFilter, Direction, LogSort};

use crate::api::{
    ChangesetList, CommitRequest, ConstraintList, MintRequest, SplitRequest, UndoRequest,
};
use crate::server::App;
use crate::server::auth::{Caller, names};
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::{id, json_body, optional_json_body, page, query, sorted};
use crate::server::work::{engine, ok};

/// The most IDs minted in one call.
const MAX_MINTED: u32 = 10_000;

pub(in crate::server) async fn commit(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    let request: CommitRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        let id = match request.id {
            Some(id) => id,
            None => engine.mint_ids(0, 1).changesets[0],
        };
        let base = match request.base {
            Some(base) => base,
            None => engine.head()?,
        };
        engine.commit_with_versions(Changeset {
            id,
            base,
            actor: caller.actor,
            reason: request.reason,
            run: request.run,
            ops: request.ops,
        })
    })
    .await?)
}

pub(in crate::server) async fn undo(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(seq): Path<String>,
    body: Bytes,
) -> ApiResult {
    let seq = Seq(id(&seq, "a log position")?);
    let request: UndoRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.undo(
            seq,
            caller.actor,
            request.reason,
            request.commit.unwrap_or(true),
        )
    })
    .await?)
}

pub(in crate::server) async fn split(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(identity): Path<String>,
    body: Bytes,
) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    let request: SplitRequest = json_body(&body)?;
    ok(engine(&app, move |engine| {
        engine.split(
            identity,
            &request.records,
            request.keep_apart.unwrap_or(true),
            request.base,
            caller.actor,
            request.reason,
        )
    })
    .await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ConstraintsQuery {
    pub(in crate::server) record: Option<String>,
    pub(in crate::server) identity: Option<String>,
    pub(in crate::server) removed: Option<bool>,
}

pub(in crate::server) async fn list_constraints(
    State(app): State<App>,
    params: Result<Query<ConstraintsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = ConstraintFilter {
        record: params
            .record
            .as_deref()
            .map(|record| id(record, "a record ID"))
            .transpose()?,
        identity: params
            .identity
            .as_deref()
            .map(|identity| id(identity, "an identity ID"))
            .transpose()?,
        include_removed: params.removed.unwrap_or(false),
    };
    let constraints = engine(&app, move |engine| engine.constraints(&filter)).await?;
    ok(ConstraintList { constraints })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct LogQuery {
    after: Option<u64>,
    limit: Option<u64>,
    sort: Option<LogSort>,
    order: Option<Direction>,
}

pub(in crate::server) async fn list_changesets(
    State(app): State<App>,
    params: Result<Query<LogQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let sort = sorted(params.sort, params.order, LogSort::Seq);
    let names = match sort {
        Some(sort) if sort.by == LogSort::Actor => names(&app).await?,
        _ => Vec::new(),
    };
    ok(engine(&app, move |engine| {
        // Sorted, a page continues after the changeset at `after`,
        // wherever that stands in the order.
        let changesets = match sort {
            None => engine.logged_after(Seq(params.after.unwrap_or(0)), limit)?,
            Some(sort) => engine.logged_sorted(sort, params.after.map(Seq), limit, &names)?,
        };
        Ok(ChangesetList {
            changesets,
            head: engine.head()?,
        })
    })
    .await?)
}

pub(in crate::server) async fn mint_ids(State(app): State<App>, body: Bytes) -> ApiResult {
    let request: MintRequest = optional_json_body(&body)?;
    if request.identities > MAX_MINTED || request.changesets > MAX_MINTED {
        return Err(ApiError::invalid(format!(
            "at most {MAX_MINTED} IDs of a kind are minted in one call"
        )));
    }
    ok(engine(&app, move |engine| {
        Ok(engine.mint_ids(request.identities as usize, request.changesets as usize))
    })
    .await?)
}
