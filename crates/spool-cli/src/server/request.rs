//! Reading requests: a body as JSON, as delimited text or as Arrow, a query
//! string, an ID in a path, and the page and the order a list is asked for.

use std::io::Cursor;
use std::sync::Arc;

use arrow::ipc::reader::StreamReader;
use axum::body::Bytes;
use axum::extract::Query;
use axum::extract::rejection::QueryRejection;
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use serde::de::DeserializeOwned;
use spool::core::RunId;
use spool::{Direction, Rows, Sort};
use tokio::sync::Semaphore;

use super::error::{ApiError, ApiResult};
use crate::api::{self, ARROW_STREAM};
use crate::delimited::{self, Malformed};

/// The most entries a list returns in one page.
pub(super) const MAX_PAGE: u64 = 1_000;
const DEFAULT_PAGE: u64 = 100;

pub(super) fn json_body<T: DeserializeOwned>(body: &Bytes) -> ApiResult<T> {
    serde_json::from_slice(body)
        .map_err(|error| ApiError::invalid(format!("the body is not the JSON expected: {error}")))
}

/// A body that may be left out, in which case every field has its default.
pub(super) fn optional_json_body<T: DeserializeOwned + Default>(body: &Bytes) -> ApiResult<T> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    json_body(body)
}

pub(super) fn query<T>(query: Result<Query<T>, QueryRejection>) -> ApiResult<T> {
    match query {
        Ok(Query(value)) => Ok(value),
        Err(rejection) => Err(ApiError::invalid(format!(
            "the query string is not what this path takes: {}",
            rejection.body_text()
        ))),
    }
}

pub(super) fn id<T>(text: &str, what: &str) -> ApiResult<T>
where
    T: std::str::FromStr,
{
    text.parse()
        .map_err(|_| ApiError::invalid(format!("`{text}` is not {what}")))
}

pub(super) fn run_id(text: &str) -> ApiResult<RunId> {
    Ok(RunId(id(text, "a run ID")?))
}

pub(super) fn is_text(headers: &HeaderMap) -> bool {
    media_type(headers, CONTENT_TYPE).is_some_and(|value| value.starts_with("text/"))
}

pub(super) fn is_json(headers: &HeaderMap) -> bool {
    media_type(headers, CONTENT_TYPE).is_some_and(|value| value.starts_with("application/json"))
}

pub(super) fn is_arrow(headers: &HeaderMap) -> bool {
    media_type(headers, CONTENT_TYPE).is_some_and(|value| value.starts_with(ARROW_STREAM))
}

pub(super) fn media_type(headers: &HeaderMap, name: axum::http::HeaderName) -> Option<String> {
    Some(
        headers
            .get(name)?
            .to_str()
            .ok()?
            .trim()
            .to_ascii_lowercase(),
    )
}

/// The character between the fields of a body of delimited text, if the
/// body is one.
pub(super) fn delimiter(headers: &HeaderMap) -> Option<u8> {
    let sent = media_type(headers, CONTENT_TYPE)?;
    if sent.starts_with(api::CSV) {
        Some(b',')
    } else if sent.starts_with(api::TSV) {
        Some(b'\t')
    } else {
        None
    }
}

/// The rows of a body of delimited text. Reading them is work, so it is
/// done off the threads that answer requests, and by no more at once than
/// `reading` has room for: a body waits its turn, and the one being read
/// keeps its place until it is done, even if whoever sent it has gone.
pub(super) async fn delimited_rows(
    reading: &Arc<Semaphore>,
    body: Bytes,
    delimiter: u8,
) -> ApiResult<Rows> {
    /// As many as DuckDB hands on at a time.
    const ROWS_AT_A_TIME: usize = 2048;
    let stopped = || ApiError::internal("the server stopped while it was reading the body");
    let turn = Arc::clone(reading)
        .acquire_owned()
        .await
        .map_err(|_| stopped())?;
    let read = move || -> Result<Rows, Malformed> {
        let _turn = turn;
        let mut reader = delimited::Reader::new(body.as_ref(), delimiter)?;
        let mut batches = Vec::new();
        while let Some(batch) = reader.next_batch(ROWS_AT_A_TIME, usize::MAX)? {
            batches.push(batch);
        }
        Ok(Rows::new(reader.schema(), batches))
    };
    match tokio::task::spawn_blocking(read).await {
        Ok(Ok(rows)) => Ok(rows),
        Ok(Err(Malformed(why))) => Err(ApiError::invalid(format!(
            "the body cannot be read as {}: {why}",
            if delimiter == b'\t' {
                "tab-separated text"
            } else {
                "CSV"
            }
        ))),
        Err(_) => Err(stopped()),
    }
}

pub(super) fn arrow_rows(body: Bytes) -> ApiResult<StreamReader<Cursor<Bytes>>> {
    StreamReader::try_new(Cursor::new(body), None)
        .map_err(|error| ApiError::invalid(format!("the body is not an Arrow IPC stream: {error}")))
}

pub(super) fn page(limit: Option<u64>) -> u64 {
    limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)
}

/// The order a list was asked for: `sort` names what it is sorted by, and
/// `order` is `asc` or `desc`. `order` alone turns the list's own order,
/// which is by `own`. Neither leaves the list in its own order.
pub(super) fn sorted<By>(sort: Option<By>, order: Option<Direction>, own: By) -> Option<Sort<By>> {
    match (sort, order) {
        (None, None) => None,
        (by, direction) => Some(Sort {
            by: by.unwrap_or(own),
            direction: direction.unwrap_or_default(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::delimited_rows;
    use axum::body::Bytes;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn a_body_of_delimited_text_waits_its_turn_to_be_read() {
        let reading = Arc::new(Semaphore::new(1));
        let body = || Bytes::from_static(b"id,name\n1,Acme\n");
        let taken = Arc::clone(&reading).acquire_owned().await.unwrap();
        let waiting = tokio::spawn({
            let reading = Arc::clone(&reading);
            async move { delimited_rows(&reading, body(), b',').await.is_ok() }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!waiting.is_finished(), "it was read out of turn");
        drop(taken);
        assert!(waiting.await.unwrap());
        // A body that cannot be read gives its turn back too.
        assert!(delimited_rows(&reading, Bytes::new(), b',').await.is_err());
        assert_eq!(reading.available_permits(), 1);
    }
}
