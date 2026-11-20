//! What the command says at its end: what a stopped one left, and the
//! records found in the files.

use anstream::println;
use spool::RecordEntry;
use spool::core::{DocumentId, RunId};
use spool_cli::text::count;

use super::{File, Found, LISTED, SHOWN};
use crate::Failure;
use crate::commands::remote::{Remote, typed};
use crate::output::{aligned, hint};

/// What a command stopped with Ctrl-C says it left: the documents read
/// before it stopped are kept, and those of the run it cancelled are not.
pub(super) fn stopped(found: &Found, again: bool, cancelled: Option<RunId>) -> String {
    let mut said = String::from("stopped");
    if let Some(run) = cancelled {
        said.push_str(&format!(
            ": run {run} was cancelled, and nothing it read is kept"
        ));
    }
    let kept = count(found.documents, "document");
    let are = if found.documents == 1 { "is" } else { "are" };
    match (again, found.documents) {
        (false, 0) => said.push_str(". The same command again reads the documents."),
        (false, _) => said.push_str(&format!(
            ". The {kept} read before it {are} kept, and the same command again reads the rest."
        )),
        (true, 0) => said.push('.'),
        (true, _) => said.push_str(&format!(
            ". The {kept} read again before it {are} kept. `--again` starts over."
        )),
    }
    said
}

/// Prints the records of the files' documents, in the order of the files
/// and of the text. They are what the extractor found there, and what any
/// other extractor found there before it.
pub(super) fn list(
    remote: &Remote,
    source: &str,
    files: &[File],
    mentions: u64,
) -> Result<(), Failure> {
    let mut lines = Vec::new();
    for file in files {
        let document = DocumentId::derive(source, &file.name, &file.text);
        let mut after = None;
        let mut found = Vec::new();
        loop {
            let mut query = vec![("document", document.to_string())];
            if let Some(after) = after {
                query.push(("after", after));
            }
            let page = remote.list("/records", &query)?;
            let records: Vec<RecordEntry> = typed(&page["records"])?;
            found.extend(
                records
                    .into_iter()
                    .filter(|record| record.superseded_by.is_none()),
            );
            match page["next"].as_str() {
                Some(next) => after = Some(next.to_owned()),
                None => break,
            }
        }
        for record in &found {
            let text = record
                .evidence
                .as_ref()
                .and_then(|evidence| evidence["text"].as_str())
                .unwrap_or_default();
            lines.push((record.entity_type.to_string(), shown(text), record.name()));
        }
    }
    if lines.is_empty() {
        return Ok(());
    }
    let rows: Vec<Vec<String>> = lines
        .iter()
        .take(LISTED)
        .map(|(entity_type, text, place)| vec![entity_type.clone(), text.clone(), place.clone()])
        .collect();
    println!();
    aligned(&rows, 2);
    let listed = lines.len().min(LISTED) as u64;
    // The extractor read whatever the source held that it had not read, and
    // that may be more than the files.
    if mentions > listed || lines.len() > LISTED {
        hint(format!(
            "and more: `spool show records --source {source}` lists every record of the source"
        ));
    }
    Ok(())
}

/// A found text on one line, cut short when it is long.
pub(super) fn shown(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= SHOWN {
        return line;
    }
    let mut cut: String = line.chars().take(SHOWN - 1).collect();
    cut.push('…');
    cut
}
