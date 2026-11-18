//! The MCP endpoint: what an agent needs of the API, as tools.
//!
//! `/mcp` speaks the Model Context Protocol over Streamable HTTP, so that an
//! assistant such as Claude or ChatGPT can be pointed at the server and look
//! things up in the store, and change identities, on someone's behalf. The
//! protocol and its transport are the `rmcp` crate's. What is here is the
//! list of tools and what each one does.
//!
//! A tool is a call of the API. It runs the handler the path under `/v1`
//! runs, as the caller the request came from, so a tool can do nothing the
//! API refuses and what it changes is recorded under the same name: the
//! agent whose token the client sends, or the person signed in. Nothing in a
//! tool's arguments says who is calling.
//!
//! The endpoint keeps nothing between requests. Each one is authorized on
//! its own, a tool call is answered in the response to it, and there is no
//! session to resume or to steal.

use std::sync::{Arc, LazyLock};

use axum::body::{Bytes, to_bytes};
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::{ALLOW, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::tool::schema_for_input;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use spool::ReviewStatus;
use spool::core::{EntityType, IdentityId, Op, RecordId, Seq};

use super::{
    ApiError, ApiResult, App, Caller, ConstraintsQuery, ExplainQuery, IdentitiesQuery, LogQuery,
    RecordsQuery, ReviewQuery, ServerOptions,
};
use crate::api::{
    CommitRequest, DismissRequest, ErrorCode, ErrorResponse, MintRequest, SplitRequest, UndoRequest,
};

/// The largest request the endpoint reads. A request is a tool call, and the
/// largest of those is a changeset.
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// The largest result a tool returns. A result goes into what the model
/// reads, where a megabyte of JSON displaces everything else, and a client
/// cuts off what it cannot hold without saying where.
const MAX_RESULT_BYTES: usize = 256 * 1024;

/// The most that is read of what the transport says when it refuses.
const MAX_REFUSAL_BYTES: usize = 4 * 1024;

/// How many entries a list returns when the call does not say.
const DEFAULT_LIMIT: u64 = 20;

/// The most IDs `mint_identity_ids` mints in one call.
const MAX_MINTED: u32 = 100;

/// What a client is told about the server when it connects. A client may
/// show the model only the beginning, so the first paragraph stands alone.
const INSTRUCTIONS: &str = "\
Spool resolves records from many sources into identities: one identity for \
each real-world thing, holding every record that refers to it. Use these \
tools to find an identity and the records behind it, to see why two records \
are or are not the same thing, and to correct identities. Start with \
`describe_store`, which lists the entity types and sources this store has.

Looking things up: `search_identities` and `search_records` find by words; \
`get_identity` lists an identity's records and `search_records` with \
`identity` shows what each one says; `explain_pair` says why two records \
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

/// Serves the endpoint. `dev` is a server nobody signs in to.
pub(super) fn service(
    app: App,
    options: &ServerOptions,
) -> StreamableHttpService<Tools, NeverSessionManager> {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(options.max_body_bytes.min(MAX_REQUEST_BYTES));
    // A page on another site can reach a server on this machine by a name
    // of its own that resolves here, and the browser then sends the page's
    // requests as if to the page's own site. A server that asks for no
    // sign-in answers them, so it answers only to the names of this
    // machine. Elsewhere every request carries a token or a session, which
    // such a page does not have, and the server is reached by whatever
    // name its operator gave it.
    let config = match options.dev {
        true => config,
        false => config.disable_allowed_hosts(),
    };
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
        let mut server = Implementation::new("spool", env!("CARGO_PKG_VERSION"));
        server.title = Some("Spool".to_owned());
        server.description = Some(
            "Entity resolution with persistent identity: records, the identities they \
             belong to, and the changes made to them."
                .to_owned(),
        );
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(server)
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(TOOLS.clone()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        TOOLS.iter().find(|tool| tool.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.get_tool(&request.name).is_none() {
            return Err(ErrorData::invalid_params(
                format!("there is no tool called `{}`", request.name),
                None,
            ));
        }
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
        let result = self.call(&request.name, arguments, caller).await;
        Ok(answer(result).await.into())
    }
}

/// The result of a tool: the JSON the API answers with, as text. A failure
/// is a result too, with the API's error in it, so that the model reads why
/// and can put it right.
async fn answer(result: ApiResult) -> CallToolResult {
    let body = match result {
        Ok(response) => match to_bytes(response.into_body(), usize::MAX).await {
            Ok(body) => body,
            Err(_) => return refused(ApiError::internal("the answer could not be read")),
        },
        Err(error) => return refused(error),
    };
    if body.len() > MAX_RESULT_BYTES {
        return refused(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::TooLarge,
            format!(
                "the answer is {} bytes and a tool returns at most {MAX_RESULT_BYTES}: ask for \
                 less, with a smaller `limit` or a narrower search",
                body.len()
            ),
        ));
    }
    match String::from_utf8(body.to_vec()) {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(_) => refused(ApiError::internal("the answer is not text")),
    }
}

fn refused(error: ApiError) -> CallToolResult {
    let body = serde_json::to_string(&ErrorResponse { error: error.body })
        .unwrap_or_else(|_| "the call failed".to_owned());
    CallToolResult::error(vec![ContentBlock::text(body)])
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

/// A request body, as the handler of a path reads it.
fn body<T: serde::Serialize>(request: &T) -> ApiResult<Bytes> {
    serde_json::to_vec(request)
        .map(Bytes::from)
        .map_err(|error| ApiError::internal(format!("the request could not be written: {error}")))
}

fn text<T: ToString>(id: Option<T>) -> Option<String> {
    id.map(|id| id.to_string())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Nothing {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchIdentities {
    /// Only identities of this entity type.
    #[serde(default)]
    entity_type: Option<EntityType>,
    /// Only identities with a record that holds every word of this text, in
    /// one of its values, in its source's name or in its key. Case and
    /// accents do not matter.
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneIdentity {
    identity: IdentityId,
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
    /// one when the row changed between ingests.
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneRecord {
    record: RecordId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Pair {
    /// One record. The order of the two does not matter.
    a: RecordId,
    /// The other record.
    b: RecordId,
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneReviewItem {
    /// The `id` of the review item.
    item: u64,
}

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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Log {
    /// Only changesets after this log position. Left out, from the start.
    #[serde(default)]
    after: Option<Seq>,
    /// How many to return, 20 unless given. A changeset a match run
    /// committed can hold very many operations.
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MintIdentityIds {
    /// How many IDs, 1 unless given, 100 at most.
    #[serde(default)]
    count: Option<u32>,
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DismissReviewItem {
    /// The `id` of the review item.
    item: u64,
    /// Why, in a sentence. It is kept with the item.
    reason: String,
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

/// What a client is told a tool does to the store, which it may use to
/// decide whether to ask the person first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    Reads,
    Changes,
}

fn tool<T: JsonSchema + 'static>(
    name: &'static str,
    title: &'static str,
    effect: Effect,
    description: &'static str,
) -> Tool {
    let schema = schema_for_input::<T>()
        .unwrap_or_else(|error| panic!("the arguments of `{name}` have no schema: {error}"));
    let schema = Arc::new(left_out(schema.as_ref().clone()));
    let annotations = ToolAnnotations::with_title(title)
        .read_only(effect == Effect::Reads)
        .destructive(effect == Effect::Changes)
        .idempotent(effect == Effect::Reads)
        // Every tool works on this server's store and reaches nothing else.
        .open_world(false);
    Tool::new(name, description, schema)
        .with_title(title)
        .with_annotations(annotations)
}

/// Every tool, in the order a client lists them: reading first.
static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(|| {
    vec![
        tool::<Nothing>(
            "describe_store",
            "Describe the store",
            Effect::Reads,
            "What this Spool store holds: its entity types with their features and sources, \
             how many records and identities each has, how many review items are open, and \
             `head`, the position of the last change. Use this first, to learn the entity \
             type and source names the other tools take.",
        ),
        tool::<SearchIdentities>(
            "search_identities",
            "Search identities",
            Effect::Reads,
            "Find identities, the real-world things records refer to, by words in their \
             records. Use this when asked about a company, person or other entity by name, \
             domain or any other value. Each result has the identity's ID, its label, how \
             many records it has and from which sources. Without `q` it lists identities in \
             ID order. Follow with `get_identity` or `search_records` for the detail.",
        ),
        tool::<OneIdentity>(
            "get_identity",
            "Get an identity",
            Effect::Reads,
            "One identity and the records that belong to it now, each with its source and \
             key. If the identity was merged into another, `current` is the one that stands \
             for it and the records are that one's. The values of each record are in \
             `search_records` with `identity`.",
        ),
        tool::<OneIdentity>(
            "get_identity_history",
            "Get an identity's history",
            Effect::Reads,
            "Every change that touched one identity, oldest first: who made it, when, why, \
             and the operations that named the identity. Use this to answer how an identity \
             came to hold what it holds, and to find the `seq` of a change to undo.",
        ),
        tool::<SearchRecords>(
            "search_records",
            "Search records",
            Effect::Reads,
            "Find records, the rows and mentions as their sources sent them, by words, \
             source, key, or the identity they belong to. Each result has the record's \
             values in `payload`, where it came from, and its identity if it has one. Use \
             it with `identity` to read everything known about one identity, and with \
             `unassigned` to find records nothing has placed.",
        ),
        tool::<OneRecord>(
            "get_record",
            "Get a record",
            Effect::Reads,
            "One record by its ID: its values as its source sent them, where it came from, \
             and the identity it belongs to now.",
        ),
        tool::<Pair>(
            "explain_pair",
            "Explain two records",
            Effect::Reads,
            "Why two records share an identity, or why they do not: what the project's \
             rules say about the pair, what each match run judged, and the constraints \
             between them. Use this before merging or splitting, and when asked why two \
             things were or were not treated as the same.",
        ),
        tool::<ReviewItems>(
            "list_review_items",
            "List review items",
            Effect::Reads,
            "The cases the server left for someone to decide: records it could not place \
             because they match more than one identity, or because the evidence about them \
             contradicts itself. Lists the open ones unless `status` says otherwise.",
        ),
        tool::<OneReviewItem>(
            "get_review_case",
            "Get a review case",
            Effect::Reads,
            "One review item with what is needed to decide it: the records it holds back, \
             the identities they matched with their records, what was judged between them, \
             and the constraints that limit the decision. Decide it with `commit_changeset`, \
             by assigning each record to an identity or merging the identities, or close it \
             with `dismiss_review_item`.",
        ),
        tool::<Constraints>(
            "list_constraints",
            "List constraints",
            Effect::Reads,
            "The decisions in force that two records must share an identity (`must_link`) \
             or must not (`cannot_link`). A changeset that would break one is refused. Give \
             `record` or `identity` to see only those that bear on it.",
        ),
        tool::<Log>(
            "list_changesets",
            "List changes",
            Effect::Reads,
            "The log of committed changesets, oldest first, after a position: who made each, \
             why, and its operations. `head` is the position of the last one. Use this to \
             see what changed since a position you read before.",
        ),
        tool::<MintIdentityIds>(
            "mint_identity_ids",
            "Mint identity IDs",
            Effect::Reads,
            "New identity IDs for a `create_identity` operation. An ID that is never used \
             costs nothing. Changes nothing in the store.",
        ),
        tool::<CommitChangeset>(
            "commit_changeset",
            "Change identities",
            Effect::Changes,
            "Change identities: merge two, move a record to another identity, place a \
             record that has none, make a new identity, or record that two records must or \
             must not share an identity. The server checks every operation against the \
             store and commits all of them or none; a refusal lists the operations that \
             broke a rule and changes nothing. The change is recorded under this \
             connection's name and can be taken back with `undo_changeset`. Returns the \
             `seq` it was committed at.",
        ),
        tool::<SplitIdentity>(
            "split_identity",
            "Split an identity",
            Effect::Changes,
            "Divide one identity in two when it holds records of two different things. Name \
             the records of one side; the rest stay. Unless `keep_apart` is false, the two \
             sides are also recorded as never to be merged. Returns the new identity and \
             the changeset that was committed.",
        ),
        tool::<UndoChangeset>(
            "undo_changeset",
            "Undo a change",
            Effect::Changes,
            "Take back one committed changeset by committing its opposite, which is a new \
             entry in the log. Refused if later changes built on it. With `commit` false it \
             shows the changeset that would undo it and changes nothing.",
        ),
        tool::<DismissReviewItem>(
            "dismiss_review_item",
            "Dismiss a review item",
            Effect::Changes,
            "Close a review item without placing its records, when it needs no decision. \
             The records stay without an identity. Recorded with who dismissed it and why.",
        ),
    ]
});

impl Tools {
    /// Runs a tool: reads its arguments and calls the handler of the path
    /// that does the same.
    async fn call(&self, tool: &str, given: JsonObject, caller: Caller) -> ApiResult {
        let app = State(self.app.clone());
        let caller = Extension(caller);
        let limit = |limit: Option<u64>| Some(limit.unwrap_or(DEFAULT_LIMIT));
        match tool {
            "describe_store" => {
                let Nothing {} = arguments(tool, given)?;
                super::info(app).await
            }
            "search_identities" => {
                let asked: SearchIdentities = arguments(tool, given)?;
                let ids = asked.ids.map(|ids| {
                    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
                    ids.join(",")
                });
                let query = IdentitiesQuery {
                    entity_type: asked.entity_type,
                    q: asked.q,
                    ids,
                    after: text(asked.after),
                    limit: limit(asked.limit),
                };
                super::list_identities(app, Ok(Query(query))).await
            }
            "get_identity" => {
                let asked: OneIdentity = arguments(tool, given)?;
                super::read_identity(app, Path(asked.identity.to_string())).await
            }
            "get_identity_history" => {
                let asked: OneIdentity = arguments(tool, given)?;
                super::identity_history(app, Path(asked.identity.to_string())).await
            }
            "search_records" => {
                let asked: SearchRecords = arguments(tool, given)?;
                let query = RecordsQuery {
                    entity_type: asked.entity_type,
                    source: asked.source,
                    key: asked.key,
                    document: None,
                    identity: text(asked.identity),
                    q: asked.q,
                    unassigned: asked.unassigned,
                    after: text(asked.after),
                    limit: limit(asked.limit),
                };
                super::list_records(app, Ok(Query(query))).await
            }
            "get_record" => {
                let asked: OneRecord = arguments(tool, given)?;
                super::read_record(app, Path(asked.record.to_string())).await
            }
            "explain_pair" => {
                let asked: Pair = arguments(tool, given)?;
                let query = ExplainQuery {
                    a: asked.a.to_string(),
                    b: asked.b.to_string(),
                };
                super::explain(app, Ok(Query(query))).await
            }
            "list_review_items" => {
                let asked: ReviewItems = arguments(tool, given)?;
                let query = ReviewQuery {
                    status: Some(asked.status.unwrap_or(ReviewStatus::Open)),
                    entity_type: asked.entity_type,
                    record: text(asked.record),
                };
                super::list_review(app, Ok(Query(query))).await
            }
            "get_review_case" => {
                let asked: OneReviewItem = arguments(tool, given)?;
                super::review_case(app, Path(asked.item.to_string())).await
            }
            "list_constraints" => {
                let asked: Constraints = arguments(tool, given)?;
                let query = ConstraintsQuery {
                    record: text(asked.record),
                    identity: text(asked.identity),
                    removed: None,
                };
                super::list_constraints(app, Ok(Query(query))).await
            }
            "list_changesets" => {
                let asked: Log = arguments(tool, given)?;
                let query = LogQuery {
                    after: asked.after.map(|seq| seq.0),
                    limit: limit(asked.limit),
                };
                super::list_changesets(app, Ok(Query(query))).await
            }
            "mint_identity_ids" => {
                let asked: MintIdentityIds = arguments(tool, given)?;
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
                super::mint_ids(app, body(&request)?).await
            }
            "commit_changeset" => {
                let asked: CommitChangeset = arguments(tool, given)?;
                let request = CommitRequest {
                    id: None,
                    base: asked.base,
                    reason: Some(asked.reason),
                    run: None,
                    ops: asked.ops,
                };
                super::commit(app, caller, body(&request)?).await
            }
            "split_identity" => {
                let asked: SplitIdentity = arguments(tool, given)?;
                let request = SplitRequest {
                    records: asked.records,
                    keep_apart: asked.keep_apart,
                    base: asked.base,
                    reason: Some(asked.reason),
                };
                let identity = Path(asked.identity.to_string());
                super::split(app, caller, identity, body(&request)?).await
            }
            "undo_changeset" => {
                let asked: UndoChangeset = arguments(tool, given)?;
                let request = UndoRequest {
                    reason: Some(asked.reason),
                    commit: asked.commit,
                };
                let seq = Path(asked.seq.0.to_string());
                super::undo(app, caller, seq, body(&request)?).await
            }
            "dismiss_review_item" => {
                let asked: DismissReviewItem = arguments(tool, given)?;
                let request = DismissRequest {
                    reason: Some(asked.reason),
                };
                let item = Path(asked.item.to_string());
                super::dismiss_review(app, caller, item, body(&request)?).await
            }
            other => Err(ApiError::not_found(format!(
                "there is no tool called `{other}`"
            ))),
        }
    }
}
