//! Reads of single things: a record, an identity and its members, the
//! changesets that touched an identity, and what the store holds overall.
//!
//! Everything here reads committed state from a connection of its own, so a
//! lookup does not wait for a run that is in progress.

use spool_core::{EntityType, StateError};

use crate::error::Result;

mod identities;
mod info;
mod log;
mod records;

pub use identities::{
    IdentityFilter, IdentityMatch, IdentitySnapshot, IdentitySort, IdentitySummary, IdentityView,
    Member, SourceCount,
};
pub(crate) use identities::{identities, identities_by_id, identity, identity_at};
pub(crate) use info::info;
pub use info::{DeclaredSource, SourceInfo, StoreInfo, TypeInfo};
pub use log::{ChangesetSummary, HistoryEntry, LogSort, LoggedChangeset};
pub(crate) use log::{changeset_summaries, changesets, history, logged_changesets};
pub(crate) use records::{DOCUMENT_STANDS, members, record, records, records_by_id};
pub use records::{FoundBy, RecordEntry, RecordFilter, RecordSort};

fn entity_type(name: String) -> Result<EntityType> {
    Ok(EntityType::new(name).map_err(StateError::new)?)
}

/// The words of a search, as the JSON list [`holds_words`] reads. `None`
/// when the text has no word, which finds everything.
fn encode_search_words(q: Option<&str>) -> Option<String> {
    let words: Vec<&str> = q?.split_whitespace().collect();
    (!words.is_empty()).then(|| serde_json::json!(words).to_string())
}

fn clamp_limit(limit: u64) -> i64 {
    limit.min(i64::MAX as u64) as i64
}

/// The IDs as a SQL list of UUIDs. An ID is hexadecimal digits and hyphens,
/// so it is safe to splice.
pub(crate) fn uuid_list<T: std::fmt::Display>(ids: &[T]) -> String {
    let quoted: Vec<String> = ids.iter().map(|id| format!("'{id}'")).collect();
    format!("[{}]::UUID[]", quoted.join(", "))
}
