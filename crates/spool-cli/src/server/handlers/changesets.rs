//! Changesets: committing one, undoing one, splitting an identity, the
//! constraints that stand, the log, and IDs to commit with.

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Path, Query, State};
use spool::core::{Changeset, Committed, IdentityId, Seq};
use spool::{Authored, ConstraintFilter, Direction, LogSort, MintedIds, SplitReport};

use crate::api::{
    ChangesetList, CommitRequest, ConstraintList, MintRequest, SplitRequest, UndoRequest,
};
use crate::server::App;
use crate::server::auth::{Caller, list_actor_names};
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::{
    build_sort, clamp_page_size, json_body, optional_json_body, parse_id, parse_query,
};
use crate::server::work::{ok_json, run_engine_work};

/// The most IDs minted in one call.
const MAX_MINTED: u32 = 10_000;

impl App {
    pub(in crate::server) async fn commit_changeset(
        &self,
        caller: Caller,
        request: CommitRequest,
    ) -> ApiResult<Committed> {
        run_engine_work(self, move |engine| {
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
        .await
    }

    pub(in crate::server) async fn undo_changeset(
        &self,
        caller: Caller,
        seq: Seq,
        request: UndoRequest,
    ) -> ApiResult<Authored> {
        run_engine_work(self, move |engine| {
            engine.undo(
                seq,
                caller.actor,
                request.reason,
                request.commit.unwrap_or(true),
            )
        })
        .await
    }

    pub(in crate::server) async fn split_identity(
        &self,
        caller: Caller,
        identity: IdentityId,
        request: SplitRequest,
    ) -> ApiResult<SplitReport> {
        run_engine_work(self, move |engine| {
            engine.split(
                identity,
                &request.records,
                request.keep_apart.unwrap_or(true),
                request.base,
                caller.actor,
                request.reason,
            )
        })
        .await
    }

    pub(in crate::server) async fn list_constraints(
        &self,
        filter: ConstraintFilter,
    ) -> ApiResult<ConstraintList> {
        let constraints = run_engine_work(self, move |engine| engine.constraints(&filter)).await?;
        Ok(ConstraintList { constraints })
    }

    /// New IDs for a caller that writes its own changeset.
    pub(in crate::server) async fn mint_ids(&self, request: MintRequest) -> ApiResult<MintedIds> {
        if request.identities > MAX_MINTED || request.changesets > MAX_MINTED {
            return Err(ApiError::invalid(format!(
                "at most {MAX_MINTED} IDs of a kind are minted in one call"
            )));
        }
        run_engine_work(self, move |engine| {
            Ok(engine.mint_ids(request.identities as usize, request.changesets as usize))
        })
        .await
    }
}

pub(in crate::server) async fn commit(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    ok_json(app.commit_changeset(caller, json_body(&body)?).await?)
}

pub(in crate::server) async fn undo(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(seq): Path<String>,
    body: Bytes,
) -> ApiResult {
    let seq = Seq(parse_id(&seq, "a log position")?);
    ok_json(app.undo_changeset(caller, seq, json_body(&body)?).await?)
}

pub(in crate::server) async fn split(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    Path(identity): Path<String>,
    body: Bytes,
) -> ApiResult {
    let identity = parse_id(&identity, "an identity ID")?;
    let request = json_body(&body)?;
    ok_json(app.split_identity(caller, identity, request).await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct ConstraintsQuery {
    record: Option<String>,
    identity: Option<String>,
    removed: Option<bool>,
}

pub(in crate::server) async fn list_constraints(
    State(app): State<App>,
    params: Result<Query<ConstraintsQuery>, QueryRejection>,
) -> ApiResult {
    let params = parse_query(params)?;
    let filter = ConstraintFilter {
        record: params
            .record
            .as_deref()
            .map(|record| parse_id(record, "a record ID"))
            .transpose()?,
        identity: params
            .identity
            .as_deref()
            .map(|identity| parse_id(identity, "an identity ID"))
            .transpose()?,
        include_removed: params.removed.unwrap_or(false),
    };
    ok_json(app.list_constraints(filter).await?)
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
    let params = parse_query(params)?;
    let limit = clamp_page_size(params.limit);
    let sort = build_sort(params.sort, params.order, LogSort::Seq);
    let names = match sort {
        Some(sort) if sort.by == LogSort::Actor => list_actor_names(&app).await?,
        _ => Vec::new(),
    };
    ok_json(
        run_engine_work(&app, move |engine| {
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
        .await?,
    )
}

pub(in crate::server) async fn mint_ids(State(app): State<App>, body: Bytes) -> ApiResult {
    ok_json(app.mint_ids(optional_json_body(&body)?).await?)
}
