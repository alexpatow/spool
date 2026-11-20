//! Records: one record, the lists of them, and the SQL that finds a
//! record by the words it holds.

use std::collections::BTreeMap;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{DocumentId, EntityType, IdentityId, RecordId, RunId, StateError};

use super::identities::identity_label;
use super::{entity_type, limit, uuid_list, words};
use crate::error::Result;
use crate::order::{Key, Keyed, Sort};
use crate::store::parse;
use crate::time::rfc3339;

/// A record as it was loaded, and the identity it belongs to now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecordEntry {
    pub id: RecordId,
    pub entity_type: EntityType,
    pub source: String,
    /// The row's key within its source, if it had one.
    pub key: Option<String>,
    /// The attributes exactly as received.
    pub payload: serde_json::Value,
    /// What a person would call it: its value of the feature its type
    /// names as the label. `None` when the type names none, or the record
    /// has no value there.
    #[serde(default)]
    pub label: Option<String>,
    /// Where in the source the record was found, if the source said.
    pub evidence: Option<serde_json::Value>,
    pub loaded_at: String,
    pub load_run: RunId,
    /// `None` while no changeset has placed it.
    pub identity: Option<IdentityId>,
    /// The version of the row this one replaced: what the source held under
    /// the same key before.
    #[serde(default)]
    pub supersedes: Option<RecordId>,
    /// The version that replaced this one, if the source has sent the row
    /// again with other content.
    #[serde(default)]
    pub superseded_by: Option<RecordId>,
    /// The retract run in which its source took the row back, while that
    /// stands. Such a record has no identity and is given none.
    #[serde(default)]
    pub retracted: Option<RunId>,
    /// For a mention: each reading that found it in this version, oldest
    /// first, with the version of the extractor that read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub found_by: Vec<FoundBy>,
}

/// A reading of a document that found a mention.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FoundBy {
    pub extractor: String,
    /// The hash of the extractor's files when it read.
    pub version: String,
    /// The extract run.
    pub run: RunId,
}

impl RecordEntry {
    /// What to call the record in a sentence: `source:key`, which is
    /// how the people who sent it know it. A mention is
    /// `source:document[start-end]` with the document's own key, since the
    /// key of a mention names its document by ID. A record without a
    /// key is its ID.
    pub fn name(&self) -> String {
        let mention = self.evidence.as_ref().and_then(|evidence| {
            evidence.get("document")?;
            Some(format!(
                "{}:{}[{}-{}]",
                self.source,
                evidence.get("key")?.as_str()?,
                evidence.get("start")?.as_u64()?,
                evidence.get("end")?.as_u64()?
            ))
        });
        match (mention, &self.key) {
            (Some(mention), _) => mention,
            (None, Some(key)) => format!("{}:{key}", self.source),
            (None, None) => self.id.to_string(),
        }
    }
}

/// Which records to list. Results are ordered by source and key, as text:
/// the rows of one source are together, and the versions of one row follow
/// each other, the oldest first. The mentions of a document are together
/// too, in the order they stand in its text. Records without a key come
/// last in their source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordFilter {
    pub entity_type: Option<EntityType>,
    pub source: Option<String>,
    pub key: Option<String>,
    /// Only the mentions an extractor found in this document.
    pub document: Option<DocumentId>,
    /// Only the members this identity has now. An identity that was merged
    /// away has none: its members are those of the one that stands for it.
    pub identity: Option<IdentityId>,
    /// Only those that hold every word of this text, in a value of the
    /// payload, in the source's name or in the key. Letters match whatever
    /// their case and accents.
    pub q: Option<String>,
    /// Only those that wait for an identity: they have none, and their
    /// source has not taken them back.
    pub unassigned: bool,
    /// Only the version of each row or mention that nothing has replaced,
    /// and no mention found in a version of a document that a later one
    /// replaced.
    pub latest: bool,
    /// Only those that come after this record in the list: the last one of
    /// the page before. A record the store does not hold is refused.
    pub after: Option<RecordId>,
    pub limit: u64,
    /// Another order than source and key.
    pub sort: Option<Sort<RecordSort>>,
}

impl Default for RecordFilter {
    fn default() -> Self {
        Self {
            entity_type: None,
            source: None,
            key: None,
            document: None,
            identity: None,
            q: None,
            unassigned: false,
            latest: false,
            after: None,
            limit: 100,
            sort: None,
        }
    }
}

impl RecordFilter {
    /// The records a source holds under one key. There is more than one
    /// when the row changed between loads.
    pub fn keyed(source: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            source: Some(source.into()),
            key: Some(key.into()),
            ..Self::default()
        }
    }
}

fn json(text: String) -> Result<serde_json::Value> {
    Ok(serde_json::from_str(&text).map_err(StateError::new)?)
}

const RECORD: &str = "
    SELECT o.record_id::VARCHAR, o.entity_type, o.source_id, o.source_key,
           o.payload::VARCHAR, o.evidence::VARCHAR, epoch_us(o.loaded_at), o.load_run,
           m.identity_id::VARCHAR, o.supersedes::VARCHAR, later.record_id::VARCHAR,
           named.label, taken.retract_run,
           (SELECT to_json(list({'extractor': f.extractor, 'version': f.extractor_version,
                                 'run': f.run_id} ORDER BY f.run_id))
            FROM spool.extraction_found f WHERE f.record_id = o.record_id)::VARCHAR
    FROM spool.record o
    LEFT JOIN spool.membership m ON m.record_id = o.record_id AND m.to_seq IS NULL
    LEFT JOIN spool.record later ON later.supersedes = o.record_id
    LEFT JOIN spool.record_label named ON named.record_id = o.record_id
    LEFT JOIN spool.retraction taken
      ON taken.record_id = o.record_id AND taken.restored_run IS NULL";

fn record_from_row(row: &duckdb::Row<'_>) -> Result<RecordEntry> {
    fn id<T>(row: &duckdb::Row<'_>, at: usize) -> Result<Option<T>>
    where
        T: std::str::FromStr,
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        row.get::<_, Option<String>>(at)?
            .as_deref()
            .map(parse)
            .transpose()
    }
    Ok(RecordEntry {
        id: parse(&row.get::<_, String>(0)?)?,
        entity_type: entity_type(row.get(1)?)?,
        source: row.get(2)?,
        key: row.get(3)?,
        label: row.get(11)?,
        payload: json(row.get(4)?)?,
        evidence: row.get::<_, Option<String>>(5)?.map(json).transpose()?,
        loaded_at: rfc3339(row.get(6)?),
        load_run: RunId(row.get::<_, i64>(7)? as u64),
        identity: id(row, 8)?,
        supersedes: id(row, 9)?,
        superseded_by: id(row, 10)?,
        retracted: row.get::<_, Option<i64>>(12)?.map(|run| RunId(run as u64)),
        found_by: match row.get::<_, Option<String>>(13)? {
            Some(found) => serde_json::from_str(&found).map_err(StateError::new)?,
            None => Vec::new(),
        },
    })
}

pub(crate) fn record(conn: &Connection, id: RecordId) -> Result<Option<RecordEntry>> {
    let mut statement = conn.prepare(&format!("{RECORD} WHERE o.record_id = ?::UUID"))?;
    let mut rows = statement.query([id.to_string()])?;
    match rows.next()? {
        Some(row) => Ok(Some(record_from_row(row)?)),
        None => Ok(None),
    }
}

/// SQL for whether the record `o` holds every word of the JSON list in
/// the parameter `words`, or the parameter is null. A word is held if it is
/// in a value of the payload, in the source's name or in the key, whatever
/// the case and the accents of its letters.
fn holds_words(words: &str) -> String {
    let matches = contains_words(
        RECORD_SEARCH_TEXT,
        &format!("from_json({words}::VARCHAR, '[\"VARCHAR\"]')"),
    );
    format!("({words}::VARCHAR IS NULL OR {matches})")
}

pub(super) const RECORD_SEARCH_TEXT: &str = "concat_ws(' ', o.source_id, o.source_key,
    array_to_string(json_extract_string(o.payload, '$.*'), ' '))";

pub(super) fn contains_words(value: &str, words: &str) -> String {
    format!(
        "len(list_filter({words}, lambda word:
           NOT coalesce(contains(lower(strip_accents({value})), lower(strip_accents(word))), false))) = 0"
    )
}

/// Where the record `o` stands in a list of records, as SQL: a row of
/// values none of which is null, so that two of them compare the way the
/// list is ordered.
///
/// A mention stands where [`RecordEntry::name`] puts it, under the key of
/// its document and then at its place there. Its own key begins with the
/// ID of the document and ends with the span as text, which would put the
/// documents in no order a reader knows and `[119-142]` before `[54-69]`.
const RECORD_ORDER: [&str; 8] = [
    "o.source_id",
    "o.source_key IS NULL",
    MENTION_KEY,
    "coalesce(CASE WHEN json_extract_string(o.evidence, '$.document') IS NOT NULL \
              THEN TRY_CAST(json_extract_string(o.evidence, '$.start') AS BIGINT) END, 0)",
    "coalesce(o.source_key, '')",
    "o.entity_type",
    "o.load_run",
    "o.record_id",
];

/// What a record is known by in its source: its key, or for a mention the
/// key of its document.
const MENTION_KEY: &str = "coalesce(CASE WHEN json_extract_string(o.evidence, '$.document') IS NOT NULL \
                   THEN json_extract_string(o.evidence, '$.key') END, o.source_key, '')";

/// What a list of records can be sorted by. Each ends with the order of the
/// list when nothing is chosen, and that ends with the ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordSort {
    Id,
    /// The source, whatever its case, and then the key, as the list is
    /// ordered when nothing is chosen.
    Source,
    /// The key, or the document a mention was found in, whatever its case.
    Key,
    /// The label, whatever its case. Those without one last.
    Label,
    EntityType,
    /// The label of the identity it belongs to. Those without one last.
    Identity,
}

impl RecordSort {
    fn keys(self) -> Vec<Key> {
        let order = || {
            RECORD_ORDER.iter().map(|sql| {
                if *sql == "o.source_key IS NULL" {
                    Key::missing_last("o.source_key")
                } else {
                    Key::of(*sql)
                }
            })
        };
        match self {
            Self::Id => vec![Key::of("o.record_id")],
            Self::Source => Key::text("o.source_id")
                .into_iter()
                .chain(order())
                .collect(),
            Self::Key => std::iter::once(Key::missing_last("o.source_key"))
                .chain(Key::text(MENTION_KEY))
                .chain(order())
                .collect(),
            Self::Label => Key::optional_text("named.label")
                .into_iter()
                .chain(order())
                .collect(),
            Self::EntityType => Key::text("o.entity_type")
                .into_iter()
                .chain(order())
                .collect(),
            Self::Identity => Key::optional_text("il.label")
                .into_iter()
                .chain([
                    Key::missing_last("m.identity_id"),
                    Key::of("coalesce(m.identity_id::VARCHAR, '')"),
                ])
                .chain(order())
                .collect(),
        }
    }
}

/// SQL for whether the record `o` is not a mention found in a version of
/// a document that a later version replaced. A mention's key names its
/// document, so one found in the new version does not replace one found in
/// the old: they are records of their own.
pub(crate) const DOCUMENT_STANDS: &str = "NOT EXISTS (
       SELECT 1 FROM spool.document replaced
       WHERE replaced.supersedes
             = TRY_CAST(json_extract_string(o.evidence, '$.document') AS UUID))";

pub(crate) fn records(conn: &Connection, filter: &RecordFilter) -> Result<Vec<RecordEntry>> {
    // A page continues from where a record stands, so the record has to be
    // one the store holds. Records are never deleted, so the last one of a
    // page always is.
    if let Some(after) = filter.after {
        let held: bool = conn.query_row(
            "SELECT count(*) > 0 FROM spool.record WHERE record_id = ?::UUID",
            [after.to_string()],
            |row| row.get(0),
        )?;
        if !held {
            return Err(crate::error::Error::UnknownRecord(after));
        }
    }
    let filters = format!(
        "($1::VARCHAR IS NULL OR o.entity_type = $1::VARCHAR)
           AND ($2::VARCHAR IS NULL OR o.source_id = $2::VARCHAR)
           AND ($3::VARCHAR IS NULL OR o.source_key = $3::VARCHAR)
           AND (NOT $5::BOOLEAN OR (m.identity_id IS NULL AND taken.record_id IS NULL))
           AND ($7::VARCHAR IS NULL
                OR json_extract_string(o.evidence, '$.document') = $7::VARCHAR)
           AND ($8::VARCHAR IS NULL OR m.identity_id = $8::UUID)
           AND (NOT $10::BOOLEAN OR (later.record_id IS NULL AND {}))
           AND {}",
        DOCUMENT_STANDS,
        holds_words("$9")
    );
    let sql = match filter.sort {
        None => {
            let order = RECORD_ORDER.join(", ");
            format!(
                "{RECORD}
                 WHERE {filters}
                   AND ($4::VARCHAR IS NULL OR row({order}) > (
                         SELECT row({order}) FROM spool.record o
                         WHERE o.record_id = $4::UUID))
                 ORDER BY {order}
                 LIMIT $6"
            )
        }
        Some(sort) => {
            let keyed = Keyed {
                before: (sort.by == RecordSort::Identity).then(|| identity_label("")),
                from: format!(
                    "spool.record o
                     LEFT JOIN spool.membership m ON m.record_id = o.record_id AND m.to_seq IS NULL
                     LEFT JOIN spool.record_label named ON named.record_id = o.record_id{}",
                    if sort.by == RecordSort::Identity {
                        "\n LEFT JOIN identity_label il ON il.identity_id = m.identity_id"
                    } else {
                        ""
                    }
                ),
                id: "o.record_id".to_owned(),
                keys: sort.by.keys(),
                direction: sort.direction,
            };
            format!(
                "{} {RECORD} {}
                 WHERE {filters} AND {}
                 ORDER BY {}
                 LIMIT $6",
                keyed.with("$4::UUID"),
                keyed.join("o.record_id"),
                keyed.after("$4::VARCHAR IS NOT NULL"),
                keyed.order_by(),
            )
        }
    };
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement.query(params![
        filter.entity_type.as_ref().map(EntityType::as_str),
        filter.source,
        filter.key,
        filter.after.map(|id| id.to_string()),
        filter.unassigned,
        limit(filter.limit),
        filter.document.map(|id| id.to_string()),
        filter.identity.map(|id| id.to_string()),
        words(filter.q.as_deref()),
        filter.latest,
    ])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(record_from_row(row)?);
    }
    Ok(found)
}

/// The records with the given IDs, in the order they were asked for.
/// One the store does not hold is left out.
pub(crate) fn records_by_id(conn: &Connection, ids: &[RecordId]) -> Result<Vec<RecordEntry>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(&format!(
        "{RECORD} WHERE list_contains({}, o.record_id)",
        uuid_list(ids)
    ))?;
    let mut rows = statement.query([])?;
    let mut found = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let record = record_from_row(row)?;
        found.insert(record.id, record);
    }
    Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
}

/// At most `most` of the members an identity has now: those among `first`
/// before the others, and of the others the ones loaded first.
pub(crate) fn members(
    conn: &Connection,
    identity: IdentityId,
    first: &[RecordId],
    most: u64,
) -> Result<Vec<RecordEntry>> {
    let mut statement = conn.prepare(&format!(
        "{RECORD}
         WHERE m.identity_id = $1::UUID
         ORDER BY list_contains({}, o.record_id) DESC,
                  o.loaded_at, o.load_run, o.record_id
         LIMIT $2",
        uuid_list(first)
    ))?;
    let mut rows = statement.query(params![identity.to_string(), limit(most)])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(record_from_row(row)?);
    }
    Ok(found)
}
