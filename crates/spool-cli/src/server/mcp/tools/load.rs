//! Tools for rows: loading rows of a source, adding one record, and
//! taking rows back.

use rmcp::model::JsonObject;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{Declared, Effect, declare_tool};
use crate::api::{AddRecordRequest, LoadRequest, RetractRequest};

pub(super) fn add_record() -> Declared {
    declare_tool(
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

pub(super) fn load_rows() -> Declared {
    declare_tool(
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

pub(super) fn retract_rows() -> Declared {
    declare_tool(
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
