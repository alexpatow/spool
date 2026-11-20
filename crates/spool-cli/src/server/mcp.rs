//! The MCP endpoint: what an agent needs of the API, as tools.
//!
//! `/mcp` speaks the Model Context Protocol over Streamable HTTP, so that an
//! assistant such as Claude or ChatGPT can be pointed at the server and look
//! things up in the store, and change identities, on someone's behalf. The
//! protocol and its transport are the `rmcp` crate's. What is here is the
//! list of tools and what each one does: `tools` declares them, and
//! `limits` is what one returns and how much of it.
//!
//! Tools run as the caller the request came from, and call the same
//! operations as the paths under `/v1` where they have one. What a tool
//! changes is recorded under the agent whose token the client sends or the
//! person signed in. Nothing in a tool's arguments says who is calling.
//!
//! The endpoint keeps nothing between requests. Each one is authorized on
//! its own, a tool call is answered in the response to it, and there is no
//! session to resume or to steal.

mod limits;
mod tools;

use std::sync::{Arc, LazyLock};

use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::http::header::{ALLOW, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Icon, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler};

use self::limits::truncate_to_field;
use super::auth::Caller;
use super::error::ApiError;
use super::{App, ServerOptions};
use crate::api::ErrorCode;

/// The largest request the endpoint reads. A request is a tool call, and the
/// largest of those is a changeset.
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// The most that is read of what the transport says when it refuses.
const MAX_REFUSAL_BYTES: usize = 4 * 1024;

/// The 48×48 PNG from `site/src/app/favicon.ico`, embedded so clients can
/// display it as soon as they discover the server.
static SERVER_ICON: LazyLock<Icon> = LazyLock::new(|| {
    let png = STANDARD.encode(include_bytes!("../../assets/mcp-icon.png"));
    Icon::new(format!("data:image/png;base64,{png}"))
        .with_mime_type("image/png")
        .with_sizes(vec!["48x48".to_owned()])
});

/// What a client is told about the server when it connects. A client may
/// show the model only the beginning, so the first paragraph stands alone.
const INSTRUCTIONS: &str = "\
Spool resolves records from many sources into identities: one identity for \
each real-world thing, holding every record that refers to it. Use these \
tools to load rows and documents and match what they hold, to find an \
identity and the records behind it, to see why two records are or are not \
the same thing, and to correct identities. Start with `describe_store`, \
which lists the entity types this store has and the sources each takes \
rows from.

Loading and matching: `add_record` loads one row and returns its record and identity, or a review item. \
`add_document` stores one text document, runs an extractor on it, and returns the identity or \
review item of each record it finds. \
`load_rows` stores rows from one source as records, \
and `run_match` then places the records no run has examined: each joins the \
identity it matches or gets a new one, and identities that exist keep their \
IDs. `describe_store` names each source the project declares in `sources`, \
with its columns and the entity types it sends rows of. Load what you were \
asked to load and nothing else; `retract_rows` takes a row back out of its \
identity and keeps it, marked as retracted. `load_rows` refuses a row whose \
key the store holds with other values, because it would replace that row: \
set `replace` only when you were asked to change it.

Text is the other way in: `load_documents` stores documents, and \
`run_extraction` has one of the project's extractors, which \
`describe_store` lists, read them. Each place where a document names \
something becomes a record, and `run_match` places those as it places rows.

Looking things up: `search_identities` and `search_records` find by words; \
`get_identity` lists an identity's records and `search_records` with \
`identity` shows what each one says; `get_identity_at` reads its records \
as of a changeset; `explain_pair` says why two records \
share an identity or do not; `list_review_items` and `get_review_case` show \
what the server could not decide by itself.

Changing things: every change is a changeset, a list of operations the \
server checks before it commits all of them or none. `commit_changeset` \
merges identities, moves or places records, and records that two records \
must or must not share an identity; `split_identity` divides one. A change \
is recorded in the log under the name of this connection with the reason \
you give, so always give one, and `undo_changeset` takes a change back. \
Read what you are about to change first, and pass the `head` you read as \
`base` so that a change someone else made in between is noticed. A refused \
changeset changes nothing; its `rejections` say which operation broke \
which rule.";

/// Serves the endpoint.
pub(super) fn build_mcp_service(
    app: App,
    options: &ServerOptions,
) -> StreamableHttpService<Tools, NeverSessionManager> {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(options.max_body_bytes.min(MAX_REQUEST_BYTES))
        // Which names the server answers to is the server's to say, for
        // every path at once: a development server answers only to this
        // machine's, and any other to whatever name its operator gave it.
        .disable_allowed_hosts();
    let tools = Tools { app };
    StreamableHttpService::new(
        move || Ok(tools.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    )
}

/// Gives what the transport refuses in words of its own, such as a request
/// that does not accept JSON, the body every other failure of this server
/// has, with those words in it. An error of the protocol is JSON already.
pub(super) async fn describe_refusal(response: Response) -> Response {
    let status = response.status();
    let json = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    if json || !(status.is_client_error() || status.is_server_error()) {
        return response;
    }
    let (parts, body) = response.into_parts();
    let said = to_bytes(body, MAX_REFUSAL_BYTES).await.unwrap_or_default();
    let said = String::from_utf8_lossy(&said);
    let code = match status {
        StatusCode::FORBIDDEN => ErrorCode::Forbidden,
        StatusCode::NOT_FOUND => ErrorCode::NotFound,
        StatusCode::PAYLOAD_TOO_LARGE => ErrorCode::TooLarge,
        _ if status.is_server_error() => ErrorCode::Internal,
        _ => ErrorCode::Invalid,
    };
    let said = match said.trim() {
        "" => "the request could not be read",
        said => said,
    };
    let mut described = ApiError::new(status, code, said).into_response();
    if let Some(allow) = parts.headers.get(ALLOW) {
        described.headers_mut().insert(ALLOW, allow.clone());
    }
    described
}

#[derive(Clone)]
pub(super) struct Tools {
    app: App,
}

impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        let server = Implementation::new("spool", env!("CARGO_PKG_VERSION"))
            .with_title("Spool")
            .with_description(
                "Resolve records from multiple sources into persistent identities, look up the \
                 records behind them, and review or correct matches.",
            )
            .with_website_url("https://spool.alexpatow.com")
            .with_icons(vec![SERVER_ICON.clone()]);
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(server)
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools::list_tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools::find_tool(name).map(|declared| declared.tool.clone())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(tool) = tools::find_tool(&request.name) else {
            return Err(ErrorData::invalid_params(
                format!(
                    "there is no tool called `{}`",
                    truncate_to_field(&request.name)
                ),
                None,
            ));
        };
        // The server authorized the request before it came here and left
        // who it is from with the request.
        let caller = context
            .extensions
            .get::<Parts>()
            .and_then(|parts| parts.extensions.get::<Caller>())
            .cloned()
            .ok_or_else(|| {
                ErrorData::internal_error("the request did not say who is calling", None)
            })?;
        let arguments = request.arguments.unwrap_or_default();
        let result = tool.call(self.app.clone(), caller, arguments).await;
        Ok(result.into())
    }
}
