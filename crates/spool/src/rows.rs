//! A minimal [`RecordBatchReader`] over batches held in memory, with the two
//! ways of making one that tests, examples and the benchmark need.

use std::path::Path;
use std::sync::Arc;

use duckdb::Connection;
use duckdb::arrow::array::{ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray};
use duckdb::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use duckdb::arrow::error::ArrowError;
use duckdb::arrow::record_batch::{RecordBatch, RecordBatchReader};

use crate::error::{Error, Result};

pub struct Rows {
    schema: SchemaRef,
    batches: std::vec::IntoIter<RecordBatch>,
}

impl Rows {
    pub fn new(schema: SchemaRef, batches: Vec<RecordBatch>) -> Self {
        Self {
            schema,
            batches: batches.into_iter(),
        }
    }

    /// Rows of text. `None` is a missing value.
    pub fn from_text(columns: &[&str], rows: &[Vec<Option<&str>>]) -> Result<Self> {
        let schema = Arc::new(Schema::new(
            columns
                .iter()
                .map(|name| Field::new(*name, DataType::Utf8, true))
                .collect::<Vec<_>>(),
        ));
        let arrays: Vec<ArrayRef> = (0..columns.len())
            .map(|column| {
                let values: Vec<Option<&str>> = rows
                    .iter()
                    .map(|row| row.get(column).copied().flatten())
                    .collect();
                Arc::new(StringArray::from(values)) as ArrayRef
            })
            .collect();
        let batch = RecordBatch::try_new(schema.clone(), arrays)?;
        Ok(Self::new(schema, vec![batch]))
    }

    /// Rows given as JSON objects, one per row. A column is every key that
    /// occurs, and a row without the key, or with `null`, has no value there.
    ///
    /// A column holds text, booleans or numbers, and one kind only. Numbers
    /// are integers unless one of them has a fraction. A nested value is
    /// refused: send it as text, or flatten it.
    pub fn from_json(rows: &[serde_json::Map<String, serde_json::Value>]) -> Result<Self> {
        use serde_json::Value;

        #[derive(Clone, Copy, PartialEq)]
        enum Kind {
            Text,
            Boolean,
            Integer,
            Float,
        }
        let mut kinds: std::collections::BTreeMap<&str, Kind> = std::collections::BTreeMap::new();
        for (index, row) in rows.iter().enumerate() {
            for (name, value) in row {
                let kind = match value {
                    Value::Null => continue,
                    Value::String(_) => Kind::Text,
                    Value::Bool(_) => Kind::Boolean,
                    Value::Number(number) if number.is_i64() => Kind::Integer,
                    Value::Number(_) => Kind::Float,
                    Value::Array(_) | Value::Object(_) => {
                        return Err(Error::Rows(format!(
                            "row {}: column `{name}` holds a nested value; send it as text",
                            index + 1
                        )));
                    }
                };
                let known = kinds.entry(name.as_str()).or_insert(kind);
                *known = match (*known, kind) {
                    (a, b) if a == b => a,
                    (Kind::Integer, Kind::Float) | (Kind::Float, Kind::Integer) => Kind::Float,
                    _ => {
                        return Err(Error::Rows(format!(
                            "row {}: column `{name}` holds values of more than one type",
                            index + 1
                        )));
                    }
                };
            }
        }

        let mut fields = Vec::with_capacity(kinds.len());
        let mut arrays: Vec<ArrayRef> = Vec::with_capacity(kinds.len());
        for (name, kind) in &kinds {
            let values = rows.iter().map(|row| row.get(*name));
            let (data_type, array): (DataType, ArrayRef) = match kind {
                Kind::Text => (
                    DataType::Utf8,
                    Arc::new(StringArray::from_iter(
                        values.map(|value| value.and_then(Value::as_str)),
                    )),
                ),
                Kind::Boolean => (
                    DataType::Boolean,
                    Arc::new(BooleanArray::from_iter(
                        values.map(|value| value.and_then(Value::as_bool)),
                    )),
                ),
                Kind::Integer => (
                    DataType::Int64,
                    Arc::new(Int64Array::from_iter(
                        values.map(|value| value.and_then(Value::as_i64)),
                    )),
                ),
                Kind::Float => (
                    DataType::Float64,
                    Arc::new(Float64Array::from_iter(
                        values.map(|value| value.and_then(Value::as_f64)),
                    )),
                ),
            };
            fields.push(Field::new(*name, data_type, true));
            arrays.push(array);
        }
        let schema = Arc::new(Schema::new(fields));
        if arrays.is_empty() {
            // Arrow needs to be told how many rows a batch without columns has.
            return Ok(Self::new(schema, Vec::new()));
        }
        let batch = RecordBatch::try_new(schema.clone(), arrays)?;
        Ok(Self::new(schema, vec![batch]))
    }

    /// Reads a CSV file with a header row. Every column is read as text, and
    /// an empty field is a missing value.
    pub fn from_csv(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_file() {
            return Err(Error::Io {
                path: path.to_owned(),
                source: std::io::ErrorKind::NotFound.into(),
            });
        }
        let conn = Connection::open_in_memory()?;
        let mut statement =
            conn.prepare("SELECT * FROM read_csv(?, header = true, all_varchar = true)")?;
        let arrow = statement.query_arrow([path.to_string_lossy().as_ref()])?;
        let schema = arrow.get_schema();
        Ok(Self::new(schema, arrow.collect()))
    }

    /// Removes a column and returns its values with those of `key_column`,
    /// row by row. Rows where either is missing are left out of the pairs.
    pub fn take_column(
        self,
        column: &str,
        key_column: &str,
    ) -> Result<(Self, Vec<(String, String)>)> {
        let missing = |name: &str| Error::Scenario(format!("the rows have no column `{name}`"));
        let at = self.schema.index_of(column).map_err(|_| missing(column))?;
        let key_at = self
            .schema
            .index_of(key_column)
            .map_err(|_| missing(key_column))?;
        let keep: Vec<usize> = (0..self.schema.fields().len())
            .filter(|index| *index != at)
            .collect();

        let mut pairs = Vec::new();
        let mut batches = Vec::new();
        for batch in self.batches {
            let text = |index: usize| -> Result<StringArray> {
                let array = duckdb::arrow::compute::cast(batch.column(index), &DataType::Utf8)?;
                Ok(duckdb::arrow::array::AsArray::as_string::<i32>(&array).clone())
            };
            let (keys, values) = (text(key_at)?, text(at)?);
            for (key, value) in keys.iter().zip(values.iter()) {
                if let (Some(key), Some(value)) = (key, value) {
                    pairs.push((key.to_owned(), value.to_owned()));
                }
            }
            batches.push(batch.project(&keep)?);
        }
        let schema = Arc::new(self.schema.project(&keep)?);
        Ok((Self::new(schema, batches), pairs))
    }
}

impl Iterator for Rows {
    type Item = Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.batches.next().map(Ok)
    }
}

impl RecordBatchReader for Rows {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}
