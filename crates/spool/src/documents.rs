//! Documents, and the mentions an extractor finds in them.
//!
//! A document is text a source holds under a key. The store keeps it so
//! that a record extracted from it can show where it was found, and
//! so that an extractor can be run again without the source being asked
//! again. A mention becomes a record whose key is its locator:
//!
//! ```text
//! <document id>#<entity type>:<start>-<end>
//! ```
//!
//! `start` and `end` count Unicode code points from the start of the text,
//! and `end` is the position after the last one. The document ID covers the
//! text, so a locator never points into two versions of a document.

use std::collections::{BTreeMap, HashMap, HashSet};

use duckdb::arrow::record_batch::RecordBatch;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use spool_core::{DocumentId, RecordId, RunId};

use crate::batch::{Column, batch, id_list};
use crate::error::{Error, Result};
use crate::order::{Key, Keyed, Sort};
use crate::project::Extractor;
use crate::store::parse;
use crate::time::rfc3339;
use crate::versions;

/// A document as a source hands it over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NewDocument {
    /// The document's key within its source: a path, a URL, a message ID.
    pub key: String,
    /// Where the source keeps the document, if that is worth showing to
    /// someone who reads a mention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// The document as text. Turning a PDF or a web page into text is the
    /// caller's work.
    pub text: String,
}

/// One version of a document as the store holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DocumentEntry {
    pub id: DocumentId,
    pub source: String,
    pub key: String,
    pub uri: Option<String>,
    /// How long the text is, in Unicode code points.
    pub length: u64,
    /// The text. Left out of a list of documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub loaded_at: String,
    pub load_run: RunId,
    /// The version of the document this one replaced: what the source held
    /// under the same key before.
    pub supersedes: Option<DocumentId>,
    /// The version that replaced this one.
    pub superseded_by: Option<DocumentId>,
    /// Each time an extractor read this version, oldest first.
    pub extractions: Vec<DocumentExtraction>,
}

/// One reading of a document by an extractor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DocumentExtraction {
    pub extractor: String,
    /// The hash of the extractor's files when it ran.
    pub version: String,
    /// The extract run.
    pub run: RunId,
    /// How many mentions it found in the document.
    pub mentions: u64,
}

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

/// Which documents an extractor is to read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractOptions {
    /// The name of one of the project's extractors.
    pub extractor: String,
    /// Read only the documents of this source.
    pub source: Option<String>,
    /// Read the documents this extractor has read before as well. Without
    /// it an extraction reads only what the extractor has not seen, whatever
    /// version of the extractor saw it.
    pub again: bool,
    /// Read at most this many documents, oldest first.
    pub limit: Option<u64>,
}

impl ExtractOptions {
    pub fn with(extractor: impl Into<String>) -> Self {
        Self {
            extractor: extractor.into(),
            ..Self::default()
        }
    }
}

/// Names the extractor of an extract run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExtractorRef {
    pub name: String,
    /// The hash of the extractor's files.
    pub version: String,
}

impl From<&Extractor> for ExtractorRef {
    fn from(extractor: &Extractor) -> Self {
        Self {
            name: extractor.name.clone(),
            version: extractor.hash.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExtractReport {
    pub run: RunId,
    /// Documents the extractor was given.
    pub documents: u64,
    /// Mentions it found.
    pub mentions: u64,
    /// Records created.
    pub inserted: u64,
    /// Mentions that were already known: the same place in the same
    /// document, with the same attributes.
    pub duplicates: u64,
    /// Records created as a new version of a mention that was known
    /// with other attributes.
    pub superseded: u64,
    /// The load run the records were stored under. There is none
    /// when nothing was found.
    pub load_run: Option<RunId>,
}

impl ExtractReport {
    pub(crate) fn empty(run: RunId) -> Self {
        Self {
            run,
            documents: 0,
            mentions: 0,
            inserted: 0,
            duplicates: 0,
            superseded: 0,
            load_run: None,
        }
    }
}

/// The documents of one call, as a batch of `ord`, `document_id`,
/// `source_key`, `uri` and `text`. `document_id` is the ID derived from the
/// text.
pub(crate) fn stage(source: &str, documents: &[NewDocument]) -> Result<Option<RecordBatch>> {
    if source.is_empty() {
        return Err(Error::Documents("the source needs a name".to_owned()));
    }
    let (mut ords, mut ids, mut keys, mut uris, mut texts) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (index, document) in documents.iter().enumerate() {
        let fail = |message: &str| Error::Documents(format!("document {}: {message}", index + 1));
        if document.key.is_empty() {
            return Err(fail("it has no key"));
        }
        if document.text.is_empty() {
            return Err(fail("it has no text"));
        }
        let id = DocumentId::derive(source, &document.key, &document.text);
        ords.push(index as i64);
        ids.push(Some(id.to_string()));
        keys.push(Some(document.key.clone()));
        uris.push(document.uri.clone().filter(|uri| !uri.is_empty()));
        texts.push(Some(document.text.clone()));
    }
    if ids.is_empty() {
        return Ok(None);
    }
    Ok(Some(batch(vec![
        ("ord", Column::Integer(ords)),
        ("document_id", Column::Text(ids)),
        ("source_key", Column::Text(keys)),
        ("uri", Column::Text(uris)),
        ("text", Column::Text(texts)),
    ])?))
}

/// Stores staged documents and returns how many were new. A text under a
/// key the source has used before, other than the one the key holds now, is
/// the next version of that document, and supersedes the latest version the
/// store holds, or the one before it in this call. That is so for a text
/// the key held earlier too. See [`versions`](crate::versions).
pub(crate) fn insert(
    conn: &Connection,
    source: &str,
    now: i64,
    run: RunId,
    staged: RecordBatch,
) -> Result<u64> {
    let mut statement = conn.prepare(
        "SELECT s.ord, known.document_id IS NOT NULL,
                latest.document_id::VARCHAR, latest.supersedes::VARCHAR
         FROM arrow($2) s
         LEFT JOIN spool.document known ON known.document_id = s.document_id::UUID
         LEFT JOIN (
           SELECT d.source_key, d.document_id, d.supersedes
           FROM spool.document d
           WHERE d.source_id = $1
             AND NOT EXISTS (
               SELECT 1 FROM spool.document later WHERE later.supersedes = d.document_id)
         ) latest ON latest.source_key = s.source_key
         WHERE known.document_id IS NOT NULL OR latest.document_id IS NOT NULL",
    )?;
    let held = statement.query(params![
        source,
        &ArrowBatchRegistration::new(staged.clone())
    ])?;
    let versions = versions::of_staged(&staged, "document_id", held, DocumentId::held_again)?;
    if versions.is_empty() {
        return Ok(0);
    }
    let stored = versions::to_store(
        &staged,
        "document_id",
        &versions,
        &["source_key", "uri", "text"],
    )?;
    let inserted = conn.execute(
        "INSERT INTO spool.document
         SELECT n.document_id::UUID, $1, n.source_key, n.supersedes::UUID,
                n.uri, n.text, make_timestamptz($2), $3
         FROM arrow($4) n",
        params![
            source,
            now,
            run.0 as i64,
            &ArrowBatchRegistration::new(stored)
        ],
    )?;
    Ok(inserted as u64)
}

const DOCUMENT: &str = "
    SELECT d.document_id::VARCHAR, d.source_id, d.source_key, d.uri, length(d.text),
           epoch_us(d.loaded_at), d.load_run, d.supersedes::VARCHAR,
           later.document_id::VARCHAR
    FROM spool.document d
    LEFT JOIN spool.document later ON later.supersedes = d.document_id";

fn entry(row: &duckdb::Row<'_>) -> Result<DocumentEntry> {
    let id = |at: usize| -> Result<Option<DocumentId>> {
        row.get::<_, Option<String>>(at)?
            .as_deref()
            .map(parse)
            .transpose()
    };
    Ok(DocumentEntry {
        id: parse(&row.get::<_, String>(0)?)?,
        source: row.get(1)?,
        key: row.get(2)?,
        uri: row.get(3)?,
        length: row.get::<_, i64>(4)? as u64,
        text: None,
        loaded_at: rfc3339(row.get(5)?),
        load_run: RunId(row.get::<_, i64>(6)? as u64),
        supersedes: id(7)?,
        superseded_by: id(8)?,
        extractions: Vec::new(),
    })
}

/// Fills in which extractors have read each of the documents.
fn add_extractions(conn: &Connection, documents: &mut [DocumentEntry]) -> Result<()> {
    if documents.is_empty() {
        return Ok(());
    }
    let ids: Vec<DocumentId> = documents.iter().map(|document| document.id).collect();
    let mut statement = conn.prepare(
        "SELECT l.ord, e.extractor, e.extractor_version, e.run_id, e.mentions
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
            Some(row) => entry(row)?,
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
            found.push(entry(row)?);
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

/// A document an extractor was given.
pub(crate) struct TaskDocument {
    pub source: String,
    pub key: String,
    pub uri: Option<String>,
    pub text: String,
}

/// A row of what an extractor wrote, before anything about it is known to
/// be right.
pub(crate) struct RawMention {
    pub document: Option<String>,
    pub entity_type: Option<String>,
    pub start: Option<i64>,
    pub end: Option<i64>,
    /// JSON text.
    pub attributes: Option<String>,
}

/// A mention that passed every check, as the record it becomes.
#[derive(Debug)]
pub(crate) struct Mention {
    pub document: DocumentId,
    pub entity_type: String,
    pub source: String,
    pub record: RecordId,
    /// The locator.
    pub key: String,
    /// The attributes in canonical form.
    pub payload: String,
    pub evidence: String,
}

/// The key of the record a mention becomes.
pub fn locator(document: DocumentId, entity_type: &str, start: u64, end: u64) -> String {
    format!("{document}#{entity_type}:{start}-{end}")
}

/// Checks what an extractor wrote against the documents it was given, and
/// turns each mention into a record. One mention that is wrong
/// refuses them all: an extractor that points outside its document, or
/// names a type the project does not declare, is not to be half believed.
pub(crate) fn stage_mentions(
    raw: &[RawMention],
    documents: &HashMap<DocumentId, TaskDocument>,
    declared: &dyn Fn(&str) -> bool,
    extractor: &Extractor,
) -> Result<Vec<Mention>> {
    // Where each code point of a document begins, worked out for the
    // documents that have mentions.
    let mut boundaries: HashMap<DocumentId, Vec<usize>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut staged = Vec::with_capacity(raw.len());

    for (index, mention) in raw.iter().enumerate() {
        let fail = |message: String| Error::Mentions(format!("row {}: {message}", index + 1));
        let need = |name: &str| fail(format!("it has no `{name}`"));

        let named = mention
            .document
            .as_deref()
            .ok_or_else(|| need("document_id"))?;
        let found = named
            .parse::<DocumentId>()
            .ok()
            .and_then(|id| documents.get(&id).map(|document| (id, document)));
        let Some((id, document)) = found else {
            return Err(fail(format!(
                "`{named}` is not a document the extractor was given"
            )));
        };
        let entity_type = mention
            .entity_type
            .as_deref()
            .ok_or_else(|| need("entity_type"))?;
        if !declared(entity_type) {
            return Err(fail(format!(
                "the project declares no entity type `{entity_type}`"
            )));
        }

        let starts = boundaries.entry(id).or_insert_with(|| {
            document
                .text
                .char_indices()
                .map(|(at, _)| at)
                .chain(std::iter::once(document.text.len()))
                .collect()
        });
        let length = (starts.len() - 1) as i64;
        let start = mention.start.ok_or_else(|| need("start"))?;
        let end = mention.end.ok_or_else(|| need("end"))?;
        if start < 0 || start >= end || end > length {
            return Err(fail(format!(
                "{start}-{end} is not a span of document `{}`, which is {length} long; \
                 `start` and `end` count Unicode code points, and `end` comes after the last one",
                document.key
            )));
        }
        let (start, end) = (start as usize, end as usize);
        if !seen.insert((id, entity_type, start, end)) {
            return Err(fail(format!(
                "document `{}` already has a mention of a `{entity_type}` at {start}-{end}",
                document.key
            )));
        }

        let payload = canonical(mention.attributes.as_deref()).map_err(fail)?;
        let key = locator(id, entity_type, start as u64, end as u64);
        let mut evidence = json!({
            "document": id,
            "key": document.key,
            "start": start,
            "end": end,
            "text": &document.text[starts[start]..starts[end]],
            "extractor": extractor.name,
            "version": extractor.hash,
        });
        if let Some(uri) = &document.uri {
            evidence["uri"] = json!(uri);
        }
        staged.push(Mention {
            document: id,
            entity_type: entity_type.to_owned(),
            source: document.source.clone(),
            record: RecordId::derive(&document.source, Some(&key), &payload),
            key,
            payload,
            evidence: evidence.to_string(),
        });
    }
    Ok(staged)
}

/// A mention's attributes in the form a row's payload has: an object of
/// text, booleans and numbers, ordered by name, without the attributes that
/// have no value. A whole number is an integer however it was written.
fn canonical(attributes: Option<&str>) -> Result<String, String> {
    let parsed: Value = match attributes {
        None => Value::Null,
        Some(text) => serde_json::from_str(text)
            .map_err(|error| format!("`attributes` is not valid JSON: {error}"))?,
    };
    let object = match parsed {
        Value::Null => serde_json::Map::new(),
        Value::Object(object) => object,
        _ => return Err("`attributes` must be a JSON object".to_owned()),
    };
    let mut payload = BTreeMap::new();
    for (name, value) in object {
        let value = match value {
            Value::Null => continue,
            Value::String(_) | Value::Bool(_) => value,
            Value::Number(number) => match number.as_f64() {
                Some(float)
                    if !number.is_i64()
                        && !number.is_u64()
                        && float.fract() == 0.0
                        && float.abs() < 9_007_199_254_740_992.0 =>
                {
                    Value::from(float as i64)
                }
                _ => Value::Number(number),
            },
            Value::Array(_) | Value::Object(_) => {
                return Err(format!(
                    "attribute `{name}` is nested; an attribute is text, a boolean or a number"
                ));
            }
        };
        payload.insert(name, value);
    }
    serde_json::to_string(&payload)
        .map_err(|error| format!("the attributes cannot be encoded: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Runtime;

    fn extractor() -> Extractor {
        Extractor {
            name: "names".to_owned(),
            runtime: Runtime::Python,
            file: "extractors/names.py".to_owned(),
            hash: "abc".to_owned(),
        }
    }

    fn given(text: &str) -> (DocumentId, HashMap<DocumentId, TaskDocument>) {
        let id = DocumentId::derive("news", "a.txt", text);
        let document = TaskDocument {
            source: "news".to_owned(),
            key: "a.txt".to_owned(),
            uri: Some("https://example.test/a".to_owned()),
            text: text.to_owned(),
        };
        (id, HashMap::from([(id, document)]))
    }

    fn raw(document: DocumentId, start: i64, end: i64, attributes: &str) -> RawMention {
        RawMention {
            document: Some(document.to_string()),
            entity_type: Some("company".to_owned()),
            start: Some(start),
            end: Some(end),
            attributes: Some(attributes.to_owned()),
        }
    }

    fn stage_one(text: &str, mention: impl Fn(DocumentId) -> RawMention) -> Result<Vec<Mention>> {
        let (id, documents) = given(text);
        stage_mentions(
            &[mention(id)],
            &documents,
            &|name| name == "company",
            &extractor(),
        )
    }

    fn refused(text: &str, mention: impl Fn(DocumentId) -> RawMention) -> String {
        match stage_one(text, mention) {
            Err(Error::Mentions(message)) => message,
            other => panic!("expected the mention to be refused, got {other:?}"),
        }
    }

    #[test]
    fn a_span_counts_code_points_and_the_evidence_quotes_it() {
        // An accent is two bytes, and the rocket is four and two UTF-16 units.
        let text = "🚀 Société Générale a ouvert un bureau.";
        let staged =
            stage_one(text, |id| raw(id, 2, 18, r#"{"name":"Société Générale"}"#)).unwrap();
        let mention = &staged[0];
        let (id, _) = given(text);
        assert_eq!(mention.key, format!("{id}#company:2-18"));
        assert_eq!(mention.payload, r#"{"name":"Société Générale"}"#);
        let evidence: Value = serde_json::from_str(&mention.evidence).unwrap();
        assert_eq!(
            evidence,
            json!({
                "document": id,
                "key": "a.txt",
                "uri": "https://example.test/a",
                "start": 2,
                "end": 18,
                "text": "Société Générale",
                "extractor": "names",
                "version": "abc",
            })
        );
        assert_eq!(
            mention.record,
            RecordId::derive("news", Some(&mention.key), &mention.payload)
        );
        // The whole text is a span, and its end is the text's length.
        let length = text.chars().count() as i64;
        assert!(stage_one(text, |id| raw(id, 0, length, "{}")).is_ok());
    }

    #[test]
    fn a_mention_outside_its_document_is_refused() {
        let text = "Acme opened an office.";
        for (start, end) in [(-1, 4), (4, 4), (5, 4), (0, 23), (22, 23)] {
            let message = refused(text, |id| raw(id, start, end, "{}"));
            assert!(
                message.starts_with("row 1: ") && message.contains("is not a span"),
                "{message}"
            );
        }
    }

    #[test]
    fn a_mention_must_name_a_document_of_the_task_and_a_declared_type() {
        let text = "Acme opened an office.";
        let other = DocumentId::derive("news", "b.txt", text);
        let message = refused(text, |_| raw(other, 0, 4, "{}"));
        assert!(
            message.contains("not a document the extractor was given"),
            "{message}"
        );
        let message = refused(text, |id| RawMention {
            document: Some("somewhere".to_owned()),
            ..raw(id, 0, 4, "{}")
        });
        assert!(
            message.contains("not a document the extractor was given"),
            "{message}"
        );

        let message = refused(text, |id| RawMention {
            entity_type: Some("person".to_owned()),
            ..raw(id, 0, 4, "{}")
        });
        assert!(message.contains("no entity type `person`"), "{message}");
        let message = refused(text, |id| RawMention {
            start: None,
            ..raw(id, 0, 4, "{}")
        });
        assert!(message.contains("no `start`"), "{message}");
    }

    #[test]
    fn attributes_have_the_form_of_a_rows_payload() {
        let text = "Acme opened an office.";
        let payload = |attributes: &str| {
            stage_one(text, |id| raw(id, 0, 4, attributes)).unwrap()[0]
                .payload
                .clone()
        };
        assert_eq!(
            payload(
                r#"{"name": "Acme", "employees": 12.0, "listed": true, "ratio": 0.5, "note": null}"#
            ),
            r#"{"employees":12,"listed":true,"name":"Acme","ratio":0.5}"#
        );
        // The order of the names and the spelling of a whole number do not
        // change the record.
        assert_eq!(
            payload(r#"{"employees": 12, "name": "Acme"}"#),
            payload(r#"{"name": "Acme", "employees": 1.2e1}"#)
        );
        assert_eq!(payload("null"), "{}");

        let message = refused(text, |id| raw(id, 0, 4, r#"{"names": ["Acme"]}"#));
        assert!(message.contains("attribute `names` is nested"), "{message}");
        let message = refused(text, |id| raw(id, 0, 4, r#"["Acme"]"#));
        assert!(message.contains("must be a JSON object"), "{message}");
        let message = refused(text, |id| raw(id, 0, 4, "{"));
        assert!(message.contains("not valid JSON"), "{message}");
    }

    #[test]
    fn one_place_holds_one_mention_of_a_type() {
        let text = "Acme opened an office.";
        let (id, documents) = given(text);
        let error = stage_mentions(
            &[
                raw(id, 0, 4, r#"{"name":"Acme"}"#),
                raw(id, 0, 4, r#"{"name":"ACME"}"#),
            ],
            &documents,
            &|_| true,
            &extractor(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("row 2: document `a.txt` already has a mention"),
            "{error}"
        );
    }

    #[test]
    fn every_document_of_a_call_is_staged_and_an_empty_one_is_refused() {
        let document = |key: &str, text: &str| NewDocument {
            key: key.to_owned(),
            uri: None,
            text: text.to_owned(),
        };
        let staged = stage(
            "news",
            &[
                document("a", "one"),
                document("a", "one"),
                document("a", "two"),
            ],
        )
        .unwrap()
        .unwrap();
        // Which of them is a version of its key is decided when they are
        // stored, against what the store holds.
        assert_eq!(staged.num_rows(), 3);
        assert!(stage("news", &[]).unwrap().is_none());

        for (source, documents, expected) in [
            ("", vec![document("a", "one")], "the source needs a name"),
            (
                "news",
                vec![document("", "one")],
                "document 1: it has no key",
            ),
            (
                "news",
                vec![document("a", "one"), document("b", "")],
                "document 2: it has no text",
            ),
        ] {
            let error = stage(source, &documents).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
