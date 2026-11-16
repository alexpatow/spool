//! Small helpers for handing rows to DuckDB as Arrow batches.
//!
//! IDs cross as strings. A batch is passed to a statement as the parameter of
//! the `arrow(?)` table function, which lets one statement insert or join
//! against any number of rows.

use std::sync::Arc;

use duckdb::arrow::array::{ArrayRef, Int64Array, StringArray};
use duckdb::arrow::datatypes::{DataType, Field, Schema};
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::vtab::arrow::ArrowBatchRegistration;

use crate::error::Result;

pub(crate) enum Column {
    Text(Vec<Option<String>>),
    Integer(Vec<i64>),
}

/// Builds a batch from named columns of equal length.
pub(crate) fn batch(columns: Vec<(&str, Column)>) -> Result<RecordBatch> {
    let mut fields = Vec::with_capacity(columns.len());
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(columns.len());
    for (name, column) in columns {
        match column {
            Column::Text(values) => {
                fields.push(Field::new(name, DataType::Utf8, true));
                arrays.push(Arc::new(StringArray::from(values)));
            }
            Column::Integer(values) => {
                fields.push(Field::new(name, DataType::Int64, false));
                arrays.push(Arc::new(Int64Array::from(values)));
            }
        }
    }
    Ok(RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)?)
}

/// A batch with columns `ord` and `id`, for looking up a list of IDs and
/// getting the answers back in the order asked.
pub(crate) fn id_list<T: ToString>(ids: &[T]) -> Result<ArrowBatchRegistration> {
    let batch = batch(vec![
        ("ord", Column::Integer((0..ids.len() as i64).collect())),
        (
            "id",
            Column::Text(ids.iter().map(|id| Some(id.to_string())).collect()),
        ),
    ])?;
    Ok(ArrowBatchRegistration::new(batch))
}
