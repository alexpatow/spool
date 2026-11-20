//! Tools that find records and read one.

use schemars::JsonSchema;
use serde::Deserialize;
use spool::RecordFilter;
use spool::core::{EntityType, IdentityId, RecordId};

use super::{Declared, Effect, declare_tool, limit_or_default};

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

pub(super) fn search_records() -> Declared {
    declare_tool(
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

pub(super) fn get_record() -> Declared {
    declare_tool(
        "get_record",
        "Get a record",
        Effect::Reads,
        "One record by its ID: its values as its source sent them, where it came from, \
         and the identity it belongs to now.",
        |app, _, asked: OneRecord| async move { app.read_record(asked.record).await },
    )
}
