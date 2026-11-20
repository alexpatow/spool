//! The truth of a scenario: what each row is, and what each document
//! mentions where.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use spool::arrow::array::{Array, AsArray};
use spool::core::DocumentId;
use spool::{Rows, locator};

use crate::error::{Error, Result};

/// What a scenario knows to be true about what it has fed the engine.
#[derive(Default)]
pub(crate) struct Truth {
    /// `(source, key)` to true entity, for the records there may be so
    /// far.
    pub(crate) entities: HashMap<(String, String), String>,
    /// The labelled mentions of each document, by `(source, document key)`.
    pub(crate) labels: HashMap<(String, String), Vec<Label>>,
    /// Whether the scenario has labelled mentions at all.
    pub(crate) labelled: bool,
    /// `(source, locator)` of the labelled mentions in the documents stored
    /// so far.
    pub(crate) gold: HashSet<(String, String)>,
}

/// One labelled mention.
pub(crate) struct Label {
    pub(crate) entity_type: String,
    pub(crate) start: u64,
    pub(crate) end: u64,
    /// The document's text at that place, if the label says.
    pub(crate) text: Option<String>,
    pub(crate) entity: String,
}

impl Truth {
    /// Takes note of a document that was stored as the version `id`: its
    /// labelled mentions are now mentions an extractor should find, each
    /// at a known key.
    pub(crate) fn add_document(
        &mut self,
        source: &str,
        id: DocumentId,
        key: &str,
        text: &str,
    ) -> Result<()> {
        let Some(labels) = self.labels.get(&(source.to_owned(), key.to_owned())) else {
            return Ok(());
        };
        let characters: Vec<char> = text.chars().collect();
        for label in labels {
            let invalid = |message: String| {
                Error::Scenario(format!(
                    "the labelled mention of `{}` at {}-{} in document `{key}` of `{source}`: {message}",
                    label.entity, label.start, label.end
                ))
            };
            let (start, end) = (label.start as usize, label.end as usize);
            if start >= end || end > characters.len() {
                return Err(invalid(format!(
                    "the document is {} long",
                    characters.len()
                )));
            }
            let there: String = characters[start..end].iter().collect();
            if let Some(expected) = &label.text
                && *expected != there
            {
                return Err(invalid(format!(
                    "the label says the text there is `{expected}`, and it is `{there}`"
                )));
            }
            let key = locator(id, &label.entity_type, label.start, label.end);
            self.gold.insert((source.to_owned(), key.clone()));
            self.entities
                .insert((source.to_owned(), key), label.entity.clone());
        }
        Ok(())
    }
}

/// Reads labelled mentions: each with the `(source, document key)` it is in.
pub(crate) fn read_labels(path: &Path) -> Result<Vec<((String, String), Label)>> {
    const COLUMNS: [&str; 6] = [
        "source",
        "document",
        "start",
        "end",
        "entity_type",
        "true_entity",
    ];
    let invalid = |message: String| Error::Scenario(format!("{}: {message}", path.display()));
    let rows = Rows::from_csv(path)?;
    let schema = spool::arrow::record_batch::RecordBatchReader::schema(&rows);
    let mut at = Vec::new();
    for name in COLUMNS {
        at.push(schema.index_of(name).map_err(|_| {
            invalid(
                "a file of labelled mentions needs the columns `source`, `document`, `start`, \
                 `end`, `entity_type` and `true_entity`"
                    .to_owned(),
            )
        })?);
    }
    let text_at = schema.index_of("text").ok();
    let mut labels = Vec::new();
    let mut line = 1;
    for batch in rows {
        let batch = batch?;
        let column = |index: usize| batch.column(index).as_string::<i32>();
        for row in 0..batch.num_rows() {
            line += 1;
            let field = |index: usize| -> Result<String> {
                let values = column(at[index]);
                if values.is_null(row) || values.value(row).is_empty() {
                    return Err(invalid(format!("line {line} has no `{}`", COLUMNS[index])));
                }
                Ok(values.value(row).to_owned())
            };
            let position = |index: usize| -> Result<u64> {
                field(index)?.parse().map_err(|_| {
                    invalid(format!(
                        "line {line}: `{}` is not a position",
                        COLUMNS[index]
                    ))
                })
            };
            let text = text_at
                .map(column)
                .filter(|values| !values.is_null(row))
                .map(|values| values.value(row).to_owned());
            labels.push((
                (field(0)?, field(1)?),
                Label {
                    start: position(2)?,
                    end: position(3)?,
                    entity_type: field(4)?,
                    entity: field(5)?,
                    text,
                },
            ));
        }
    }
    Ok(labels)
}

pub(crate) fn read_truth_file(path: &Path) -> Result<Vec<(String, String, String)>> {
    let invalid = |message: &str| Error::Scenario(format!("{}: {message}", path.display()));
    let rows = Rows::from_csv(path)?;
    let schema = spool::arrow::record_batch::RecordBatchReader::schema(&rows);
    let mut at = Vec::new();
    for name in ["source", "source_key", "true_entity"] {
        at.push(schema.index_of(name).map_err(|_| {
            invalid("a truth file needs the columns `source`, `source_key` and `true_entity`")
        })?);
    }
    let mut truth = Vec::new();
    for batch in rows {
        let batch = batch?;
        let column = |index: usize| batch.column(at[index]).as_string::<i32>();
        let (sources, keys, entities) = (column(0), column(1), column(2));
        for row in 0..batch.num_rows() {
            if sources.is_null(row) || keys.is_null(row) || entities.is_null(row) {
                return Err(invalid("a row has an empty field"));
            }
            truth.push((
                sources.value(row).to_owned(),
                keys.value(row).to_owned(),
                entities.value(row).to_owned(),
            ));
        }
    }
    Ok(truth)
}
