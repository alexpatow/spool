//! The MCP endpoint: what an agent needs of the API, as tools.
//!
//! `/mcp` speaks the Model Context Protocol over Streamable HTTP, so that an
//! assistant such as Claude or ChatGPT can be pointed at the server and look
//! things up in the store, and change identities, on someone's behalf. The
//! protocol and its transport are the `rmcp` crate's. What is here is the
//! list of tools and what each one does.
//!
//! Tools run as the caller the request came from, and call the same
//! operations as the paths under `/v1` where they have one. What a tool changes is
//! recorded under the agent whose token the client sends or the person
//! signed in. Nothing in a tool's arguments says who is calling.
//!
//! The endpoint keeps nothing between requests. Each one is authorized on
//! its own, a tool call is answered in the response to it, and there is no
//! session to resume or to steal.

mod limits;

use std::pin::Pin;
use std::sync::{Arc, LazyLock};

use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::http::header::{ALLOW, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rmcp::handler::server::tool::schema_for_input;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Icon, Implementation, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spool::core::{EntityType, IdentityId, Op, RecordId, RunId, Seq};
use spool::{
    ConstraintFilter, IdentityFilter, NewDocument, RecordFilter, ReviewFilter, ReviewStatus,
    StrategyRef,
};

use self::limits::{MAX_RESULT_BYTES, answer, beginning, prune, size};
use super::auth::Caller;
use super::error::{ApiError, ApiResult};
use super::request::page;
use super::work::engine;
use super::{App, ServerOptions};
use crate::api::{
    AddDocumentRequest, AddRecordRequest, CommitRequest, DismissRequest, DocumentsRequest,
    ErrorCode, ExtractRequest, LoadRequest, MatchRequest, MintRequest, RetractRequest,
    SplitRequest, UndoRequest,
};

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
pub(super) fn service(
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
        Ok(ListToolsResult::with_all_items(list_tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        find_tool(name).map(|declared| declared.tool.clone())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(tool) = find_tool(&request.name) else {
            return Err(ErrorData::invalid_params(
                format!("there is no tool called `{}`", beginning(&request.name)),
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

/// How many entries a list returns when the call does not say.
const DEFAULT_LIMIT: u64 = 20;

/// How many entries a list returns: as many as the call says, and no more
/// than a page of the API.
fn limit_or_default(limit: Option<u64>) -> u64 {
    page(Some(limit.unwrap_or(DEFAULT_LIMIT)))
}

/// What a client is told a tool does to the store, which it may use to
/// decide whether to ask the person first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    Reads,
    Changes,
}

/// A tool at work, which ends with the JSON the tool answers with or with
/// why it failed.
type Running = Pin<Box<dyn Future<Output = ApiResult<String>> + Send>>;

/// A tool: what a client is told about it, and what a call of it does.
struct Declared {
    tool: Tool,
    /// Whether what the tool answers with can be asked for again in smaller
    /// parts. An answer too large to return is then refused, and otherwise
    /// returned without its large parts.
    in_parts: bool,
    run: Box<dyn Fn(App, Caller, JsonObject) -> Running + Send + Sync>,
}

impl Declared {
    /// Says that what the tool answers with is one thing, which cannot be
    /// asked for in smaller parts.
    fn answered_whole(mut self) -> Self {
        self.in_parts = false;
        self
    }

    /// Calls the tool for a caller, with the arguments a client gave.
    async fn call(&self, app: App, caller: Caller, given: JsonObject) -> CallToolResult {
        answer((self.run)(app, caller, given).await, self.in_parts)
    }
}

/// Declares a tool: its name, what a client is told about it, and what it
/// does. Its arguments are read as `Asked`, whose schema is what the client
/// is told they are, and what it answers with is returned as JSON.
fn declare<Asked, Answer, Run, Fut>(
    name: &'static str,
    title: &'static str,
    effect: Effect,
    description: &'static str,
    run: Run,
) -> Declared
where
    Asked: DeserializeOwned + JsonSchema + 'static,
    Answer: Serialize,
    Run: Fn(App, Caller, Asked) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ApiResult<Answer>> + Send + 'static,
{
    let schema = schema_for_input::<Asked>()
        .unwrap_or_else(|error| panic!("the arguments of `{name}` have no schema: {error}"));
    let schema = Arc::new(left_out(schema.as_ref().clone()));
    let annotations = ToolAnnotations::with_title(title)
        .read_only(effect == Effect::Reads)
        .destructive(effect == Effect::Changes)
        .idempotent(effect == Effect::Reads)
        // Every tool works on this server's store and reaches nothing else.
        .open_world(false);
    let tool = Tool::new(name, description, schema)
        .with_title(title)
        .with_annotations(annotations);
    let run = move |app: App, caller: Caller, given: JsonObject| -> Running {
        match arguments(name, given) {
            Ok(asked) => {
                let answer = run(app, caller, asked);
                Box::pin(async move { write_answer(&answer.await?) })
            }
            Err(refused) => Box::pin(std::future::ready(Err(refused))),
        }
    };
    Declared {
        tool,
        // What a tool looked up can be asked for again in smaller parts.
        in_parts: effect == Effect::Reads,
        run: Box::new(run),
    }
}

/// Reads a tool's arguments. What does not fit is the model's to correct,
/// so it is an answer and not a failure of the protocol.
fn arguments<T: DeserializeOwned>(tool: &str, arguments: JsonObject) -> ApiResult<T> {
    serde_json::from_value(Value::Object(arguments)).map_err(|error| {
        ApiError::invalid(format!(
            "the arguments are not what `{tool}` takes: {error}"
        ))
    })
}

/// What a tool answers with, as the JSON the API answers with.
fn write_answer(answer: &impl Serialize) -> ApiResult<String> {
    serde_json::to_string(answer)
        .map_err(|error| ApiError::internal(format!("the answer could not be written: {error}")))
}

/// Describes an argument that may be left out as what it is when given.
///
/// The schema derived from an `Option` says the value may also be `null`
/// and that `null` is its default, which is three times the text and tells
/// a model nothing: such an argument is not in `required`, and that is
/// what says it may be left out. Only a tool's own arguments are changed.
/// Inside an operation, `null` means something.
fn left_out(mut schema: JsonObject) -> JsonObject {
    let null = |schema: &Value| schema.get("type").is_some_and(|kind| kind == "null");
    let Some(Value::Object(properties)) = schema.get_mut("properties") else {
        return schema;
    };
    for property in properties.values_mut() {
        let Value::Object(property) = property else {
            continue;
        };
        if property.get("default").is_some_and(Value::is_null) {
            property.remove("default");
        }
        if let Some(Value::Array(kinds)) = property.get("type")
            && let [kind, nothing] = kinds.as_slice()
            && nothing == "null"
        {
            property.insert("type".to_owned(), kind.clone());
        }
        if let Some(Value::Array(any)) = property.get("anyOf")
            && let [given, nothing] = any.as_slice()
            && null(nothing)
            && let Value::Object(given) = given.clone()
        {
            property.remove("anyOf");
            property.extend(given);
        }
    }
    schema
}

/// Every tool, in the order a client lists them: reading first.
static TOOLS: LazyLock<Vec<Declared>> = LazyLock::new(|| {
    vec![
        describe_store(),
        search_identities(),
        get_identity(),
        get_identity_history(),
        get_identity_at(),
        search_records(),
        get_record(),
        explain_pair(),
        list_review_items(),
        get_review_case(),
        list_constraints(),
        list_changesets(),
        list_changeset_ops(),
        get_run(),
        mint_identity_ids(),
        add_record(),
        add_document(),
        load_rows(),
        load_documents(),
        run_extraction(),
        run_match(),
        retract_rows(),
        commit_changeset(),
        split_identity(),
        undo_changeset(),
        dismiss_review_item(),
    ]
});

/// What a client is told about each tool, in the order it lists them.
fn list_tools() -> Vec<Tool> {
    TOOLS.iter().map(|declared| declared.tool.clone()).collect()
}

/// The tool of a name.
fn find_tool(name: &str) -> Option<&'static Declared> {
    TOOLS.iter().find(|declared| declared.tool.name == name)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Nothing {}

fn describe_store() -> Declared {
    declare(
        "describe_store",
        "Describe the store",
        Effect::Reads,
        "What this Spool store holds: its entity types with their features and how \
         many records and identities each has; in `sources`, the sources the project \
         declares, each with its key column, its columns and the entity types it sends \
         rows of; in `extractors`, the project's extractors; how many review items are \
         open; and `head`, the position of the last change. Use this first, to learn \
         the entity type and source names the other tools take.",
        |app, _, Nothing {}| async move { app.describe_store().await },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchIdentities {
    /// Only identities of this entity type.
    #[serde(default)]
    entity_type: Option<EntityType>,
    /// Only identities with a member whose label or record holds every
    /// word of this text. Case and accents do not matter.
    #[serde(default)]
    q: Option<String>,
    /// Only these identities. One that was merged away is not found here:
    /// `get_identity` says which identity stands for it.
    #[serde(default)]
    ids: Option<Vec<IdentityId>>,
    /// For the next page: the `next` of the page before.
    #[serde(default)]
    after: Option<IdentityId>,
    /// How many to return, 20 unless given.
    #[serde(default)]
    limit: Option<u64>,
}

fn search_identities() -> Declared {
    declare(
        "search_identities",
        "Search identities",
        Effect::Reads,
        "Find identities by name, domain or other record values. Results rank identity \
         labels first, then aliases (other member labels), then record values. Each \
         result's `match` says why it appeared, with the matching record and field when \
         available. Without `q` it lists identities in ID order. Follow with \
         `get_identity` or `search_records` for detail.",
        |app, _, asked: SearchIdentities| async move {
            let filter = IdentityFilter {
                entity_type: asked.entity_type,
                q: asked.q,
                ids: asked.ids,
                after: asked.after,
                limit: limit_or_default(asked.limit),
                sort: None,
            };
            app.list_identities(filter).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneIdentity {
    identity: IdentityId,
}

fn get_identity() -> Declared {
    declare(
        "get_identity",
        "Get an identity",
        Effect::Reads,
        "One identity and the records that belong to it now, each with its source and \
         key. If the identity was merged into another, `current` is the one that stands \
         for it and the records are that one's. The values of each record are in \
         `search_records` with `identity`.",
        |app, _, asked: OneIdentity| async move { app.read_identity(asked.identity).await },
    )
}

fn get_identity_history() -> Declared {
    declare(
        "get_identity_history",
        "Get an identity's history",
        Effect::Reads,
        "Every change that touched one identity, oldest first: who made it, when, why, \
         and the operations that named the identity. Use this to answer how an identity \
         came to hold what it holds, and to find the `seq` of a change to undo.",
        |app, _, asked: OneIdentity| async move { app.identity_history(asked.identity).await },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdentityAt {
    identity: IdentityId,
    /// The inclusive changeset position, from `list_changesets` or identity history.
    seq: Seq,
}

fn get_identity_at() -> Declared {
    declare(
        "get_identity_at",
        "Get an identity at a changeset",
        Effect::Reads,
        "Read what an identity contained immediately after changeset `seq`. The response \
         gives the requested `id`, the survivor at that point as `current`, and its label \
         and members at that point. Position zero is the empty store.",
        |app, _, asked: IdentityAt| async move { app.identity_at(asked.identity, asked.seq).await },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchRecords {
    /// Only records of this entity type.
    #[serde(default)]
    entity_type: Option<EntityType>,
    /// Only records from this source.
    #[serde(default)]
    source: Option<String>,
    /// Only the records a source holds under this key. There is more than
    /// one when the row changed between loads.
    #[serde(default)]
    key: Option<String>,
    /// Only the records that belong to this identity now.
    #[serde(default)]
    identity: Option<IdentityId>,
    /// Only records that hold every word of this text, in one of their
    /// values, in their source's name or in their key. Case and accents do
    /// not matter.
    #[serde(default)]
    q: Option<String>,
    /// Only records that have no identity yet.
    #[serde(default)]
    unassigned: Option<bool>,
    /// For the next page: the `next` of the page before.
    #[serde(default)]
    after: Option<RecordId>,
    /// How many to return, 20 unless given.
    #[serde(default)]
    limit: Option<u64>,
}

fn search_records() -> Declared {
    declare(
        "search_records",
        "Search records",
        Effect::Reads,
        "Find records, the rows and mentions as their sources sent them, by words, \
         source, key, or the identity they belong to. Each result has the record's \
         values in `payload`, where it came from, and its identity if it has one. Use \
         it with `identity` to read everything known about one identity, and with \
         `unassigned` to find records nothing has placed.",
        |app, _, asked: SearchRecords| async move {
            let filter = RecordFilter {
                entity_type: asked.entity_type,
                source: asked.source,
                key: asked.key,
                identity: asked.identity,
                q: asked.q,
                unassigned: asked.unassigned.unwrap_or(false),
                after: asked.after,
                limit: limit_or_default(asked.limit),
                ..RecordFilter::default()
            };
            app.list_records(filter).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneRecord {
    record: RecordId,
}

fn get_record() -> Declared {
    declare(
        "get_record",
        "Get a record",
        Effect::Reads,
        "One record by its ID: its values as its source sent them, where it came from, \
         and the identity it belongs to now.",
        |app, _, asked: OneRecord| async move { app.read_record(asked.record).await },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Pair {
    /// One record: its ID, or `<source>:<key>`, such as `crm:c-1`, which
    /// names the row its source holds under that key now. What an extractor
    /// found in a document is also named by its place, as
    /// `<source>:<document key>[<start>-<end>]`. The order of the two does
    /// not matter.
    a: String,
    /// The other record, named the same way.
    b: String,
}

fn explain_pair() -> Declared {
    declare(
        "explain_pair",
        "Explain two records",
        Effect::Reads,
        "Why two records share an identity, or why they do not: what the project's \
         rules say about the pair, what each match run judged, and the constraints \
         between them. Each record is named by its ID or as `<source>:<key>`. Use this \
         before merging or splitting, and when asked why two things were or were not \
         treated as the same.",
        |app, _, asked: Pair| async move { app.explain_pair(&asked.a, &asked.b).await },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReviewItems {
    /// Only items in this state, `open` unless given.
    #[serde(default)]
    status: Option<ReviewStatus>,
    /// Only items about records of this entity type.
    #[serde(default)]
    entity_type: Option<EntityType>,
    /// Only items that hold this record back.
    #[serde(default)]
    record: Option<RecordId>,
}

fn list_review_items() -> Declared {
    declare(
        "list_review_items",
        "List review items",
        Effect::Reads,
        "The cases the server left for someone to decide. `possible_merge` and \
         `conflict` are records it could not place, because they match more than one \
         identity or because the evidence about them contradicts itself. \
         `possible_split` is an identity whose records nothing holds together since a \
         source took back one of its rows: `groups` are the parts that still hold. \
         Lists the open ones unless `status` says otherwise.",
        |app, _, asked: ReviewItems| async move {
            let filter = ReviewFilter {
                status: Some(asked.status.unwrap_or(ReviewStatus::Open)),
                entity_type: asked.entity_type,
                record: asked.record,
                ..ReviewFilter::default()
            };
            app.list_review(filter).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneReviewItem {
    /// The `id` of the review item.
    item: u64,
}

fn get_review_case() -> Declared {
    declare(
        "get_review_case",
        "Get a review case",
        Effect::Reads,
        "One review item with what is needed to decide it: the records it holds back, \
         the identities they matched with their records, what was judged between them, \
         and the constraints that limit the decision. Decide it with `commit_changeset`, \
         by assigning each record to an identity or merging the identities, or close it \
         with `dismiss_review_item`. Leave out a record whose `retracted` is set: its \
         source took it back, it does not wait for a decision, and a changeset that \
         gives it an identity is refused. A `possible_split` is different: its records \
         share one identity, in `candidates`, and `taken_back` has the records whose \
         retraction left the question, with what was judged between them and the \
         records that remain. If the groups are different things, divide the identity \
         with `split_identity`, naming the records of one group. If they are one thing, \
         record a `must_link` between a record of each group with `commit_changeset`, \
         which closes the item, or close it with `dismiss_review_item`.",
        |app, _, asked: OneReviewItem| async move { app.review_case(asked.item).await },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DismissReviewItem {
    /// The `id` of the review item.
    item: u64,
    /// Why, in a sentence. It is kept with the item.
    reason: String,
}

fn dismiss_review_item() -> Declared {
    declare(
        "dismiss_review_item",
        "Dismiss a review item",
        Effect::Changes,
        "Close a review item without placing its records, when it needs no decision. \
         The records stay without an identity. Dismissing a `possible_split` leaves its \
         identity as it is. Recorded with who dismissed it and why.",
        |app, caller, asked: DismissReviewItem| async move {
            let request = DismissRequest {
                reason: Some(asked.reason),
            };
            app.dismiss_review(caller, asked.item, request).await
        },
    )
}

/// An operation has at most three UUIDs and no free text, so a thousand
/// fit in a tool result even when every operation uses all three.
const MAX_LOG_OPS: u64 = 1_000;

/// The most IDs `mint_identity_ids` mints in one call.
const MAX_MINTED: u32 = 100;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Constraints {
    /// Only constraints that name this record.
    #[serde(default)]
    record: Option<RecordId>,
    /// Only constraints that name a record of this identity.
    #[serde(default)]
    identity: Option<IdentityId>,
}

fn list_constraints() -> Declared {
    declare(
        "list_constraints",
        "List constraints",
        Effect::Reads,
        "The decisions in force that two records must share an identity (`must_link`) \
         or must not (`cannot_link`). A changeset that would break one is refused. Give \
         `record` or `identity` to see only those that bear on it.",
        |app, _, asked: Constraints| async move {
            let filter = ConstraintFilter {
                record: asked.record,
                identity: asked.identity,
                include_removed: false,
            };
            app.list_constraints(filter).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Log {
    /// Only changesets after this log position. Left out, from the start.
    #[serde(default)]
    after: Option<Seq>,
    /// Maximum summaries to return, 20 unless given and 1000 at most.
    #[serde(default)]
    limit: Option<u64>,
}

fn list_changesets() -> Declared {
    declare(
        "list_changesets",
        "List changes",
        Effect::Reads,
        "Summaries of committed changesets, oldest first. Each gives its log position `seq`, \
         author, reason, run, and `operation_count`. `started_by` says who asked for a run. \
         `head` is the position of the latest commit. Pass `next` as `after` until `next` is \
         null. Use `list_changeset_ops` with a summary's `seq` to read its operations.",
        read_changeset_summaries,
    )
}

async fn read_changeset_summaries(app: App, _: Caller, asked: Log) -> ApiResult<Value> {
    let after = asked.after.unwrap_or(Seq::ZERO);
    let limit = limit_or_default(asked.limit);
    let (head, summaries) =
        engine(&app, move |engine| engine.changeset_summaries(after, limit)).await?;
    // Reserve space for the envelope and the longest possible cursor.
    let mut room =
        MAX_RESULT_BYTES - size(&json!({ "changesets": [], "head": head, "next": u64::MAX }));
    let mut changesets = Vec::new();
    let mut next = None;
    for summary in summaries {
        let mut entry = json!({
            "seq": summary.seq,
            "changeset": {
                "id": summary.id,
                "base": summary.base,
                "actor": summary.actor,
                "reason": summary.reason,
                "run": summary.run,
            },
            "content_hash": summary.content_hash,
            "started_by": summary.started_by,
            "operation_count": summary.operation_count,
        });
        let mut left_out = Vec::new();
        prune(entry.as_object_mut().unwrap(), "", true, &mut left_out);
        if !left_out.is_empty() {
            entry["left_out"] = json!(left_out);
        }
        // Also allow a byte for the comma that separates entries.
        let takes = size(&entry) + 1;
        if takes > room {
            break;
        }
        room -= takes;
        next = (summary.seq < head).then_some(summary.seq);
        changesets.push(entry);
    }
    Ok(json!({ "changesets": changesets, "head": head, "next": next }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogOps {
    /// The committed changeset's log position, `seq` from `list_changesets`.
    seq: Seq,
    /// Zero-based operation offset. Omit for the first page, then pass back `next`.
    #[serde(default)]
    offset: Option<u64>,
    /// Maximum operations to return, 20 unless given and 1000 at most.
    #[serde(default)]
    limit: Option<u64>,
}

fn list_changeset_ops() -> Declared {
    declare(
        "list_changeset_ops",
        "List changeset operations",
        Effect::Reads,
        "Operations of one committed changeset in their original order. Give its `seq` from \
         `list_changesets`. Pass `next` as `offset` until `next` is null.",
        read_changeset_ops,
    )
}

async fn read_changeset_ops(app: App, _: Caller, asked: LogOps) -> ApiResult<Value> {
    let seq = asked.seq;
    let offset = asked.offset.unwrap_or(0);
    let limit = asked.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LOG_OPS);
    let mut ops = engine(&app, move |engine| {
        engine.changeset_ops(seq, offset, limit + 1)
    })
    .await?;
    let next = if ops.len() as u64 > limit {
        ops.pop();
        Some(offset + limit)
    } else {
        None
    };
    Ok(json!({ "seq": seq, "offset": offset, "ops": ops, "next": next }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MintIdentityIds {
    /// How many IDs, 1 unless given, 100 at most.
    #[serde(default)]
    count: Option<u32>,
}

fn mint_identity_ids() -> Declared {
    declare(
        "mint_identity_ids",
        "Mint identity IDs",
        Effect::Reads,
        "New identity IDs for a `create_identity` operation. An ID that is never used \
         costs nothing. Changes nothing in the store.",
        |app, _, asked: MintIdentityIds| async move {
            let count = asked.count.unwrap_or(1);
            if count > MAX_MINTED {
                return Err(ApiError::invalid(format!(
                    "at most {MAX_MINTED} identity IDs are minted in one call"
                )));
            }
            let request = MintRequest {
                identities: count,
                changesets: 0,
            };
            app.mint_ids(request).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CommitChangeset {
    /// The operations, applied in order, all of them or none.
    ///
    /// `{"op":"merge","from":A,"into":B}` moves every record of identity A
    /// to identity B; A stands for B from then on.
    ///
    /// `{"op":"assign","record":R,"from":X,"to":Y}` moves record R from
    /// identity X to identity Y. `from` must be the identity R has now,
    /// `null` if it has none; `to` is `null` to leave R without an
    /// identity.
    ///
    /// `{"op":"create_identity","identity":I,"entity_type":T}` makes an
    /// empty identity. I comes from `mint_identity_ids`; `assign` records
    /// to it in the same changeset.
    ///
    /// `{"op":"constrain","kind":"cannot_link","a":R1,"b":R2}` records that
    /// two records must never share an identity; with `must_link`, that
    /// they always must. `unconstrain` with the same fields removes one.
    ///
    /// `{"op":"reinstate","identity":I}` makes an identity that was merged
    /// away active again, with no records.
    ops: Vec<Op>,
    /// Why, in a sentence. It is kept in the log beside the change.
    reason: String,
    /// The `head` of the store when you read what you are changing. With
    /// it, a merge is refused if either identity's records changed since.
    /// Left out, the change is made to the store as it is now.
    #[serde(default)]
    base: Option<Seq>,
}

fn commit_changeset() -> Declared {
    declare(
        "commit_changeset",
        "Change identities",
        Effect::Changes,
        "Change identities: merge two, move a record to another identity, place a \
         record that has none, make a new identity, or record that two records must or \
         must not share an identity. The server checks every operation against the \
         store and commits all of them or none; a refusal lists the operations that \
         broke a rule and changes nothing. Placing the latest version of a row moves \
         its earlier versions along, as operations of the same change. The change is \
         recorded under this connection's name and can be taken back with \
         `undo_changeset`. Returns the \
         `seq` it was committed at.",
        |app, caller, asked: CommitChangeset| async move {
            let request = CommitRequest {
                id: None,
                base: asked.base,
                reason: Some(asked.reason),
                run: None,
                ops: asked.ops,
            };
            app.commit_changeset(caller, request).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SplitIdentity {
    /// The identity to divide.
    identity: IdentityId,
    /// The records of one side. The rest of the identity's records are the
    /// other side. The larger side keeps the identity's ID and the smaller
    /// gets a new one.
    records: Vec<RecordId>,
    /// Record that the two sides must not share an identity, so that a
    /// later match does not put them back together. True unless given.
    #[serde(default)]
    keep_apart: Option<bool>,
    /// Why, in a sentence. It is kept in the log beside the change.
    reason: String,
    /// The `head` of the store when you read the identity. With it, the
    /// split is refused if the identity's records changed since.
    #[serde(default)]
    base: Option<Seq>,
}

fn split_identity() -> Declared {
    declare(
        "split_identity",
        "Split an identity",
        Effect::Changes,
        "Divide one identity in two when it holds records of two different things. Name \
         the records of one side; the rest stay. Unless `keep_apart` is false, the two \
         sides are also recorded as never to be merged. Returns the new identity and \
         the changeset that was committed.",
        |app, caller, asked: SplitIdentity| async move {
            let request = SplitRequest {
                records: asked.records,
                keep_apart: asked.keep_apart,
                base: asked.base,
                reason: Some(asked.reason),
            };
            app.split_identity(caller, asked.identity, request).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UndoChangeset {
    /// The log position of the changeset to take back: its `seq`.
    seq: Seq,
    /// Why, in a sentence. It is kept in the log beside the change.
    reason: String,
    /// False to see the changeset that would undo it without committing
    /// it. True unless given.
    #[serde(default)]
    commit: Option<bool>,
}

fn undo_changeset() -> Declared {
    declare(
        "undo_changeset",
        "Undo a change",
        Effect::Changes,
        "Take back one committed changeset by committing its opposite, which is a new \
         entry in the log. Refused if later changes built on it. With `commit` false it \
         shows the changeset that would undo it and changes nothing.",
        |app, caller, asked: UndoChangeset| async move {
            let request = UndoRequest {
                reason: Some(asked.reason),
                commit: asked.commit,
            };
            app.undo_changeset(caller, asked.seq, request).await
        },
    )
}

/// The longest `get_run` waits for a run that is still in progress. A
/// client gives up on a call that takes much longer.
const MAX_RUN_WAIT_MS: u64 = 60_000;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneRun {
    /// The `run` of the run: a number.
    run: u64,
    /// If the run is still in progress, wait up to this many milliseconds
    /// for it to finish. Not at all unless given, 60000 at most.
    #[serde(default)]
    wait_ms: Option<u64>,
}

fn get_run() -> Declared {
    declare(
        "get_run",
        "Get a run",
        Effect::Reads,
        "One run by its number: whether it is `running`, `succeeded` or `failed`, who \
         started it, and what it reported. A load reports in `load` how many rows or documents were \
         new; an extraction reports in `extraction` the documents it read and the \
         records it found; a match run reports in `resolution` how many records it \
         examined, the pairs it judged, the identities it created, and what it left \
         for review. Use this after `load_rows`, `run_extraction` or `run_match` \
         answered with a run that was still `running`, with `wait_ms` to wait for it.",
        |app, _, asked: OneRun| async move {
            let wait_ms = asked.wait_ms.map(|wait| wait.min(MAX_RUN_WAIT_MS));
            app.read_run(RunId(asked.run), wait_ms).await
        },
    )
    .answered_whole()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunMatch {
    /// The entity type whose records are matched.
    entity_type: EntityType,
    /// Match every record of the type again, not only those no run has
    /// examined. Records that have an identity keep it: where the rules
    /// now disagree with the identities, the run's `resolution` lists
    /// `suggestions` and changes nothing. Records that have none, such as
    /// those of a match run that was undone, are placed. False unless
    /// given.
    #[serde(default)]
    rematch: Option<bool>,
    /// The name of one of the project's strategies for the type, which
    /// `describe_store` lists. Left out, the project's SQL rules judge.
    #[serde(default)]
    strategy: Option<String>,
}

fn run_match() -> Declared {
    declare(
        "run_match",
        "Run a match",
        Effect::Changes,
        "Match the records of one entity type that no run has examined, and commit the \
         result: each record joins the identity it matches or gets a new one, and \
         identities that existed keep their IDs. A record that matches more than one \
         identity, or that the evidence contradicts itself about, is left as a review \
         item. The change is one changeset in the log, which `undo_changeset` takes \
         back. Returns the run, whose `resolution` has the counts; `run` is null when \
         there was nothing to match. If its `status` is still `running`, read it with \
         `get_run`.",
        |app, caller, asked: RunMatch| async move {
            let strategy = match asked.strategy {
                Some(name) => Some(project_strategy(&app, &asked.entity_type, name)?),
                None => None,
            };
            let request = MatchRequest {
                entity_type: asked.entity_type,
                strategy,
                // A draft would wait for an approval no tool gives.
                commit: None,
                rematch: asked.rematch,
                wait_ms: None,
            };
            Ok(app.start_match(caller, request).await?.run)
        },
    )
}

/// The strategy a match run is asked for by name, which has to be one
/// the server runs. The API takes any other name as that of a strategy
/// the caller runs, and starts a run that waits for its judgements;
/// no tool sends any, so such a run would wait for good.
fn project_strategy(app: &App, entity_type: &EntityType, name: String) -> ApiResult<StrategyRef> {
    let manifest = app.engine.manifest();
    let known = name == StrategyRef::RULES
        || manifest
            .as_deref()
            .and_then(|manifest| manifest.entity_type(entity_type.as_str()))
            .is_some_and(|entry| entry.strategy(&name).is_some());
    match known {
        true => Ok(StrategyRef {
            name,
            version: None,
        }),
        false => Err(ApiError::invalid(format!(
            "the project has no strategy `{}` for `{entity_type}`: `describe_store` lists \
             the strategies of each entity type, and `rules` is always one",
            beginning(&name)
        ))),
    }
}

fn add_record() -> Declared {
    declare(
        "add_record",
        "Add a record and find its identity",
        Effect::Changes,
        "Store one row as a record and use the project's rules to match it against existing \
         identities. Returns the record ID and its identity. If it needs review, the identity \
         is null and the result includes the review item and the identities it matched. \
         Retry with the same row to get the same record. Set `replace` only when the person \
         asked to change a row under a key the source already holds. Use `load_rows` and \
         `run_match` to load and compare a batch of rows together.",
        |app, caller, mut request: AddRecordRequest| async move {
            empty_fields(&mut request.row);
            app.add_record(caller, request).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LoadRows {
    /// The name of the source the rows come from. The project declares its
    /// sources: `describe_store` lists them in `sources`, each with its
    /// columns and the entity types it sends rows of.
    source: String,
    /// The entity type the rows are records of.
    entity_type: String,
    /// One object for each row, such as
    /// `{"id": "c-1", "name": "Acorn Tools", "country": "GB"}`. Its fields
    /// are columns the source declares, and a column it does not declare
    /// is refused. A value is text, a number or a boolean. An empty field
    /// of a file has no value: leave the column out of that row, or send
    /// `""` or `null`, which are read the same. The source's `key` column
    /// tells one row from another: give it in every row.
    rows: Vec<JsonObject>,
    /// Whether these rows may replace rows the store holds. Left out or
    /// `false`, a row whose key the store holds with other values is
    /// refused, and the whole call with it. Set it to `true` only when the
    /// person asked for those rows to change.
    replace: Option<bool>,
}

fn load_rows() -> Declared {
    declare(
        "load_rows",
        "Load rows",
        Effect::Changes,
        "Load rows from one source into the store, as records of one entity type. \
         Send the rows as they are in the file or message you were given: read it \
         first, and never make a row up. `describe_store` lists the project's sources \
         in `sources`, each with the columns a row may have. A row the store already \
         holds, with the same key and the same values, adds nothing, so loading the \
         same rows twice is safe. A row with a known key and other values would \
         replace the row the store holds under that key, so it is refused, and the \
         whole call with it: the error names the keys and nothing is loaded. If the \
         person asked for those rows to change, load again with `replace` set to \
         true; otherwise the rows are not the file's, so read the file again. Values \
         the row held before count as other values: loading them changes the row \
         back, so an older file loaded after a newer one is refused the same way. \
         Loading does not match: follow it with `run_match`. Load every row you were \
         asked to load, and no row you were not. A row loaded by mistake, or that the \
         person wants out later, is taken back with `retract_rows`, which removes it \
         from its identity and keeps it, marked as retracted. This is for a \
         few thousand rows at most: a call is 4 MiB at most, and a pipeline loads \
         more with `POST /v1/load`. Returns the run that stored the rows, whose \
         `load` counts them.",
        |app, caller, asked: LoadRows| async move {
            // An empty field of a file reaches a tool as `""`. It is a
            // value the row does not have, as it is when the file is
            // sent as CSV. Kept as text it would be a value like any
            // other, and a rule that compares it would hold or fail
            // where it should say nothing. The column stays in the
            // row, so one the source does not list is still refused.
            let mut rows = asked.rows;
            for row in &mut rows {
                empty_fields(row);
            }
            // The API replaces a row unless it is told not to. An
            // assistant is to add rows unless it is told to change one.
            let request = LoadRequest {
                source: asked.source,
                entity_type: asked.entity_type,
                rows,
                replace: Some(asked.replace.unwrap_or(false)),
                wait_ms: None,
            };
            Ok(app.load_rows(caller, request).await?.run)
        },
    )
}

/// Treat empty strings as missing, as the file loaders do. Keep every
/// column name so the engine can still refuse columns the source does not list.
fn empty_fields(row: &mut JsonObject) {
    for value in row.values_mut() {
        if value.as_str() == Some("") {
            *value = Value::Null;
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RetractRows {
    /// The name of the source that held the rows.
    source: String,
    /// The entity type the rows are records of.
    entity_type: String,
    /// The keys of the rows, as the source's key column gave them. A key
    /// the source has no row under is counted as `unknown` and changes
    /// nothing.
    keys: Vec<String>,
    /// Why, in a sentence. It is kept in the log beside the change.
    reason: String,
}

fn retract_rows() -> Declared {
    declare(
        "retract_rows",
        "Take rows back",
        Effect::Changes,
        "Take back rows a source no longer holds, or that were loaded by mistake, by \
         their keys. Every version of each row leaves its identity and is examined by \
         no later run. The rows are not deleted: they stay in the store as evidence, \
         marked as retracted. Loading a row under the same key again ends the \
         retraction and puts its records back in the identity they left. A record an \
         extractor found in a document is taken back the same way, by the `key` \
         `search_records` shows for it. Returns the run, whose `retraction` counts \
         what was taken back.",
        |app, caller, asked: RetractRows| async move {
            let request = RetractRequest {
                source: asked.source,
                entity_type: asked.entity_type,
                keys: asked.keys,
                reason: Some(asked.reason),
                wait_ms: None,
            };
            Ok(app.retract_rows(caller, request).await?.run)
        },
    )
}

fn add_document() -> Declared {
    declare(
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

fn load_documents() -> Declared {
    declare(
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
        beginning(source),
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

fn run_extraction() -> Declared {
    declare(
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
