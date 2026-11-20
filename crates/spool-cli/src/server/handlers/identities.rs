//! Identities: listing them, reading one as it is and as it was, and its
//! history.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use spool::core::{EntityType, Seq};
use spool::{Direction, IdentityFilter, IdentitySort};

use crate::api::{History, IdentityList};
use crate::server::App;
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::{MAX_PAGE, id, page, query, sorted};
use crate::server::work::{engine, ok};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct IdentitiesQuery {
    pub(in crate::server) entity_type: Option<EntityType>,
    pub(in crate::server) q: Option<String>,
    /// Identity IDs with commas between them.
    pub(in crate::server) ids: Option<String>,
    pub(in crate::server) after: Option<String>,
    pub(in crate::server) limit: Option<u64>,
    pub(in crate::server) sort: Option<IdentitySort>,
    pub(in crate::server) order: Option<Direction>,
}

pub(in crate::server) async fn list_identities(
    State(app): State<App>,
    params: Result<Query<IdentitiesQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let ids = params
        .ids
        .as_deref()
        .map(|ids| {
            let ids: Vec<&str> = ids.split(',').filter(|id| !id.is_empty()).collect();
            if ids.len() as u64 > MAX_PAGE {
                return Err(ApiError::invalid(format!(
                    "at most {MAX_PAGE} identities are read by their IDs in one call"
                )));
            }
            ids.into_iter()
                .map(|identity| id(identity, "an identity ID"))
                .collect::<ApiResult<Vec<_>>>()
        })
        .transpose()?;
    let filter = IdentityFilter {
        entity_type: params.entity_type,
        q: params.q,
        ids,
        after: params
            .after
            .as_deref()
            .map(|after| id(after, "an identity ID"))
            .transpose()?,
        limit,
        sort: sorted(params.sort, params.order, IdentitySort::Id),
    };
    let identities = engine(&app, move |engine| engine.identities(&filter)).await?;
    let next = (identities.len() as u64 == limit)
        .then(|| identities.last().map(|identity| identity.id))
        .flatten();
    ok(IdentityList { identities, next })
}

pub(in crate::server) async fn read_identity(
    State(app): State<App>,
    Path(identity): Path<String>,
) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    ok(engine(&app, move |engine| engine.identity(identity)).await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct IdentityAtQuery {
    pub(in crate::server) seq: Seq,
}

pub(in crate::server) async fn identity_at(
    State(app): State<App>,
    Path(identity): Path<String>,
    params: Result<Query<IdentityAtQuery>, QueryRejection>,
) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    let seq = query(params)?.seq;
    ok(engine(&app, move |engine| engine.identity_at(identity, seq)).await?)
}

pub(in crate::server) async fn identity_history(
    State(app): State<App>,
    Path(identity): Path<String>,
) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    let history = engine(&app, move |engine| engine.history(identity)).await?;
    ok(History { history })
}
