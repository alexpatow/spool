//! `spool show records`: the list of records, and one record with its
//! payload, its evidence and, for a mention, its place in its document. Also
//! how a record or a mention is found by the name a list gives it, which
//! `spool explain` uses as well.

use std::collections::BTreeMap;

use anstream::println;
use serde::Serialize;
use serde_json::{Value, json};
use spool::core::{DocumentId, RecordId};
use spool::{DocumentEntry, RecordEntry, ReviewEntry};
use spool_cli::api::{DocumentList, Info, RecordList};

use super::{
    Query, RecordsArgs, View, format_enum_word, format_json_value, format_source_key,
    format_timestamp, format_version_status, has_mixed_types, label_cell, record_name,
};
use crate::Failure;
use crate::commands::remote::{Remote, typed};
use crate::output::{Cell, Table, aligned, fields, hint};

/// How much of a document is shown on either side of a mention, in
/// characters.
const CONTEXT: usize = 80;

/// The record a reference names: an ID, or `<source>:<key>`, which
/// names the version of the row that nothing has replaced, or a mention as
/// the lists name it, `<source>:<document>[<start>-<end>]`.
pub(in crate::commands) fn find_record(
    remote: &Remote,
    reference: &str,
) -> Result<RecordEntry, Failure> {
    if reference.parse::<RecordId>().is_ok() {
        let found = remote.get(&format!("/records/{reference}"), &[], || {
            format!("there is no record {reference}")
        })?;
        return typed(&found);
    }
    let Some((source, key)) = reference.split_once(':') else {
        return Err(Failure::usage(format!(
            "`{reference}` does not name a record: give its ID, or <source>:<key>"
        )));
    };
    // Versions are listed oldest first, in pages. The latest is the one no
    // other replaced.
    let mut query = vec![("source", source.to_owned()), ("key", key.to_owned())];
    let mut last = None;
    loop {
        let found = remote.list("/records", &query)?;
        let page: RecordList = typed(&found)?;
        for version in page.records {
            if version.superseded_by.is_none() {
                return Ok(version);
            }
            last = Some(version);
        }
        match page.next {
            Some(next) => {
                query.truncate(2);
                query.push(("after", next.to_string()));
            }
            None => break,
        }
    }
    if let Some(last) = last {
        return Ok(last);
    }
    if let Some(mention) = find_mention(remote, reference, source, key)? {
        return Ok(mention);
    }
    Err(Failure::failed(format!(
        "there is no record with key `{key}` from source `{source}`"
    )))
}

/// The record found at a place in a document, named as
/// [`RecordEntry::name`] names it. Of the versions of the document, it is
/// the one in the latest that has such a record.
fn find_mention(
    remote: &Remote,
    reference: &str,
    source: &str,
    place: &str,
) -> Result<Option<RecordEntry>, Failure> {
    let Some((document, span)) = place
        .strip_suffix(']')
        .and_then(|place| place.rsplit_once('['))
    else {
        return Ok(None);
    };
    let is_span = span
        .split_once('-')
        .is_some_and(|(start, end)| start.parse::<u64>().is_ok() && end.parse::<u64>().is_ok());
    if !is_span {
        return Ok(None);
    }
    let mut versions: Vec<DocumentEntry> = Vec::new();
    let mut query = vec![("source", source.to_owned()), ("key", document.to_owned())];
    loop {
        let page: DocumentList = typed(&remote.list("/documents", &query)?)?;
        versions.extend(page.documents);
        let Some(next) = page.next else { break };
        query.truncate(2);
        query.push(("after", next.to_string()));
    }
    for version in newest_first(versions) {
        let mut found = Vec::new();
        let mut query = vec![("document", version.id.to_string())];
        loop {
            let page: RecordList = typed(&remote.list("/records", &query)?)?;
            found.extend(
                page.records
                    .into_iter()
                    .filter(|record| record.name() == reference),
            );
            let Some(next) = page.next else { break };
            query.truncate(1);
            query.push(("after", next.to_string()));
        }
        // A mention found again with other values is a version of the same
        // record, at the same place: the name is the one no other replaced.
        let ids: Vec<RecordId> = found.iter().map(|record| record.id).collect();
        found.retain(|record| {
            record
                .superseded_by
                .is_none_or(|later| !ids.contains(&later))
        });
        match found.len() {
            0 => {}
            1 => return Ok(found.pop()),
            several => {
                let ids: Vec<String> = found.iter().map(|record| record.id.to_string()).collect();
                return Err(Failure::failed(format!(
                    "{several} records are named `{reference}`: name one by its ID, of {}",
                    ids.join(", ")
                )));
            }
        }
    }
    Ok(None)
}

/// The versions of one document, the one nothing replaced first and then
/// the one each replaced. A list of documents is in the order of their
/// IDs, which are hashes of their text and say nothing of which is newer.
fn newest_first(versions: Vec<DocumentEntry>) -> Vec<DocumentEntry> {
    let mut left: BTreeMap<DocumentId, DocumentEntry> = versions
        .into_iter()
        .map(|version| (version.id, version))
        .collect();
    let mut ordered = Vec::with_capacity(left.len());
    let mut next = left
        .values()
        .find(|version| version.superseded_by.is_none())
        .map(|version| version.id);
    while let Some(version) = next.and_then(|id| left.remove(&id)) {
        next = version.supersedes;
        ordered.push(version);
    }
    // A chain that is whole leaves none.
    ordered.extend(left.into_values());
    ordered
}

impl View<'_> {
    /// The documents that the mentions among `records` were found in and
    /// that a later version replaced. Such a mention is not current, though
    /// nothing replaced the mention itself: one found in the new version is
    /// a record of its own.
    fn replaced_documents(&self, records: &[RecordEntry]) -> Result<Vec<DocumentId>, Failure> {
        let mut documents: Vec<DocumentId> = records.iter().filter_map(mention_document).collect();
        documents.sort_unstable();
        documents.dedup();
        let mut replaced = Vec::new();
        for document in documents {
            if let Some(found) = self.remote.find(&format!("/documents/{document}"), &[])?
                && !found["superseded_by"].is_null()
            {
                replaced.push(document);
            }
        }
        Ok(replaced)
    }

    /// The attributes a list of records shows a column for: the columns
    /// each record's source declares, in its order, other than its key,
    /// which the list shows already, and its evidence. Then any other a
    /// record has, such as a column the source no longer declares.
    fn attribute_columns(&self, records: &[RecordEntry]) -> Result<Vec<String>, Failure> {
        let info: Info = typed(&self.remote.list("/info", &[])?)?;
        let mut columns: Vec<String> = Vec::new();
        let mut add = |column: &String| {
            if !columns.contains(column) {
                columns.push(column.clone());
            }
        };
        for record in records {
            let source = info
                .store
                .sources
                .iter()
                .find(|source| source.name == record.source);
            let shown = |column: &&String| {
                source.is_none_or(|source| {
                    source.key.as_ref() != Some(*column)
                        && source.evidence.as_ref() != Some(*column)
                })
            };
            if let Some(source) = source {
                source.columns.iter().filter(shown).for_each(&mut add);
            }
            if let Some(payload) = record.payload.as_object() {
                payload.keys().filter(shown).for_each(&mut add);
            }
        }
        Ok(columns)
    }

    pub(super) fn records(&self, what: &RecordsArgs) -> Result<(), Failure> {
        let mut query = Query::default()
            .add("entity_type", what.entity_type.as_ref())
            .add("source", what.source.as_ref())
            .add("key", what.key.as_ref())
            .add("document", what.document.as_ref())
            .flag("unassigned", what.unassigned)
            .add("after", what.after.as_ref())
            .add("limit", what.limit);
        let narrowed = !query.0.is_empty();
        let all_versions = what.all_versions;
        Self::ask_for_latest(&mut query, all_versions);
        let answer = self.remote.list("/records", &query.0)?;
        // JSON is the answer as it is, which needs nothing more.
        if self.out.is_json() {
            self.out.items(&answer, "records", &Table::new("records"))?;
            return Ok(());
        }
        let records: Vec<RecordEntry> = typed(&answer["records"])?;
        if records.is_empty() && !narrowed && self.out.is_table() {
            println!("no records");
            hint("`spool load <file>` sends rows");
            return Ok(());
        }
        let attributes = match what.attributes {
            true => self.attribute_columns(&records)?,
            false => Vec::new(),
        };
        let typed_rows = has_mixed_types(records.iter().map(|record| &record.entity_type));
        let mut table = Table::new("records")
            .column_if("record", what.ids)
            .column_if("type", typed_rows)
            .column("source and key");
        match what.attributes {
            true => table = table.columns(attributes.iter().map(String::as_str)),
            false => table = table.column("name"),
        }
        table = table.column("identity");
        let replaced_documents = match all_versions {
            true => self.replaced_documents(&records)?,
            false => Vec::new(),
        };
        if all_versions {
            table = table.column("version");
        }
        for record in &records {
            let mut row: Vec<Cell> = vec![
                record.id.to_string().into(),
                record.entity_type.to_string().into(),
                record_name(record).into(),
            ];
            if what.attributes {
                row.extend(attributes.iter().map(
                    |attribute| match record.payload.get(attribute) {
                        Some(value) if !value.is_null() => {
                            label_cell(Some(&format_json_value(value)))
                        }
                        _ => "-".into(),
                    },
                ));
            } else {
                row.push(label_cell(record.label.as_deref()));
            }
            row.push(
                record
                    .identity
                    .map_or_else(|| "-".to_owned(), |identity| identity.to_string())
                    .into(),
            );
            if all_versions {
                let in_replaced = mention_document(record)
                    .is_some_and(|document| replaced_documents.contains(&document));
                row.push(
                    format_version_status(record.superseded_by.is_some() || in_replaced).into(),
                );
            }
            table.row(row);
        }
        self.out.items(&answer, "records", &table)?;
        self.more(&answer);
        // A `-` in the last column says nothing about why, or what to do.
        let waiting = records
            .iter()
            .filter(|record| record.identity.is_none() && record.retracted.is_none())
            .count() as u64;
        if waiting > 0 {
            hint(format!(
                "{} no identity: `spool match` places new records in identities, and `spool show \
                 review` lists the ones a match left out",
                match waiting {
                    1 => "1 record has".to_owned(),
                    several => format!("{several} records have"),
                }
            ));
        }
        Ok(())
    }

    pub(super) fn record(&self, reference: &str) -> Result<(), Failure> {
        let record = find_record(self.remote, reference)?;
        let review = self
            .remote
            .list("/review", &[("record", record.id.to_string())])?;
        let mention = self.mention(&record)?;
        if !self.out.is_table() {
            self.out.json(&json!({
                "record": record,
                "mention": mention,
                "review_items": review["items"],
            }));
            return Ok(());
        }
        let review: Vec<ReviewEntry> = typed(&review["items"])?;

        let mut rows = vec![
            ("record", record.id.to_string()),
            ("type", record.entity_type.to_string()),
            ("source", record.source.clone()),
        ];
        if let Some(key) = &record.key {
            rows.push(("key", key.clone()));
        }
        rows.push((
            "loaded",
            format!(
                "{} by run {}",
                format_timestamp(&record.loaded_at),
                record.load_run
            ),
        ));
        if !record.found_by.is_empty() {
            rows.push((
                "found by",
                record
                    .found_by
                    .iter()
                    .map(|found| {
                        format!(
                            "{} version {} in run {}",
                            found.extractor, found.version, found.run
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        let found = !record.found_by.is_empty();
        rows.push((
            "identity",
            match (record.identity, record.retracted) {
                (Some(identity), _) => identity.to_string(),
                (None, Some(run)) if found => {
                    format!("none: taken back in run {run}, as no extractor finds it any more")
                }
                (None, Some(run)) => format!("none: its source took it back in run {run}"),
                (None, None) => "none".to_owned(),
            },
        ));
        if let Some(earlier) = record.supersedes {
            rows.push(("replaces", earlier.to_string()));
        }
        if let Some(later) = record.superseded_by {
            rows.push(("replaced by", later.to_string()));
        }
        for item in &review {
            rows.push((
                "review",
                format!(
                    "item {} ({}, {})",
                    item.id,
                    format_enum_word(&item.kind),
                    format_enum_word(&item.status)
                ),
            ));
        }
        fields(&rows);

        println!("\npayload");
        print_json_object(&record.payload);
        if let Some(evidence) = &record.evidence {
            println!("\nevidence");
            print_json_object(evidence);
        }
        if let Some(mention) = mention {
            println!(
                "\nin document {}, characters {} to {}",
                format_source_key(mention.source.as_str(), Some(mention.key.as_str())),
                mention.start,
                mention.end
            );
            println!(
                "  {}{}[{}]{}{}",
                if mention.cut_before { "…" } else { "" },
                mention.before,
                mention.text,
                mention.after,
                if mention.cut_after { "…" } else { "" },
            );
        }
        Ok(())
    }

    /// Where a mention was found: the text around it in its document. `None`
    /// for a record that is not a mention.
    fn mention(&self, record: &RecordEntry) -> Result<Option<Mention>, Failure> {
        let Some(evidence) = &record.evidence else {
            return Ok(None);
        };
        let (Some(document), Some(start), Some(end)) = (
            evidence["document"]
                .as_str()
                .and_then(|id| id.parse::<DocumentId>().ok()),
            evidence["start"].as_u64(),
            evidence["end"].as_u64(),
        ) else {
            return Ok(None);
        };
        let Some(found) = self.remote.find(&format!("/documents/{document}"), &[])? else {
            return Ok(None);
        };
        let entry: DocumentEntry = typed(&found)?;
        let text: Vec<char> = entry.text.unwrap_or_default().chars().collect();
        let (start, end) = (start as usize, end as usize);
        if start > end || end > text.len() {
            return Ok(None);
        }
        let from = start.saturating_sub(CONTEXT);
        let to = (end + CONTEXT).min(text.len());
        let line = |characters: &[char]| -> String {
            characters
                .iter()
                .map(|character| match character {
                    '\n' | '\r' | '\t' => ' ',
                    other => *other,
                })
                .collect()
        };
        Ok(Some(Mention {
            document,
            source: entry.source,
            key: entry.key,
            start: start as u64,
            end: end as u64,
            before: line(&text[from..start]),
            text: line(&text[start..end]),
            after: line(&text[end..to]),
            cut_before: from > 0,
            cut_after: to < text.len(),
        }))
    }
}

/// The text around a mention in its document.
#[derive(Serialize)]
struct Mention {
    document: DocumentId,
    source: String,
    key: String,
    start: u64,
    end: u64,
    before: String,
    text: String,
    after: String,
    /// Whether the document goes on before `before` and after `after`.
    cut_before: bool,
    cut_after: bool,
}

/// The document a mention was found in, or `None` for a record that is
/// not a mention.
fn mention_document(record: &RecordEntry) -> Option<DocumentId> {
    record.evidence.as_ref()?["document"].as_str()?.parse().ok()
}

/// The members of a JSON object, one per line. Anything else on one line.
fn print_json_object(value: &Value) {
    match value.as_object() {
        Some(members) => {
            let rows: Vec<Vec<String>> = members
                .iter()
                .map(|(name, value)| vec![name.clone(), format_json_value(value)])
                .collect();
            aligned(&rows, 2);
        }
        None => println!("  {}", format_json_value(value)),
    }
}
