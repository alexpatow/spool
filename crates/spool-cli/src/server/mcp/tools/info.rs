//! The tool that says what the store holds.

use schemars::JsonSchema;
use serde::Deserialize;

use super::{Declared, Effect, declare};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Nothing {}

pub(super) fn describe_store() -> Declared {
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
