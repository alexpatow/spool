//! Documents: storing them, listing and reading them, and having an
//! extractor read them.

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Path, Query, RawQuery, State};
use axum::http::HeaderMap;
use spool::core::DocumentId;
use spool::{AddDocumentReport, Direction, DocumentFilter, DocumentSort, ExtractOptions};

use crate::api::{AddDocumentRequest, DocumentList, DocumentsRequest, ExtractRequest};
use crate::server::App;
use crate::server::auth::Caller;
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::{
    id, is_json, is_text, json_body, optional_json_body, page, query, sorted,
};
use crate::server::work::{Started, engine, job, ok};
use crate::text::listed;

impl App {
    /// Stores documents for an extractor to read.
    pub(in crate::server) async fn add_documents(
        &self,
        caller: Caller,
        request: DocumentsRequest,
    ) -> ApiResult<Started> {
        job(self, request.wait_ms, move |engine| {
            let report = engine
                .acting(caller.actor)
                .add_documents(&request.source, &request.documents)?;
            Ok(Some(report.run))
        })
        .await
    }

    /// Stores one document, has an extractor read it and matches what it
    /// found.
    pub(in crate::server) async fn add_document(
        &self,
        caller: Caller,
        request: AddDocumentRequest,
    ) -> ApiResult<AddDocumentReport> {
        engine(self, move |engine| {
            engine.acting(caller.actor).add_document(
                &request.source,
                &request.document,
                request.extractor.as_deref(),
            )
        })
        .await
    }

    /// Has one of the project's extractors read the documents it has not
    /// read.
    pub(in crate::server) async fn run_extraction(
        &self,
        caller: Caller,
        request: ExtractRequest,
    ) -> ApiResult<Started> {
        let extractor = match request.extractor {
            Some(extractor) => extractor,
            // A project with one extractor need not say which.
            None => {
                let manifest = self.engine.manifest();
                let names: Vec<&str> = manifest
                    .as_deref()
                    .map(|manifest| {
                        manifest
                            .extractors
                            .iter()
                            .map(|extractor| extractor.name.as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                match names[..] {
                    [only] => only.to_owned(),
                    [] => {
                        return Err(ApiError::invalid(
                            "the project has no extractors: one is a file under extractors/, and \
                             `spool init <dir> --example companies` makes a project with a sample",
                        ));
                    }
                    _ => {
                        return Err(ApiError::invalid(format!(
                            "the project has the extractors {}: say which in `extractor`",
                            listed(&names)
                        )));
                    }
                }
            }
        };
        let again = request.again.unwrap_or(false);
        let options = ExtractOptions {
            extractor,
            source: request.source,
            again,
            since: request.since,
            limit: request.limit,
        };
        job(self, request.wait_ms, move |engine| {
            let extracted = engine.acting(caller.actor).extract(&options);
            print_extracted(&options.extractor, &extracted);
            Ok(Some(extracted?.run))
        })
        .await
    }
}

/// What the query string of `POST /v1/documents` says about a document
/// that is sent as text.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct TextDocumentParams {
    source: String,
    key: String,
    uri: Option<String>,
    wait_ms: Option<u64>,
}

/// `POST /v1/documents` as it arrives over HTTP: documents as JSON, or one
/// document as the text it is, named in the query string.
///
/// The query string decides, and not the content type alone: `curl` sends a
/// file as a form unless it is told otherwise, and a browser sends JSON as
/// `text/plain` unless it is told otherwise, and each means what it would
/// mean with the header it left out.
pub(in crate::server) async fn post_documents(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    RawQuery(named): RawQuery,
    params: Result<Query<TextDocumentParams>, QueryRejection>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Started> {
    const NAMED: &str = "in the query string, as in `/v1/documents?source=notes&key=call.txt`";
    let named = named.is_some_and(|named| !named.is_empty());
    if !named || is_json(&headers) {
        return add_documents(State(app), Extension(caller), body)
            .await
            .map_err(|mut refused| {
                if !named && is_text(&headers) {
                    refused.body.message = format!(
                        "{}. To send one document as text, name it {NAMED}",
                        refused.body.message
                    );
                }
                refused
            });
    }
    // Nothing has to be put into JSON: the body is the file.
    let params = query(params).map_err(|refused| {
        ApiError::invalid(format!(
            "a document sent as text is named {NAMED}: {}",
            refused.body.message
        ))
    })?;
    let text = String::from_utf8(body.to_vec()).map_err(|_| {
        ApiError::invalid(
            "the body is not UTF-8 text: a document is stored as text, so turn a PDF or a web \
             page into text first",
        )
    })?;
    let request = DocumentsRequest {
        source: params.source,
        documents: vec![spool::NewDocument {
            key: params.key,
            uri: params.uri,
            text,
        }],
        wait_ms: params.wait_ms,
    };
    app.add_documents(caller, request).await
}

pub(in crate::server) async fn add_documents(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult<Started> {
    app.add_documents(caller, json_body(&body)?).await
}

pub(in crate::server) async fn add_document(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult {
    ok(app.add_document(caller, json_body(&body)?).await?)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::server) struct DocumentsQuery {
    source: Option<String>,
    key: Option<String>,
    latest: Option<bool>,
    after: Option<String>,
    limit: Option<u64>,
    sort: Option<DocumentSort>,
    order: Option<Direction>,
}

pub(in crate::server) async fn list_documents(
    State(app): State<App>,
    params: Result<Query<DocumentsQuery>, QueryRejection>,
) -> ApiResult {
    let params = query(params)?;
    let limit = page(params.limit);
    let filter = DocumentFilter {
        source: params.source,
        key: params.key,
        latest: params.latest.unwrap_or(false),
        after: params
            .after
            .as_deref()
            .map(|after| id(after, "a document ID"))
            .transpose()?,
        limit,
        sort: sorted(params.sort, params.order, DocumentSort::Id),
    };
    let documents = engine(&app, move |engine| engine.documents(&filter)).await?;
    let next = (documents.len() as u64 == limit)
        .then(|| documents.last().map(|document| document.id))
        .flatten();
    ok(DocumentList { documents, next })
}

pub(in crate::server) async fn read_document(
    State(app): State<App>,
    Path(document): Path<String>,
) -> ApiResult {
    let document: DocumentId = id(&document, "a document ID")?;
    ok(engine(&app, move |engine| engine.document(document)).await?)
}

pub(in crate::server) async fn extract(
    State(app): State<App>,
    Extension(caller): Extension<Caller>,
    body: Bytes,
) -> ApiResult<Started> {
    app.run_extraction(caller, optional_json_body(&body)?).await
}

/// Prints, where the server was started, what an extractor wrote to
/// standard error, or why its run failed, so that whoever runs `spool dev`
/// sees it too. Output that nobody reads is no reason to stop.
fn print_extracted(extractor: &str, extracted: &spool::Result<spool::ExtractReport>) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    match extracted {
        Ok(report) if !report.stderr.is_empty() => {
            let _ = writeln!(out, "extractor `{extractor}`, run {}:", report.run);
            for line in &report.stderr {
                let _ = writeln!(out, "          {line}");
            }
        }
        Ok(_) => {}
        Err(error) => {
            let _ = writeln!(out, "extractor `{extractor}`: {error}");
        }
    }
    let _ = out.flush();
}
