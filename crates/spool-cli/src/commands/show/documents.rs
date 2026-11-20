//! `spool show documents`: the list of documents, and one document with its
//! text.

use anstream::println;
use spool::DocumentEntry;
use spool::core::DocumentId;
use spool_cli::text::count;

use super::{DocumentsArgs, Query, View, named, version, when};
use crate::Failure;
use crate::commands::remote::typed;
use crate::output::{Cell, Table, fields};

impl View<'_> {
    pub(super) fn documents(&self, what: &DocumentsArgs) -> Result<(), Failure> {
        let mut query = Query::default()
            .add("source", what.source.as_ref())
            .add("key", what.key.as_ref())
            .add("after", what.after.as_ref())
            .add("limit", what.limit);
        let all_versions = what.all_versions;
        Self::latest(&mut query, all_versions);
        let answer = self.remote.list("/documents", &query.0)?;
        let documents: Vec<DocumentEntry> = typed(&answer["documents"])?;
        let mut table = Table::new("documents").columns([
            "document",
            "source and key",
            "characters",
            "read by",
        ]);
        if all_versions {
            table = table.column("version");
        }
        for document in &documents {
            let mut row: Vec<Cell> = vec![
                document.id.to_string().into(),
                named(&document.source, Some(&document.key)).into(),
                document.length.to_string().into(),
                read_by(document).into(),
            ];
            if all_versions {
                row.push(version(document.superseded_by.is_some()).into());
            }
            table.row(row);
        }
        self.out.items(&answer, "documents", &table)?;
        self.more(&answer);
        Ok(())
    }

    pub(super) fn document(&self, document: DocumentId) -> Result<(), Failure> {
        let answer = self
            .remote
            .get(&format!("/documents/{document}"), &[], || {
                format!("there is no document {document}")
            })?;
        if !self.out.is_table() {
            self.out.json(&answer);
            return Ok(());
        }
        let document: DocumentEntry = typed(&answer)?;
        let mut rows = vec![
            ("document", document.id.to_string()),
            ("source", document.source.clone()),
            ("key", document.key.clone()),
        ];
        if let Some(uri) = &document.uri {
            rows.push(("uri", uri.clone()));
        }
        rows.push(("characters", document.length.to_string()));
        rows.push((
            "loaded",
            format!("{} by run {}", when(&document.loaded_at), document.load_run),
        ));
        if let Some(earlier) = document.supersedes {
            rows.push(("replaces", earlier.to_string()));
        }
        if let Some(later) = document.superseded_by {
            rows.push(("replaced by", later.to_string()));
        }
        rows.push(("read by", read_by(&document)));
        fields(&rows);
        println!("\n{}", document.text.unwrap_or_default().trim_end());
        Ok(())
    }
}

/// Which extractors read a document, which could not and why, and which is
/// reading it now.
fn read_by(document: &DocumentEntry) -> String {
    let mut said: Vec<String> = document
        .extractions
        .iter()
        .map(|extraction| match &extraction.failure {
            Some(failure) => format!(
                "{} failed in run {}: {failure}",
                extraction.extractor, extraction.run
            ),
            None => format!(
                "{} in run {} ({})",
                extraction.extractor,
                extraction.run,
                count(extraction.mentions, "mention")
            ),
        })
        .collect();
    if let Some(reading) = &document.reading {
        said.push(format!(
            "being read by {} in run {}",
            reading.extractor, reading.run
        ));
    }
    match said.is_empty() {
        true => "not read yet".to_owned(),
        false => said.join(", "),
    }
}
