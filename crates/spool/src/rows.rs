//! A minimal [`RecordBatchReader`] over batches held in memory, with the two
//! ways of making one that tests, examples and the benchmark need.

use std::path::Path;
use std::sync::Arc;

use duckdb::Connection;
use duckdb::arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, NullArray, StringArray, UInt64Array,
};
use duckdb::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use duckdb::arrow::error::ArrowError;
use duckdb::arrow::record_batch::{RecordBatch, RecordBatchOptions, RecordBatchReader};

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

    /// The batches that have not been read yet.
    pub fn into_batches(self) -> Vec<RecordBatch> {
        self.batches.collect()
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
    /// refused: send it as text, or flatten it. So is an integer that its
    /// column cannot hold exactly.
    #[expect(clippy::too_many_lines)]
    pub fn from_json(rows: &[serde_json::Map<String, serde_json::Value>]) -> Result<Self> {
        use serde_json::Value;

        /// Whole numbers from here on are not all floats, and the ones that
        /// are, are not written as integers when they are stored.
        const FLOATS_SKIP_INTEGERS_FROM: u64 = 1 << 53;

        #[derive(Clone, Copy, PartialEq)]
        enum Kind {
            Text,
            Boolean,
            Number,
        }
        /// What is known of a column, with the first row that showed it.
        struct Seen {
            kind: Option<Kind>,
            fraction: Option<usize>,
            negative: Option<usize>,
            past_signed: Option<usize>,
            past_floats: Option<usize>,
        }

        let mut columns: std::collections::BTreeMap<&str, Seen> = std::collections::BTreeMap::new();
        for (index, row) in rows.iter().enumerate() {
            for (name, value) in row {
                let seen = columns.entry(name.as_str()).or_insert(Seen {
                    kind: None,
                    fraction: None,
                    negative: None,
                    past_signed: None,
                    past_floats: None,
                });
                let kind = match value {
                    Value::Null => continue,
                    Value::String(_) => Kind::Text,
                    Value::Bool(_) => Kind::Boolean,
                    Value::Number(number) => {
                        if let Some(integer) = number.as_i64() {
                            if integer < 0 {
                                seen.negative.get_or_insert(index);
                            }
                            if integer.unsigned_abs() >= FLOATS_SKIP_INTEGERS_FROM {
                                seen.past_floats.get_or_insert(index);
                            }
                        } else if number.is_u64() {
                            seen.past_signed.get_or_insert(index);
                            seen.past_floats.get_or_insert(index);
                        } else {
                            seen.fraction.get_or_insert(index);
                        }
                        Kind::Number
                    }
                    Value::Array(_) | Value::Object(_) => {
                        return Err(Error::Rows(format!(
                            "row {}: column `{name}` holds a nested value; send it as text",
                            index + 1
                        )));
                    }
                };
                if *seen.kind.get_or_insert(kind) != kind {
                    return Err(Error::Rows(format!(
                        "row {}: column `{name}` holds values of more than one type",
                        index + 1
                    )));
                }
            }
        }

        let mut fields = Vec::with_capacity(columns.len());
        let mut arrays: Vec<ArrayRef> = Vec::with_capacity(columns.len());
        for (name, seen) in &columns {
            let values = rows.iter().map(|row| row.get(*name));
            let (data_type, array): (DataType, ArrayRef) = match seen.kind {
                None => (DataType::Null, Arc::new(NullArray::new(rows.len()))),
                Some(Kind::Text) => (
                    DataType::Utf8,
                    Arc::new(StringArray::from_iter(
                        values.map(|value| value.and_then(Value::as_str)),
                    )),
                ),
                Some(Kind::Boolean) => (
                    DataType::Boolean,
                    Arc::new(BooleanArray::from_iter(
                        values.map(|value| value.and_then(Value::as_bool)),
                    )),
                ),
                Some(Kind::Number) => match (seen.fraction, seen.past_signed) {
                    (Some(_), _) => {
                        if let Some(index) = seen.past_floats {
                            return Err(Error::Rows(format!(
                                "row {}: column `{name}` holds an integer too large to keep exactly beside numbers with a fraction; send the column as text",
                                index + 1
                            )));
                        }
                        (
                            DataType::Float64,
                            Arc::new(Float64Array::from_iter(
                                values.map(|value| value.and_then(Value::as_f64)),
                            )),
                        )
                    }
                    (None, Some(index)) => {
                        if let Some(negative) = seen.negative {
                            return Err(Error::Rows(format!(
                                "row {}: column `{name}` holds integers too far apart to keep exactly; send the column as text",
                                index.max(negative) + 1
                            )));
                        }
                        (
                            DataType::UInt64,
                            Arc::new(UInt64Array::from_iter(
                                values.map(|value| value.and_then(Value::as_u64)),
                            )),
                        )
                    }
                    (None, None) => (
                        DataType::Int64,
                        Arc::new(Int64Array::from_iter(
                            values.map(|value| value.and_then(Value::as_i64)),
                        )),
                    ),
                },
            };
            fields.push(Field::new(*name, data_type, true));
            arrays.push(array);
        }
        let schema = Arc::new(Schema::new(fields));
        if rows.is_empty() {
            return Ok(Self::new(schema, Vec::new()));
        }
        // Rows that name no column are rows all the same, and Arrow has to be
        // told how many there are.
        let batch = RecordBatch::try_new_with_options(
            schema.clone(),
            arrays,
            &RecordBatchOptions::new().with_row_count(Some(rows.len())),
        )?;
        Ok(Self::new(schema, vec![batch]))
    }

    /// Reads a CSV file with a header row. Every column is read as text, and
    /// an empty field is a missing value.
    pub fn from_csv(path: impl AsRef<Path>) -> Result<Self> {
        Self::read_csv(path.as_ref(), None)
    }

    /// Reads the rows of a CSV file for which `condition` holds, in the
    /// order of the file. The condition is a SQL expression over the file's
    /// columns, each of which is text: `region = 'north'`, or
    /// `id::BIGINT % 10 = 0`.
    pub fn from_csv_where(path: impl AsRef<Path>, condition: &str) -> Result<Self> {
        Self::read_csv(path.as_ref(), Some(condition))
    }

    fn read_csv(path: &Path, condition: Option<&str>) -> Result<Self> {
        if !path.is_file() {
            return Err(Error::Io {
                path: path.to_owned(),
                source: std::io::ErrorKind::NotFound.into(),
            });
        }
        let conn = Connection::open_in_memory()?;
        let filter = match condition {
            // The condition ends at its own end: a line comment in it would
            // otherwise take the closing bracket with it.
            Some(condition) => format!(" WHERE (\n{condition}\n)"),
            None => String::new(),
        };
        let mut statement = conn.prepare(&format!(
            "SELECT * FROM read_csv(?, header = true, all_varchar = true){filter}"
        ))?;
        let arrow = statement.query_arrow([path.to_string_lossy().as_ref()])?;
        let schema = arrow.get_schema();
        Ok(Self::new(schema, arrow.collect()))
    }

    /// Removes a column and returns its values with those of `key_column`,
    /// row by row. Rows where either is missing are left out of the pairs.
    /// A column that is the key column itself is read and left in place.
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
            .filter(|index| *index != at || at == key_at)
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

    /// Leaves out the rows whose value of `key_column` is one of `keys`.
    pub(crate) fn without(
        self,
        key_column: &str,
        keys: &std::collections::HashSet<String>,
    ) -> Result<Self> {
        let at = self
            .schema
            .index_of(key_column)
            .map_err(|_| Error::Scenario(format!("the rows have no column `{key_column}`")))?;
        let mut batches = Vec::new();
        for batch in self.batches {
            let array = duckdb::arrow::compute::cast(batch.column(at), &DataType::Utf8)?;
            let kept: duckdb::arrow::array::BooleanArray =
                duckdb::arrow::array::AsArray::as_string::<i32>(&array)
                    .iter()
                    .map(|key| Some(key.is_none_or(|key| !keys.contains(key))))
                    .collect();
            batches.push(duckdb::arrow::compute::filter_record_batch(&batch, &kept)?);
        }
        Ok(Self::new(self.schema, batches))
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
