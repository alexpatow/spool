//! Records: listing and reading them, what their attributes hold, the
//! record a name stands for, and the identities records are in.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use spool::core::{EntityType, RecordId};
use spool::{AttributeFilter, Direction, DocumentFilter, Engine, RecordFilter, RecordSort};

use crate::api::{AttributeList, IdentitiesOfRequest, IdentitiesOfResponse, RecordList};
use crate::server::App;
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::{MAX_PAGE, id, json_body, page, query, sorted};
use crate::server::work::{engine, ok};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct RecordsQuery {
    pub(in crate::server) entity_type: Option<EntityType>,
    pub(in crate::server) source: Option<String>,
    pub(in crate::server) key: Option<String>,
    pub(in crate::server) document: Option<String>,
    pub(in crate::server) identity: Option<String>,
    pub(in crate::server) q: Option<String>,
    pub(in crate::server) unassigned: Option<bool>,
    pub(in crate::server) latest: Option<bool>,
    pub(in crate::server) after: Option<String>,
    pub(in crate::server) limit: Option<u64>,
    pub(in crate::server) sort: Option<RecordSort>,
    pub(in crate::server) order: Option<Direction>,
}

pub(in crate::server) async fn list_records(
    State(app): State<App>,
    params: Result<Query<RecordsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let filter = RecordFilter {
        entity_type: params.entity_type,
        source: params.source,
        key: params.key,
        document: params
            .document
            .as_deref()
            .map(|document| id(document, "a document ID"))
            .transpose()?,
        identity: params
            .identity
            .as_deref()
            .map(|identity| id(identity, "an identity ID"))
            .transpose()?,
        q: params.q,
        unassigned: params.unassigned.unwrap_or(false),
        latest: params.latest.unwrap_or(false),
        after: params
            .after
            .as_deref()
            .map(|after| id(after, "a record ID"))
            .transpose()?,
        limit,
        sort: sorted(params.sort, params.order, RecordSort::Source),
    };
    let records = engine(&app, move |engine| engine.records(&filter)).await?;
    let next = (records.len() as u64 == limit)
        .then(|| records.last().map(|record| record.id))
        .flatten();
    ok(RecordList { records, next })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct AttributesQuery {
    entity_type: Option<EntityType>,
    source: Option<String>,
}

/// What the attributes of the records that stand hold, for each entity
/// type and source.
pub(in crate::server) async fn list_attributes(
    State(app): State<App>,
    params: Result<Query<AttributesQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let filter = AttributeFilter {
        entity_type: params.entity_type,
        source: params.source,
    };
    let groups = engine(&app, move |engine| engine.attributes(&filter)).await?;
    ok(AttributeList { groups })
}

pub(in crate::server) async fn read_record(
    State(app): State<App>,
    Path(record): Path<String>,
) -> ApiResult {
    let record = id(&record, "a record ID")?;
    match engine(&app, move |engine| engine.record(record)).await? {
        Some(record) => ok(record),
        None => Err(spool::Error::UnknownRecord(record).into()),
    }
}

/// The record a caller names: by its ID, or as `<source>:<key>`, which
/// names the version of the row that nothing has replaced, or a mention as
/// the lists name it, `<source>:<document>[<start>-<end>]`. A source's name
/// has no colon in it, so the first one ends it, and a key may have any. A
/// name more than one record has is refused, with their IDs.
pub(super) async fn named_record(app: &App, name: &str) -> ApiResult<RecordId> {
    if let Ok(record) = name.parse() {
        return Ok(record);
    }
    let Some((source, key)) = name.split_once(':') else {
        return Err(ApiError::invalid(format!(
            "`{name}` does not name a record: give its ID, or `<source>:<key>`"
        )));
    };
    let (source, key) = (source.to_owned(), key.to_owned());
    let found = engine(app, {
        let (name, source, key) = (name.to_owned(), source.clone(), key.clone());
        move |engine| {
            let rows = current_versions(engine, &source, &key)?;
            match rows.is_empty() {
                true => mentions(engine, &name, &source, &key),
                false => Ok(rows),
            }
        }
    })
    .await?;
    match found.as_slice() {
        [record] => Ok(record.id),
        [] => {
            // The first part is often not a source at all, such as the
            // name of the entity type. The sources there are say what to
            // give.
            let sources: Vec<String> = app
                .engine
                .manifest()
                .iter()
                .flat_map(|manifest| &manifest.sources)
                .map(|declared| format!("`{}`", declared.name))
                .collect();
            let known = sources.is_empty() || sources.contains(&format!("`{source}`"));
            let hint = match known {
                true => String::new(),
                false => format!(": the project's sources are {}", sources.join(", ")),
            };
            Err(ApiError::not_found(format!(
                "there is no record with key `{key}` from source `{source}`{hint}"
            )))
        }
        several => {
            let ids: Vec<String> = several
                .iter()
                .map(|record| format!("{} (a `{}`)", record.id, record.entity_type))
                .collect();
            Err(ApiError::invalid(format!(
                "{} records are named `{name}`: name one by its ID, of {}",
                several.len(),
                ids.join(", ")
            )))
        }
    }
}

/// The versions of a source's row that nothing has replaced. A source that
/// sends rows of more than one entity type may hold a row of each under
/// one key, so there can be more than one.
fn current_versions(
    engine: &Engine,
    source: &str,
    key: &str,
) -> spool::Result<Vec<spool::RecordEntry>> {
    let mut filter = RecordFilter {
        source: Some(source.to_owned()),
        key: Some(key.to_owned()),
        limit: MAX_PAGE,
        ..RecordFilter::default()
    };
    let mut current = Vec::new();
    loop {
        let page = engine.records(&filter)?;
        let full = page.len() as u64 == filter.limit;
        filter.after = page.last().map(|version| version.id);
        current.extend(
            page.into_iter()
                .filter(|version| version.superseded_by.is_none()),
        );
        if !full {
            return Ok(current);
        }
    }
}

/// The records found at a place in a document, named as
/// [`spool::RecordEntry::name`] names a mention. Of the versions of the
/// document, they are those of the latest that has such a record. A
/// mention found again with other values is a version of the same record
/// at the same place, and only the one no other replaced is named.
fn mentions(
    engine: &Engine,
    name: &str,
    source: &str,
    place: &str,
) -> spool::Result<Vec<spool::RecordEntry>> {
    let Some((document, span)) = place
        .strip_suffix(']')
        .and_then(|place| place.rsplit_once('['))
    else {
        return Ok(Vec::new());
    };
    let is_span = span
        .split_once('-')
        .is_some_and(|(start, end)| start.parse::<u64>().is_ok() && end.parse::<u64>().is_ok());
    if !is_span {
        return Ok(Vec::new());
    }
    let mut versions = HashMap::new();
    let mut filter = DocumentFilter {
        source: Some(source.to_owned()),
        key: Some(document.to_owned()),
        limit: MAX_PAGE,
        ..DocumentFilter::default()
    };
    loop {
        let page = engine.documents(&filter)?;
        let full = page.len() as u64 == filter.limit;
        filter.after = page.last().map(|version| version.id);
        versions.extend(page.into_iter().map(|version| (version.id, version)));
        if !full {
            break;
        }
    }
    // The newest version is the one nothing replaced, and then the one
    // each replaced.
    let mut next = versions
        .values()
        .find(|version| version.superseded_by.is_none())
        .map(|version| version.id);
    while let Some(version) = next.and_then(|id| versions.remove(&id)) {
        next = version.supersedes;
        let mut found = Vec::new();
        let mut filter = RecordFilter {
            document: Some(version.id),
            limit: MAX_PAGE,
            ..RecordFilter::default()
        };
        loop {
            let page = engine.records(&filter)?;
            let full = page.len() as u64 == filter.limit;
            filter.after = page.last().map(|record| record.id);
            found.extend(page.into_iter().filter(|record| record.name() == name));
            if !full {
                break;
            }
        }
        let ids: Vec<RecordId> = found.iter().map(|record| record.id).collect();
        found.retain(|record| {
            record
                .superseded_by
                .is_none_or(|later| !ids.contains(&later))
        });
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Ok(Vec::new())
}

pub(in crate::server) async fn identities_of(State(app): State<App>, body: Bytes) -> ApiResult {
    let request: IdentitiesOfRequest = json_body(&body)?;
    let identities = engine(&app, move |engine| engine.identities_of(&request.records)).await?;
    ok(IdentitiesOfResponse { identities })
}
