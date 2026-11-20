//! Rows as a scenario reads them: the truth taken out of a column, and the
//! rows a source took back left out.

use std::collections::HashSet;
use std::sync::Arc;

use spool::Rows;
use spool::arrow::array::{AsArray, BooleanArray, StringArray};
use spool::arrow::compute::{cast, filter_record_batch};
use spool::arrow::datatypes::DataType;
use spool::arrow::record_batch::RecordBatchReader;

use crate::error::{Error, Result};

/// Removes a column and returns its values with those of `key_column`,
/// row by row. Rows where either is missing are left out of the pairs.
/// A column that is the key column itself is read and left in place.
pub fn take_column(
    rows: Rows,
    column: &str,
    key_column: &str,
) -> Result<(Rows, Vec<(String, String)>)> {
    let schema = rows.schema();
    let missing = |name: &str| Error::Scenario(format!("the rows have no column `{name}`"));
    let at = schema.index_of(column).map_err(|_| missing(column))?;
    let key_at = schema
        .index_of(key_column)
        .map_err(|_| missing(key_column))?;
    let keep: Vec<usize> = (0..schema.fields().len())
        .filter(|index| *index != at || at == key_at)
        .collect();

    let mut pairs = Vec::new();
    let mut batches = Vec::new();
    for batch in rows.into_batches() {
        let text = |index: usize| -> Result<StringArray> {
            let array = cast(batch.column(index), &DataType::Utf8)?;
            Ok(array.as_string::<i32>().clone())
        };
        let (keys, values) = (text(key_at)?, text(at)?);
        for (key, value) in keys.iter().zip(values.iter()) {
            if let (Some(key), Some(value)) = (key, value) {
                pairs.push((key.to_owned(), value.to_owned()));
            }
        }
        batches.push(batch.project(&keep)?);
    }
    let schema = Arc::new(schema.project(&keep)?);
    Ok((Rows::new(schema, batches), pairs))
}

/// Leaves out the rows whose value of `key_column` is one of `keys`.
pub(crate) fn without_keys(rows: Rows, key_column: &str, keys: &HashSet<String>) -> Result<Rows> {
    let schema = rows.schema();
    let at = schema
        .index_of(key_column)
        .map_err(|_| Error::Scenario(format!("the rows have no column `{key_column}`")))?;
    let mut batches = Vec::new();
    for batch in rows.into_batches() {
        let array = cast(batch.column(at), &DataType::Utf8)?;
        let kept: BooleanArray = array
            .as_string::<i32>()
            .iter()
            .map(|key| Some(key.is_none_or(|key| !keys.contains(key))))
            .collect();
        batches.push(filter_record_batch(&batch, &kept)?);
    }
    Ok(Rows::new(schema, batches))
}
