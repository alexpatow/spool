//! Reading documents back: one with its text, a list of them, and how
//! many the store holds.

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{DocumentId, RunId};

use super::{DocumentEntry, DocumentExtraction};
use crate::batch::id_list;
use crate::error::Result;
use crate::order::{Key, Keyed, Sort};
use crate::store::parse_stored;
use crate::time::format_rfc3339;

/// Which documents to list. Results are ordered by ID, or as `sort` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentFilter {
    pub source: Option<String>,
    pub key: Option<String>,
    /// Only the version of each document that nothing has replaced.
    pub latest: bool,
    /// Only those that come after this one in the list: the last one of the
    /// page before.
    pub after: Option<DocumentId>,
    pub limit: u64,
    pub sort: Option<Sort<DocumentSort>>,
}

impl Default for DocumentFilter {
    fn default() -> Self {
        Self {
            source: None,
            key: None,
            latest: false,
            after: None,
            limit: 100,
            sort: None,
        }
    }
}

/// What a list of documents can be sorted by. Each ends with the ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentSort {
    Id,
    /// The key, whatever its case, then the source and the versions of the
    /// key the oldest first.
    Key,
    /// The source, whatever its case, then the key as `Key` does.
    Source,
    /// How long the text is.
    Length,
    /// When the document was loaded.
    Loaded,
}

impl DocumentSort {
    fn keys(self) -> Vec<Key> {
        let mut keys = match self {
            Self::Id => Vec::new(),
            Self::Key => [
                &Key::text("d.source_key")[..],
                &Key::text("d.source_id"),
                &[Key::of("d.load_run")],
            ]
            .concat(),
            Self::Source => [
                &Key::text("d.source_id")[..],
                &Key::text("d.source_key"),
                &[Key::of("d.load_run")],
            ]
            .concat(),
            Self::Length => vec![Key::of("length(d.text)")],
            Self::Loaded => vec![Key::of("d.loaded_at"), Key::of("d.load_run")],
        };
        keys.push(Key::of("d.document_id"));
        keys
    }
}

const DOCUMENT: &str = "
    SELECT d.document_id::VARCHAR, d.source_id, d.source_key, d.uri, length(d.text),
           epoch_us(d.loaded_at), d.load_run, d.supersedes::VARCHAR,
           later.document_id::VARCHAR
    FROM spool.document d
    LEFT JOIN spool.document later ON later.supersedes = d.document_id";

fn document_entry_from_row(row: &duckdb::Row<'_>) -> Result<DocumentEntry> {
    let id = |at: usize| -> Result<Option<DocumentId>> {
        row.get::<_, Option<String>>(at)?
            .as_deref()
            .map(parse_stored)
            .transpose()
    };
    Ok(DocumentEntry {
        id: parse_stored(&row.get::<_, String>(0)?)?,
        source: row.get(1)?,
        key: row.get(2)?,
        uri: row.get(3)?,
        length: row.get::<_, i64>(4)? as u64,
        text: None,
        loaded_at: format_rfc3339(row.get(5)?),
        load_run: RunId(row.get::<_, i64>(6)? as u64),
        supersedes: id(7)?,
        superseded_by: id(8)?,
        extractions: Vec::new(),
        reading: None,
    })
}

/// Fills in which extractors have read each of the documents.
fn add_extractions(conn: &Connection, documents: &mut [DocumentEntry]) -> Result<()> {
    if documents.is_empty() {
        return Ok(());
    }
    let ids: Vec<DocumentId> = documents.iter().map(|document| document.id).collect();
    let mut statement = conn.prepare(
        "SELECT l.ord, e.extractor, e.extractor_version, e.run_id, e.mentions, e.failure
         FROM arrow(?) l
         JOIN spool.extraction e ON e.document_id = l.id::UUID
         ORDER BY e.run_id",
    )?;
    let mut rows = statement.query([&id_list(&ids)?])?;
    while let Some(row) = rows.next()? {
        let at = row.get::<_, i64>(0)? as usize;
        documents[at].extractions.push(DocumentExtraction {
            extractor: row.get(1)?,
            version: row.get(2)?,
            run: RunId(row.get::<_, i64>(3)? as u64),
            mentions: row.get::<_, i64>(4)? as u64,
            failure: row.get(5)?,
        });
    }
    Ok(())
}

/// One document with its text, or `None` if the store holds none by that ID.
pub(crate) fn get(conn: &Connection, id: DocumentId) -> Result<Option<DocumentEntry>> {
    let mut found = {
        let mut statement = conn.prepare(&format!("{DOCUMENT} WHERE d.document_id = ?::UUID"))?;
        let mut rows = statement.query([id.to_string()])?;
        match rows.next()? {
            Some(row) => document_entry_from_row(row)?,
            None => return Ok(None),
        }
    };
    found.text = Some(conn.query_row(
        "SELECT text FROM spool.document WHERE document_id = ?::UUID",
        [id.to_string()],
        |row| row.get(0),
    )?);
    add_extractions(conn, std::slice::from_mut(&mut found))?;
    Ok(Some(found))
}

/// Documents without their text, ordered by ID or as the filter says.
pub(crate) fn list(conn: &Connection, filter: &DocumentFilter) -> Result<Vec<DocumentEntry>> {
    let mut found = Vec::new();
    {
        let sql = match filter.sort {
            None => format!(
                "{DOCUMENT}
                 WHERE ($1::VARCHAR IS NULL OR d.source_id = $1::VARCHAR)
                   AND ($2::VARCHAR IS NULL OR d.source_key = $2::VARCHAR)
                   AND (NOT $3 OR later.document_id IS NULL)
                   AND ($4::VARCHAR IS NULL OR d.document_id > $4::UUID)
                 ORDER BY d.document_id
                 LIMIT $5"
            ),
            Some(sort) => {
                let keyed = Keyed {
                    before: None,
                    from: "spool.document d".to_owned(),
                    id: "d.document_id".to_owned(),
                    keys: sort.by.keys(),
                    direction: sort.direction,
                };
                format!(
                    "{} {DOCUMENT} {}
                     WHERE ($1::VARCHAR IS NULL OR d.source_id = $1::VARCHAR)
                       AND ($2::VARCHAR IS NULL OR d.source_key = $2::VARCHAR)
                       AND (NOT $3 OR later.document_id IS NULL)
                       AND {}
                     ORDER BY {}
                     LIMIT $5",
                    keyed.with("$4::UUID"),
                    keyed.join("d.document_id"),
                    keyed.after("$4::VARCHAR IS NOT NULL"),
                    keyed.order_by(),
                )
            }
        };
        let mut statement = conn.prepare(&sql)?;
        let mut rows = statement.query(params![
            filter.source,
            filter.key,
            filter.latest,
            filter.after.map(|id| id.to_string()),
            filter.limit.min(i64::MAX as u64) as i64,
        ])?;
        while let Some(row) = rows.next()? {
            found.push(document_entry_from_row(row)?);
        }
    }
    add_extractions(conn, &mut found)?;
    Ok(found)
}

/// How many documents the store holds, counting each version.
pub(crate) fn count(conn: &Connection) -> Result<u64> {
    let count: i64 = conn.query_row("SELECT count(*) FROM spool.document", [], |row| row.get(0))?;
    Ok(count as u64)
}
