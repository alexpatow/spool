//! Turns incoming rows into records: a canonical payload, a key and a
//! content-derived ID per row. Only this module computes record IDs.

use std::collections::{BTreeMap, HashSet};

use duckdb::arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, Float64Array, Int64Array, StringArray, UInt64Array,
};
use duckdb::arrow::compute::cast;
use duckdb::arrow::datatypes::{DataType, Float64Type, Int64Type, UInt64Type};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use spool_core::{Committed, RecordId, RunId};

use crate::batch::{Column, batch};
use crate::error::{Error, Result};
use crate::project::Source;
use crate::retract;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IngestReport {
    pub run: RunId,
    /// Rows received.
    pub received: u64,
    /// Records created.
    pub inserted: u64,
    /// Rows that were already known, or repeated within this call.
    pub duplicates: u64,
    /// Records created as a new version of a row the source had sent
    /// before under the same key with other content.
    #[serde(default)]
    pub superseded: u64,
    /// Records the source had taken back and holds again: every version
    /// of a row that was sent again, changed or not.
    #[serde(default)]
    pub restored: u64,
    /// Those of them that are back in the identity they left, or in the
    /// one that stands for it now. The rest had none, or can no longer be
    /// there, and are new to the next match run.
    #[serde(default)]
    pub returned: u64,
    /// The changeset that put them back, if any went back.
    #[serde(default)]
    pub committed: Option<Committed>,
    /// Identities a record went back to where nothing holds it with the
    /// records that are there: what joined them went through a record that
    /// is still taken back. Each has an open `possible_split` review item,
    /// which this run raised.
    #[serde(default)]
    pub possible_splits: u64,
}

/// What storing one staged batch did.
pub(crate) struct Stored {
    /// Records created.
    pub inserted: u64,
    /// Records that had been taken back and came back.
    pub restored: u64,
}

/// The rows of one incoming batch that were not seen earlier in this call,
/// as a batch of `ord`, `record_id`, `source_key`, `payload` and
/// `evidence`. `ord` is the row's position in the call.
///
/// The rows may have only the columns the source lists. The source's key
/// column stays part of the payload; its evidence column does not.
pub(crate) fn stage(
    source: &Source,
    incoming: &RecordBatch,
    seen: &mut HashSet<RecordId>,
    first_row: usize,
) -> Result<Option<RecordBatch>> {
    let schema = incoming.schema();
    for field in schema.fields() {
        if !source.columns.iter().any(|column| column == field.name()) {
            return Err(Error::Ingest(format!(
                "the rows have a column `{}`, which sources/{}.yaml does not list",
                field.name(),
                source.name
            )));
        }
    }
    // A column the rows do not have is empty in each of them.
    let position =
        |column: &Option<String>| -> Option<usize> { schema.index_of(column.as_deref()?).ok() };
    let key_at = position(&source.key);
    let evidence_at = position(&source.evidence);

    let mut readers = Vec::with_capacity(incoming.num_columns());
    for (index, field) in schema.fields().iter().enumerate() {
        readers.push((field.name().as_str(), Reader::new(incoming.column(index))?));
    }

    let mut ords = Vec::new();
    let mut ids = Vec::new();
    let mut keys = Vec::new();
    let mut payloads = Vec::new();
    let mut evidences = Vec::new();

    for row in 0..incoming.num_rows() {
        let row_number = first_row + row + 1;
        let fail = |message: String| Error::Ingest(format!("row {row_number}: {message}"));

        let mut payload = BTreeMap::new();
        let mut key = None;
        let mut evidence = None;
        for (index, (name, reader)) in readers.iter().enumerate() {
            let value = reader
                .value(row)
                .map_err(|message| fail(format!("column `{name}`: {message}")))?;
            if Some(index) == evidence_at {
                evidence = match value {
                    None => None,
                    Some(Value::String(text)) => {
                        let parsed: Value = serde_json::from_str(&text).map_err(|error| {
                            fail(format!("column `{name}` is not valid JSON: {error}"))
                        })?;
                        (!parsed.is_null()).then_some(parsed.to_string())
                    }
                    Some(_) => return Err(fail(format!("column `{name}` must hold JSON text"))),
                };
                continue;
            }
            if Some(index) == key_at {
                key = match &value {
                    Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
                    Some(Value::String(_)) | None => None,
                    Some(other) => Some(other.to_string()),
                };
            }
            // An attribute without a value is left out, so a source that adds
            // an empty column does not change the ID of every row.
            if let Some(value) = value {
                payload.insert(*name, value);
            }
        }
        if evidence.is_some() && key.is_none() {
            return Err(fail(
                "the record has evidence and no key; an extracted mention's key is its locator in the document"
                    .to_owned(),
            ));
        }

        // A map ordered by key, serialised without whitespace, is the canonical form.
        let payload = serde_json::to_string(&payload)
            .map_err(|error| fail(format!("the payload cannot be encoded: {error}")))?;
        let id = RecordId::derive(&source.name, key.as_deref(), &payload);
        if seen.insert(id) {
            ords.push(row_number as i64);
            ids.push(Some(id.to_string()));
            keys.push(key);
            payloads.push(Some(payload));
            evidences.push(evidence);
        }
    }

    if ids.is_empty() {
        return Ok(None);
    }
    Ok(Some(batch(vec![
        ("ord", Column::Integer(ords)),
        ("record_id", Column::Text(ids)),
        ("source_key", Column::Text(keys)),
        ("payload", Column::Text(payloads)),
        ("evidence", Column::Text(evidences)),
    ])?))
}

/// Stores staged records of one entity type from one source.
///
/// A new record under a key the source has used before is the next
/// version of that row or mention. It supersedes the latest version the
/// store holds, or the one before it in the batch.
///
/// A row the source had taken back is held again, with every version of
/// it, whether or not it came with new content.
pub(crate) fn insert(
    conn: &Connection,
    entity_type: &str,
    source: &str,
    now: i64,
    run: RunId,
    staged: RecordBatch,
) -> Result<Stored> {
    let restored = retract::restore(conn, entity_type, source, run, &staged)?;
    let inserted = conn.execute(
        "INSERT INTO spool.record
         SELECT n.record_id, $1, $2, n.source_key,
                CASE WHEN n.source_key IS NOT NULL THEN coalesce(
                  lag(n.record_id) OVER (PARTITION BY n.source_key ORDER BY n.ord),
                  latest.record_id) END,
                n.payload::JSON, n.evidence::JSON, NULL, make_timestamptz($3), $4
         FROM (
           SELECT s.record_id::UUID AS record_id, s.source_key,
                  s.payload, s.evidence, s.ord
           FROM arrow($5) s
           WHERE NOT EXISTS (
             SELECT 1 FROM spool.record o
             WHERE o.record_id = s.record_id::UUID)
         ) n
         LEFT JOIN (
           SELECT o.source_key, o.record_id
           FROM spool.record o
           WHERE o.entity_type = $1 AND o.source_id = $2
             AND o.source_key IS NOT NULL
             AND NOT EXISTS (
               SELECT 1 FROM spool.record later
               WHERE later.supersedes = o.record_id)
         ) latest ON latest.source_key = n.source_key",
        params![
            entity_type,
            source,
            now,
            run.0 as i64,
            &ArrowBatchRegistration::new(staged)
        ],
    )?;
    Ok(Stored {
        inserted: inserted as u64,
        restored,
    })
}

/// Reads one Arrow column as JSON values. Text, booleans and numbers keep
/// their type; anything else is stored as the text DuckDB would print.
enum Reader {
    Null,
    Text(StringArray),
    Boolean(BooleanArray),
    Integer(Int64Array),
    Unsigned(UInt64Array),
    Float(Float64Array),
}

impl Reader {
    fn new(array: &ArrayRef) -> Result<Self> {
        use DataType::*;
        Ok(match array.data_type() {
            Null => Reader::Null,
            Boolean => Reader::Boolean(array.as_boolean().clone()),
            Int8 | Int16 | Int32 | Int64 | UInt8 | UInt16 | UInt32 => {
                Reader::Integer(cast(array, &Int64)?.as_primitive::<Int64Type>().clone())
            }
            UInt64 => Reader::Unsigned(array.as_primitive::<UInt64Type>().clone()),
            Float16 | Float32 | Float64 => {
                Reader::Float(cast(array, &Float64)?.as_primitive::<Float64Type>().clone())
            }
            _ => Reader::Text(cast(array, &Utf8)?.as_string::<i32>().clone()),
        })
    }

    fn value(&self, row: usize) -> Result<Option<Value>, String> {
        Ok(match self {
            Reader::Null => None,
            Reader::Text(array) => array.is_valid(row).then(|| Value::from(array.value(row))),
            Reader::Boolean(array) => array.is_valid(row).then(|| Value::from(array.value(row))),
            Reader::Integer(array) => array.is_valid(row).then(|| Value::from(array.value(row))),
            Reader::Unsigned(array) => array.is_valid(row).then(|| Value::from(array.value(row))),
            Reader::Float(array) => {
                if array.is_null(row) {
                    None
                } else {
                    let value = array.value(row);
                    // A whole number is written as an integer whatever the
                    // column's type, so that a row's ID does not depend on
                    // the rows it arrived with: a column of 1 and 2.5 is a
                    // float column, and a column of 1 alone is not.
                    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
                        Some(Value::from(value as i64))
                    } else {
                        let number = serde_json::Number::from_f64(value)
                            .ok_or("the number is not finite, so it has no JSON form")?;
                        Some(Value::Number(number))
                    }
                }
            }
        })
    }
}
