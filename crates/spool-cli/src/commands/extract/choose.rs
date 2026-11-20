//! Choosing the extractor that reads, and the source the documents
//! belong to.

use spool::{DeclaredSource, Extractor};
use spool_cli::text::listed;

use super::File;
use crate::Failure;
use crate::commands::remote;

/// The extractor that was named, or the project's only one.
pub(super) fn choose_extractor<'a>(
    known: &'a [Extractor],
    named: Option<&str>,
) -> Result<&'a Extractor, Failure> {
    let names: Vec<&str> = known
        .iter()
        .map(|extractor| extractor.name.as_str())
        .collect();
    match (named, known) {
        (_, []) => Err(Failure::failed(
            "the project has no extractors: one is a file under extractors/, and \
             `spool init <dir> --example companies` makes a project with a sample"
                .to_owned(),
        )),
        (Some(named), _) => known
            .iter()
            .find(|extractor| extractor.name == named)
            .ok_or_else(|| {
                Failure::failed(format!(
                    "the project has no extractor `{named}`: it has {}",
                    listed(&names)
                ))
            }),
        (None, [only]) => Ok(only),
        (None, _) => Err(Failure::failed(format!(
            "the project has the extractors {}: say which with `--extractor`",
            listed(&names)
        ))),
    }
}

/// The source the documents belong to: the one that was named, the
/// project's only source, or the one every file is named after. `None` when
/// none was named and there is no file: the extractor then reads the
/// documents of every source.
pub(super) fn source_of(
    sources: &[DeclaredSource],
    named: Option<&str>,
    files: &[File],
) -> Result<Option<String>, Failure> {
    let names: Vec<&str> = sources.iter().map(|source| source.name.as_str()).collect();
    if let Some(named) = named {
        if !names.is_empty() && !names.contains(&named) {
            return Err(Failure::failed(format!(
                "the project has no source `{named}`: there is no sources/{named}.yaml. It has \
                 {}.",
                listed(&names)
            )));
        }
        return Ok(Some(named.to_owned()));
    }
    if files.is_empty() {
        return Ok(None);
    }
    match names.as_slice() {
        [] => {
            return Err(Failure::failed(
                "the project declares no source, so there is nothing a document can belong to. \
                 A source is a file sources/<name>.yaml that lists what a record found in its \
                 documents may say."
                    .to_owned(),
            ));
        }
        [only] => return Ok(Some((*only).to_owned())),
        _ => {}
    }
    let mut found: Option<&str> = None;
    for file in files {
        let Some(source) = remote::named_after(&file.name, &names) else {
            return Err(Failure::failed(format!(
                "the project has the sources {}, and {} is named after {}: say which its \
                 documents belong to with `--source`",
                listed(&names),
                file.name,
                match names.len() {
                    2 => "neither",
                    _ => "none of them",
                }
            )));
        };
        match found {
            Some(first) if first != source => {
                return Err(Failure::failed(format!(
                    "the files are named after different sources, `{first}` and `{source}`: \
                     extract them one source at a time, or say which they all belong to with \
                     `--source`"
                )));
            }
            _ => found = Some(source),
        }
    }
    Ok(found.map(str::to_owned))
}

/// Refuses a source in whose documents the extractor can find no record,
/// before anything is stored: what an extractor finds is a record of an
/// entity type, and the type's mapping has to say what it reads from the
/// source.
pub(super) fn check_source_readable(
    sources: &[DeclaredSource],
    source: &str,
) -> Result<(), Failure> {
    let Some(declared) = sources.iter().find(|declared| declared.name == source) else {
        return Ok(());
    };
    if declared.entity_types.is_empty() {
        return Err(Failure::failed(format!(
            "no entity type reads source `{source}`, so nothing found in its documents can be \
             a record. Add the source to the `mapping` in an entities/<type>/entity.yaml."
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use spool::DeclaredSource;

    use super::{File, source_of};

    /// What `source_of` says of files called `files` in a project with
    /// `sources`: the source, or why there is none.
    fn describe_source_choice(sources: &[&str], named: Option<&str>, files: &[&str]) -> String {
        let sources: Vec<DeclaredSource> = sources
            .iter()
            .map(|name| DeclaredSource {
                name: (*name).to_owned(),
                key: None,
                evidence: None,
                columns: vec!["name".to_owned()],
                entity_types: Vec::new(),
            })
            .collect();
        let files: Vec<File> = files
            .iter()
            .map(|name| File {
                path: format!("samples/{name}"),
                name: (*name).to_owned(),
                text: "text".to_owned(),
            })
            .collect();
        match source_of(&sources, named, &files) {
            Ok(source) => source.unwrap_or_else(|| "every source".to_owned()),
            Err(failure) => failure.message(),
        }
    }

    #[test]
    fn the_source_is_the_one_named_the_only_one_or_the_one_the_files_are_named_after() {
        assert_eq!(
            describe_source_choice(&["notes"], None, &["call.txt"]),
            "notes"
        );
        assert_eq!(
            describe_source_choice(&["crm", "notes"], None, &["notes-1.txt"]),
            "notes"
        );
        assert_eq!(
            describe_source_choice(&["crm", "notes"], None, &["notes-1.txt", "notes.md"]),
            "notes"
        );
        assert_eq!(
            describe_source_choice(&["crm", "notes"], Some("crm"), &["call.txt"]),
            "crm"
        );
        // With no file, the extractor reads what the server holds.
        assert_eq!(
            describe_source_choice(&["crm", "notes"], None, &[]),
            "every source"
        );
        assert_eq!(
            describe_source_choice(&["crm", "notes"], Some("notes"), &[]),
            "notes"
        );
    }

    #[test]
    fn a_source_that_cannot_be_told_is_asked_for() {
        assert_eq!(
            describe_source_choice(&["crm", "notes"], None, &["call.txt"]),
            "the project has the sources `crm` and `notes`, and call.txt is named after \
             neither: say which its documents belong to with `--source`"
        );
        assert_eq!(
            describe_source_choice(
                &["crm", "notes", "wire"],
                None,
                &["notes-1.txt", "call.txt"]
            ),
            "the project has the sources `crm`, `notes` and `wire`, and call.txt is named after \
             none of them: say which its documents belong to with `--source`"
        );
        assert_eq!(
            describe_source_choice(&["crm", "notes"], None, &["notes-1.txt", "crm-1.txt"]),
            "the files are named after different sources, `notes` and `crm`: extract them one \
             source at a time, or say which they all belong to with `--source`"
        );
        assert_eq!(
            describe_source_choice(&["crm", "notes"], Some("news"), &["call.txt"]),
            "the project has no source `news`: there is no sources/news.yaml. It has `crm` and \
             `notes`."
        );
        assert_eq!(
            describe_source_choice(&[], None, &["call.txt"]),
            "the project declares no source, so there is nothing a document can belong to. A \
             source is a file sources/<name>.yaml that lists what a record found in its \
             documents may say."
        );
    }
}
