//! Staging the documents of one call, and storing them as versions of
//! their keys.

use duckdb::arrow::record_batch::RecordBatch;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use duckdb::{Connection, params};
use spool_core::{DocumentId, RunId};

use super::NewDocument;
use crate::batch::{Column, batch};
use crate::error::{Error, Result};
use crate::versions;

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

#[cfg(test)]
mod tests {
    use super::*;

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
