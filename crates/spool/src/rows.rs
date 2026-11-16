//! A minimal [`RecordBatchReader`] over batches held in memory, with the two
//! ways of making one that tests, examples and the benchmark need.

use std::path::Path;
use std::sync::Arc;

use duckdb::Connection;
use duckdb::arrow::array::{ArrayRef, StringArray};
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
