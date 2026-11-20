//! Why two records share an identity, or do not.
//!
//! An explanation gathers what the store holds about a pair: where each
//! record is and which changeset put it there, what every run that
//! looked at the pair concluded, the constraints and review items that name
//! them, and what the project's rules say about the pair as it stands. It
//! decides nothing and changes nothing.
//!
//! Two things in it are computed for the question rather than read back:
//! the reading of the rules, which uses the rules and the feature rows as
//! they are now and so can differ from what a run recorded, and the chain
//! of links between two members of one identity that were never compared
//! with each other.

use duckdb::Connection;
use spool_core::{RecordId, StateError};

use crate::error::Result;

mod identity;
mod pair;
mod prose;

pub(crate) use identity::explain_identity;
pub use identity::{IdentityExplanation, Link, LinkKind, LinkValue, NameCount, Named, Side, Split};
pub(crate) use pair::explain_pair;
pub use pair::{
    Comparison, Explained, Explanation, Outcome, Placement, RecordedJudgement, RuleReading,
    RuleResult,
};
pub use prose::how_linked;

/// Whether a feature table exists. A store read without its manifest can
/// hold records of a type whose table is not there.
fn has_table(conn: &Connection, table: &str) -> Result<bool> {
    let name = table.rsplit('.').next().unwrap_or(table);
    let found: i64 = conn.query_row(
        "SELECT count(*) FROM information_schema.tables
         WHERE table_schema = 'spool' AND table_name = ?",
        [name],
        |row| row.get(0),
    )?;
    Ok(found > 0)
}

/// An entity type's feature row of a record, by column name, without the
/// two columns the engine adds. `None` when it has none.
fn feature_row(
    conn: &Connection,
    table: &str,
    id: RecordId,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>> {
    if !has_table(conn, table)? {
        return Ok(None);
    }
    // The whole row: a table may have no columns but the two the engine
    // adds, and a query has to select something.
    let mut statement = conn.prepare(&format!(
        "SELECT to_json(f)::VARCHAR FROM {table} f WHERE f.record_id = ?::UUID"
    ))?;
    let mut rows = statement.query([id.to_string()])?;
    match rows.next()? {
        Some(row) => {
            let mut features: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&row.get::<_, String>(0)?).map_err(StateError::new)?;
            features.remove("record_id");
            features.remove("declaration_version");
            Ok(Some(features))
        }
        None => Ok(None),
    }
}
