//! The tools: how one is declared, and the list of them. Each is declared
//! in the file of what it is about, named as the handlers of `/v1` are:
//! its arguments, what a client is told about it, and what it does.

mod changesets;
mod documents;
mod explain;
mod identities;
mod info;
mod load;
mod records;
mod review;
mod runs;

use std::pin::Pin;
use std::sync::{Arc, LazyLock};

use rmcp::handler::server::tool::schema_for_input;
use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::limits::build_tool_result;
use crate::server::App;
use crate::server::auth::Caller;
use crate::server::error::{ApiError, ApiResult};
use crate::server::request::clamp_page_size;

/// How many entries a list returns when the call does not say.
const DEFAULT_LIMIT: u64 = 20;

/// How many entries a list returns: as many as the call says, and no more
/// than a page of the API.
fn limit_or_default(limit: Option<u64>) -> u64 {
    clamp_page_size(Some(limit.unwrap_or(DEFAULT_LIMIT)))
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
pub(super) struct Declared {
    pub(super) tool: Tool,
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
    pub(super) async fn call(&self, app: App, caller: Caller, given: JsonObject) -> CallToolResult {
        build_tool_result((self.run)(app, caller, given).await, self.in_parts)
    }
}

/// Declares a tool: its name, what a client is told about it, and what it
/// does. Its arguments are read as `Asked`, whose schema is what the client
/// is told they are, and what it answers with is returned as JSON.
fn declare_tool<Asked, Answer, Run, Fut>(
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
        match parse_arguments(name, given) {
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
fn parse_arguments<T: DeserializeOwned>(tool: &str, arguments: JsonObject) -> ApiResult<T> {
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
        info::describe_store(),
        identities::search_identities(),
        identities::get_identity(),
        identities::get_identity_history(),
        identities::get_identity_at(),
        records::search_records(),
        records::get_record(),
        explain::explain_pair(),
        review::list_review_items(),
        review::get_review_case(),
        changesets::list_constraints(),
        changesets::list_changesets(),
        changesets::list_changeset_ops(),
        runs::get_run(),
        changesets::mint_identity_ids(),
        load::add_record(),
        documents::add_document(),
        load::load_rows(),
        documents::load_documents(),
        documents::run_extraction(),
        runs::run_match(),
        load::retract_rows(),
        changesets::commit_changeset(),
        changesets::split_identity(),
        changesets::undo_changeset(),
        review::dismiss_review_item(),
    ]
});

/// What a client is told about each tool, in the order it lists them.
pub(super) fn list_tools() -> Vec<Tool> {
    TOOLS.iter().map(|declared| declared.tool.clone()).collect()
}

/// The tool of a name.
pub(super) fn find_tool(name: &str) -> Option<&'static Declared> {
    TOOLS.iter().find(|declared| declared.tool.name == name)
}
