//! Identities: listing them, reading one as it is and as it was, and its
//! history.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use spool::core::{EntityType, IdentityId, Seq};
use spool::{Direction, IdentityFilter, IdentitySnapshot, IdentitySort, IdentityView};

use crate::api::{History, IdentityList};
use crate::server::App;
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::{MAX_PAGE, id, page, query, sorted};
use crate::server::work::{engine, ok};

impl App {
    /// A page of the identities a filter finds, `filter.limit` of them at
    /// most, and where the next page starts.
    pub(in crate::server) async fn list_identities(
        &self,
        filter: IdentityFilter,
    ) -> ApiResult<IdentityList> {
        if let Some(ids) = &filter.ids {
            refuse_more_ids_than_a_page(ids.len())?;
        }
        let limit = filter.limit;
        let identities = engine(self, move |engine| engine.identities(&filter)).await?;
        let next = (identities.len() as u64 == limit)
            .then(|| identities.last().map(|identity| identity.id))
            .flatten();
        Ok(IdentityList { identities, next })
    }

    pub(in crate::server) async fn read_identity(
        &self,
        identity: IdentityId,
    ) -> ApiResult<IdentityView> {
        engine(self, move |engine| engine.identity(identity)).await
    }

    /// What an identity held just after the changeset at `seq`.
    pub(in crate::server) async fn identity_at(
        &self,
        identity: IdentityId,
        seq: Seq,
    ) -> ApiResult<IdentitySnapshot> {
        engine(self, move |engine| engine.identity_at(identity, seq)).await
    }

    pub(in crate::server) async fn identity_history(
        &self,
        identity: IdentityId,
    ) -> ApiResult<History> {
        let history = engine(self, move |engine| engine.history(identity)).await?;
        Ok(History { history })
    }
}

/// Refuses a call that names more identities by their IDs than a page holds.
fn refuse_more_ids_than_a_page(ids: usize) -> ApiResult<()> {
    if ids as u64 > MAX_PAGE {
        return Err(ApiError::invalid(format!(
            "at most {MAX_PAGE} identities are read by their IDs in one call"
        )));
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct IdentitiesQuery {
    entity_type: Option<EntityType>,
    q: Option<String>,
    /// Identity IDs with commas between them.
    ids: Option<String>,
    after: Option<String>,
    limit: Option<u64>,
    sort: Option<IdentitySort>,
    order: Option<Direction>,
}

pub(in crate::server) async fn list_identities(
    State(app): State<App>,
    params: Result<Query<IdentitiesQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let ids = params
        .ids
        .as_deref()
        .map(|ids| {
            let ids: Vec<&str> = ids.split(',').filter(|id| !id.is_empty()).collect();
            // Too many are refused before any of them is read.
            refuse_more_ids_than_a_page(ids.len())?;
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
        limit: page(params.limit),
        sort: sorted(params.sort, params.order, IdentitySort::Id),
    };
    ok(app.list_identities(filter).await?)
}

pub(in crate::server) async fn read_identity(
    State(app): State<App>,
    Path(identity): Path<String>,
) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    ok(app.read_identity(identity).await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct IdentityAtQuery {
    seq: Seq,
}

pub(in crate::server) async fn identity_at(
    State(app): State<App>,
    Path(identity): Path<String>,
    params: Result<Query<IdentityAtQuery>, QueryRejection>,
) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    let seq = query(params)?.seq;
    ok(app.identity_at(identity, seq).await?)
}

pub(in crate::server) async fn identity_history(
    State(app): State<App>,
    Path(identity): Path<String>,
) -> ApiResult {
    let identity = id(&identity, "an identity ID")?;
    ok(app.identity_history(identity).await?)
}
