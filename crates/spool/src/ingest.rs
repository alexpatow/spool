//! Turns incoming rows into observations: a canonical payload, a key and a
//! content-derived ID per row. Only this module computes observation IDs.

use std::collections::{BTreeMap, HashSet};

use duckdb::arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, Float64Array, Int64Array, StringArray, UInt64Array,
};
use duckdb::arrow::compute::cast;
use duckdb::arrow::datatypes::{DataType, Float64Type, Int64Type, UInt64Type};
use duckdb::arrow::record_batch::RecordBatch;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use spool_core::{ObservationId, RunId};

use crate::batch::{Column, batch};
use crate::error::{Error, Result};

/// Which columns of the incoming rows are not plain attributes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IngestOptions {
    /// The column holding each row's key within the source: a row key, or
    /// the locator of an extracted mention. It stays part of the payload.
    pub key_column: Option<String>,
    /// A column of JSON text saying where in the source each observation was
    /// found. It is stored as `evidence` and is not part of the payload. A
    /// row that has evidence must also have a key.
    pub evidence_column: Option<String>,
}

impl IngestOptions {
    pub fn keyed_by(column: impl Into<String>) -> Self {
        Self {
            key_column: Some(column.into()),
            evidence_column: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IngestReport {
    pub run: RunId,
    /// Rows received.
    pub received: u64,
    /// Observations created.
    pub inserted: u64,
    /// Rows that were already known, or repeated within this call.
    pub duplicates: u64,
}

/// The rows of one incoming batch that were not seen earlier in this call,
/// as a batch of `observation_id`, `source_key`, `payload` and `evidence`.
pub(crate) fn stage(
    source: &str,
    incoming: &RecordBatch,
    options: &IngestOptions,
    seen: &mut HashSet<ObservationId>,
    first_row: usize,
) -> Result<Option<RecordBatch>> {
    let schema = incoming.schema();
    let position = |column: &Option<String>| -> Result<Option<usize>> {
        column
            .as_deref()
            .map(|name| {
                schema
                    .index_of(name)
                    .map_err(|_| Error::Ingest(format!("the rows have no column `{name}`")))
            })
            .transpose()
    };
    let key_at = position(&options.key_column)?;
    let evidence_at = position(&options.evidence_column)?;

    let mut readers = Vec::with_capacity(incoming.num_columns());
    for (index, field) in schema.fields().iter().enumerate() {
        readers.push((field.name().as_str(), Reader::new(incoming.column(index))?));
    }

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
                "the observation has evidence and no key; an extracted mention's key is its locator in the document"
                    .to_owned(),
            ));
        }

        // A map ordered by key, serialised without whitespace, is the canonical form.
        let payload = serde_json::to_string(&payload)
            .map_err(|error| fail(format!("the payload cannot be encoded: {error}")))?;
        let id = ObservationId::derive(source, key.as_deref(), &payload);
        if seen.insert(id) {
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
        ("observation_id", Column::Text(ids)),
        ("source_key", Column::Text(keys)),
        ("payload", Column::Text(payloads)),
        ("evidence", Column::Text(evidences)),
    ])?))
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
