//! Tools for documents: storing them, adding one, and having an extractor
//! read them.

use schemars::JsonSchema;
use serde::Deserialize;
use spool::NewDocument;

use super::{Declared, Effect, declare_tool};
use crate::api::{AddDocumentRequest, DocumentsRequest, ExtractRequest};
use crate::server::App;
use crate::server::error::{ApiError, ApiResult};
use crate::server::mcp::limits::truncate_to_field;

pub(super) fn add_document() -> Declared {
    declare_tool(
        "add_document",
        "Add a document and find its identities",
        Effect::Changes,
        "Store one text document and run a project extractor on it. Returns the document ID \
         and each mention's span, record ID and identity or review item. The only extractor \
         in a project can be left unnamed. New text under the same source and key creates \
         a new document version.",
        |app, caller, request: AddDocumentRequest| async move {
            app.add_document(caller, request).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LoadDocuments {
    /// The name of the source the documents come from. The records an
    /// extractor finds in them have the same source, so it has to be one
    /// the project declares: `describe_store` lists them in `sources`.
    source: String,
    /// The documents, such as
    /// `[{"key": "notes-1.txt", "text": "Call with Dana Reyes on ..."}]`.
    documents: Vec<Document>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Document {
    /// What tells this document from the others of its source, such as
    /// the name of its file, a URL or a message ID.
    key: String,
    /// The document, whole, as plain text. Turn a PDF or a web page into
    /// text first.
    text: String,
    /// Where the source keeps the document, if that is worth showing to
    /// someone who reads what was found in it.
    #[serde(default)]
    uri: Option<String>,
}

pub(super) fn load_documents() -> Declared {
    declare_tool(
        "load_documents",
        "Load documents",
        Effect::Changes,
        "Store documents from one source, as text, for an extractor to read. Send \
         each as it is in the file or message you were given: read it first, and never \
         make one up. A document is told from another by its `key`, such as the name \
         of its file. The same key with the same text adds nothing, so loading a \
         document twice is safe, and the same key with other text becomes the new \
         version of that document. Storing a document finds nothing in it: follow \
         this with `run_extraction`, and that with `run_match`. A stored document is \
         kept for good and nothing deletes it, so load only what the person asked \
         for. A call is 4 MiB at most, and a pipeline loads more with \
         `POST /v1/documents`. Returns the run that stored the documents, whose \
         `load` counts them.",
        |app, caller, asked: LoadDocuments| async move {
            refuse_undeclared_source(&app, &asked.source)?;
            let documents = asked.documents.into_iter().map(|document| NewDocument {
                key: document.key,
                uri: document.uri,
                text: document.text,
            });
            let request = DocumentsRequest {
                source: asked.source,
                documents: documents.collect(),
                wait_ms: None,
            };
            Ok(app.add_documents(caller, request).await?.run)
        },
    )
}

/// Refuses documents from a source the project does not declare. The
/// API stores them, and then refuses every record an extractor finds
/// in them, so they would stay in the store for good and give nothing.
fn refuse_undeclared_source(app: &App, source: &str) -> ApiResult<()> {
    let Some(manifest) = app.engine.manifest() else {
        return Ok(());
    };
    if manifest.source(source).is_some() {
        return Ok(());
    }
    let sources: Vec<String> = manifest
        .sources
        .iter()
        .map(|declared| format!("`{}`", declared.name))
        .collect();
    Err(ApiError::invalid(format!(
        "source `{}` is not declared: the project's sources are {}",
        truncate_to_field(source),
        sources.join(", ")
    )))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunExtraction {
    /// The name of one of the project's extractors, which `describe_store`
    /// lists in `extractors`. It may be left out when the project has only
    /// one.
    #[serde(default)]
    extractor: Option<String>,
    /// Read only the documents of this source.
    #[serde(default)]
    source: Option<String>,
    /// Read the documents this extractor has read before as well. False
    /// unless given.
    #[serde(default)]
    again: Option<bool>,
    /// Read at most this many documents, oldest first. Every document the
    /// extractor has not read, unless given.
    #[serde(default)]
    limit: Option<u64>,
}

pub(super) fn run_extraction() -> Declared {
    declare_tool(
        "run_extraction",
        "Run an extractor",
        Effect::Changes,
        "Run one of the project's extractors over the documents it has not read. \
         `extractor` may be left out when the project has only one. Each place where \
         a document names something becomes a record of the document's source, which \
         keeps the document and the characters it was found at as its evidence. The \
         server runs the project's own extractor and nothing a caller sends. An \
         extractor may ask a language model, which takes time and can cost money: \
         `limit` says how many documents it reads at most. Extraction does not match: \
         follow it with `run_match` for the entity type of the records. What was found \
         is kept, as loaded rows are, and `retract_rows` takes a record back by its \
         key. Returns the run, whose `extraction` counts the documents read and the \
         records found. If its `status` is still `running`, read it with `get_run`.",
        |app, caller, asked: RunExtraction| async move {
            let request = ExtractRequest {
                extractor: asked.extractor,
                source: asked.source,
                again: asked.again,
                since: None,
                limit: asked.limit,
                wait_ms: None,
            };
            Ok(app.run_extraction(caller, request).await?.run)
        },
    )
}
