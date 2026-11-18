//! `spool extract`: gives a running server documents and has one of the
//! project's extractors read them.

use std::collections::HashSet;
use std::path::Path;

use serde_json::{Value, json};
use spool::core::DocumentId;
use spool::{DeclaredSource, Extractor, RecordEntry, RunEntry, RunStatus};

use super::remote::{self, Remote, WAIT_MS, listed, print_json, typed};
use crate::Failure;
use crate::args::Spec;

pub const USAGE: &str = "\
Usage: spool extract [<file>...] [options]

Finds records in text, on a running server. Each file is one document: the
server stores its text under the file's name, and one of the project's
extractors reads it. What the extractor finds are records like the rows of a
source, and the next match run places them in identities.

With no file, the extractor reads the documents the server holds and it has
not read.

An extractor's first run may fetch what it depends on, a model for one. When
a run takes more than a few seconds, the command says on standard error which
file is running, and what the file says as it goes.

One run reads every document the extractor has not read, and the server stops
a run that takes longer than it lets user code run: 600 seconds, unless
`--task-timeout` said otherwise when the server was started. A run that is
stopped finds nothing, and the documents stay stored for the next one.

  <file>               a file of UTF-8 text

Options:
  --source <name>      the source the documents belong to (default: the
                       project's only source, or the one the files are named
                       after, as notes-1.txt is after `notes`)
  --extractor <name>   the extractor (default: the project's only one)
  --again              read every document of the source, and not only the
                       ones the extractor has not read
  --url <url>          the server (default: $SPOOL_URL, or http://127.0.0.1:7765)
  --token <token>      a token of that server (default: $SPOOL_TOKEN); `spool dev`
                       asks for none
  --json               print the runs the server answered with, as JSON
";

/// The most records that are listed.
const LISTED: usize = 20;

/// How long an extraction is waited for in silence. One that takes longer
/// is followed: what its extractor says is printed as it says it.
const QUIET_MS: u64 = 3_000;

/// How often a running extractor is asked what it has said.
const ASK_MS: u64 = 1_000;

/// The most of a found text that a line shows, in characters.
const SHOWN: usize = 60;

/// A file to store as a document.
struct File {
    /// As it was named on the command line.
    path: String,
    /// The file's name, which is the document's key.
    name: String,
    text: String,
}

pub fn extract(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &[
                "--source",
                "--extractor",
                remote::VALUES[0],
                remote::VALUES[1],
            ],
            flags: &["--again", "--json"],
            positional: usize::MAX,
        },
    )?;
    let files = read(&arguments.positional)?;

    let remote = Remote::new(&arguments);
    let info = remote.info()?.store;
    let extractor = extractor(&info.extractors, arguments.value("--extractor"))?;
    let source = source_of(&info.sources, arguments.value("--source"), &files)?;
    if let Some(source) = &source {
        readable(&info.sources, source)?;
    }
    let as_json = arguments.flag("--json");

    let mut loaded = Value::Null;
    if let Some(source) = &source
        && !files.is_empty()
    {
        let documents: Vec<Value> = files
            .iter()
            .map(|file| json!({ "key": file.name, "text": file.text }))
            .collect();
        let (answer, run) = finish(
            &remote,
            remote.post(
                "/documents",
                &json!({ "source": source, "documents": documents, "wait_ms": WAIT_MS }),
            )?,
            as_json,
        )?;
        let report = run
            .load
            .as_ref()
            .ok_or_else(|| unreported(&run, "what it stored"))?;
        if !as_json {
            println!("source     {source}");
            println!(
                "read       {} from {}",
                plural(files.len() as u64, "document", "documents"),
                match files.as_slice() {
                    [file] => file.path.clone(),
                    files => plural(files.len() as u64, "file", "files"),
                }
            );
            println!(
                "loaded     {} new, {} already held, {}",
                report.inserted.saturating_sub(report.superseded),
                report.duplicates,
                plural(report.superseded, "new version", "new versions"),
            );
        }
        loaded = answer;
    }

    let started = remote.post(
        "/extractions",
        &json!({
            "extractor": extractor.name,
            "source": source,
            "again": arguments.flag("--again"),
            "wait_ms": QUIET_MS,
        }),
    )?;
    let (answer, run) = finish(&remote, follow(&remote, started, extractor)?, as_json)?;
    if as_json {
        print_json(&json!({ "documents": loaded["run"], "extraction": answer["run"] }));
        return Ok(());
    }
    let report = run
        .extraction
        .as_ref()
        .ok_or_else(|| unreported(&run, "what it found"))?;
    println!("extractor  {}", extractor.name);
    if report.documents == 0 {
        let name = &extractor.name;
        println!(
            "found      0 records: {}",
            if arguments.flag("--again") {
                match &source {
                    Some(source) => format!("the server holds no document of `{source}`."),
                    None => "the server holds no document.".to_owned(),
                }
            } else if info.documents == 0 && files.is_empty() {
                "the server holds no document. `spool extract <file>` gives it one.".to_owned()
            } else {
                format!("`{name}` has read every document. `--again` reads them again.")
            }
        );
        return Ok(());
    }
    println!(
        "found      {} in {}: {} new, {} already held{}",
        plural(report.mentions, "record", "records"),
        plural(report.documents, "document", "documents"),
        report.inserted.saturating_sub(report.superseded),
        report.duplicates,
        match report.superseded {
            0 => String::new(),
            changed => format!(", {changed} changed"),
        }
    );

    if let Some(source) = &source
        && !files.is_empty()
    {
        list(&remote, source, &files, report.mentions)?;
    }
    if report.inserted > 0 {
        println!(
            "\nnext       `spool match` places {} in identities",
            if report.inserted == 1 { "it" } else { "them" }
        );
    }
    Ok(())
}

/// Reads the files, each of which is one document.
fn read(paths: &[String]) -> Result<Vec<File>, Failure> {
    let mut names = HashSet::new();
    let mut files = Vec::new();
    for path in paths {
        let bytes =
            std::fs::read(path).map_err(|error| Failure::Failed(format!("{path}: {error}")))?;
        let text = String::from_utf8(bytes).map_err(|_| {
            Failure::Failed(format!(
                "{path} is not text: a document is stored as UTF-8 text. Turn a PDF or a web \
                 page into text first."
            ))
        })?;
        if text.trim().is_empty() {
            return Err(Failure::Failed(format!("{path} is empty")));
        }
        let name = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| Failure::Failed(format!("{path} does not name a file")))?;
        // A document is stored under its file's name, and a second document
        // under the same name would be taken for a new version of the first.
        if !names.insert(name.clone()) {
            return Err(Failure::Failed(format!(
                "two of the files are called {name}, and a document is stored under its file's \
                 name: extract them one at a time, from sources of their own"
            )));
        }
        files.push(File {
            path: path.clone(),
            name,
            text,
        });
    }
    Ok(files)
}

/// The extractor that was named, or the project's only one.
fn extractor<'a>(known: &'a [Extractor], named: Option<&str>) -> Result<&'a Extractor, Failure> {
    let names: Vec<&str> = known
        .iter()
        .map(|extractor| extractor.name.as_str())
        .collect();
    match (named, known) {
        (_, []) => Err(Failure::Failed(
            "the project has no extractors: one is a file under extractors/, and \
             `spool init <dir> --example companies` makes a project with a sample"
                .to_owned(),
        )),
        (Some(named), _) => known
            .iter()
            .find(|extractor| extractor.name == named)
            .ok_or_else(|| {
                Failure::Failed(format!(
                    "the project has no extractor `{named}`: it has {}",
                    listed(&names)
                ))
            }),
        (None, [only]) => Ok(only),
        (None, _) => Err(Failure::Failed(format!(
            "the project has the extractors {}: say which with `--extractor`",
            listed(&names)
        ))),
    }
}

/// The source the documents belong to: the one that was named, the
/// project's only source, or the one every file is named after. `None` when
/// none was named and there is no file: the extractor then reads the
/// documents of every source.
fn source_of(
    sources: &[DeclaredSource],
    named: Option<&str>,
    files: &[File],
) -> Result<Option<String>, Failure> {
    let names: Vec<&str> = sources.iter().map(|source| source.name.as_str()).collect();
    if let Some(named) = named {
        if !names.is_empty() && !names.contains(&named) {
            return Err(Failure::Failed(format!(
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
            return Err(Failure::Failed(
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
            return Err(Failure::Failed(format!(
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
                return Err(Failure::Failed(format!(
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
fn readable(sources: &[DeclaredSource], source: &str) -> Result<(), Failure> {
    let Some(declared) = sources.iter().find(|declared| declared.name == source) else {
        return Ok(());
    };
    if declared.entity_types.is_empty() {
        return Err(Failure::Failed(format!(
            "no entity type reads source `{source}`, so nothing found in its documents can be \
             a record. Add the source to the `mapping` in an entities/<type>/entity.yaml."
        )));
    }
    Ok(())
}

/// Waits for an extraction that did not end at once, and prints what its
/// extractor says while it runs: the first run of a Python file takes as
/// long as its downloads, and this is all there is to see of them. Returns
/// the server's answer once the run has ended.
///
/// The lines go to standard error, so that what is printed to standard
/// output is the same for a run that took long.
fn follow(remote: &Remote, mut answer: Value, extractor: &Extractor) -> Result<Value, Failure> {
    let mut last: Option<String> = None;
    let mut announced = false;
    loop {
        if answer["run"].is_null() {
            return Ok(answer);
        }
        let run: RunEntry = typed(&answer["run"])?;
        if run.status != RunStatus::Running {
            return Ok(answer);
        }
        if !announced {
            eprintln!("running    {}", extractor.file);
            announced = true;
        }
        if let Some(said) = run.said
            && last.as_ref() != Some(&said)
        {
            eprintln!("           {said}");
            last = Some(said);
        }
        answer = remote.get(
            &format!("/runs/{}", run.run),
            &[("wait_ms", ASK_MS.to_string())],
            || format!("the server no longer has run {}", run.run),
        )?;
    }
}

/// Waits for the run a call started, and fails as the run did.
fn finish(remote: &Remote, answer: Value, as_json: bool) -> Result<(Value, RunEntry), Failure> {
    let (answer, run) = remote.ended(answer)?.ok_or_else(|| {
        Failure::Failed(
            "the server started no run; is it a Spool server of this version?".to_owned(),
        )
    })?;
    if run.status != RunStatus::Succeeded {
        if as_json {
            print_json(&answer);
        }
        return Err(remote::unsuccessful(&run));
    }
    Ok((answer, run))
}

/// Prints the records of the files' documents, in the order of the files
/// and of the text. They are what the extractor found there, and what any
/// other extractor found there before it.
fn list(remote: &Remote, source: &str, files: &[File], mentions: u64) -> Result<(), Failure> {
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
    let width = |column: fn(&(String, String, String)) -> &String| {
        lines
            .iter()
            .take(LISTED)
            .map(|line| column(line).chars().count())
            .max()
            .unwrap_or(0)
    };
    let (types, texts) = (width(|line| &line.0), width(|line| &line.1));
    println!();
    for (entity_type, text, place) in lines.iter().take(LISTED) {
        println!("  {entity_type:<types$}  {text:<texts$}  {place}");
    }
    let listed = lines.len().min(LISTED) as u64;
    // The extractor read whatever the source held that it had not read, and
    // that may be more than the files.
    if mentions > listed || lines.len() > LISTED {
        println!(
            "  and more: `spool show records --source {source}` lists every record of the source"
        );
    }
    Ok(())
}

/// A found text on one line, cut short when it is long.
fn shown(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= SHOWN {
        return line;
    }
    let mut cut: String = line.chars().take(SHOWN - 1).collect();
    cut.push('…');
    cut
}

fn plural(number: u64, one: &str, many: &str) -> String {
    format!("{number} {}", if number == 1 { one } else { many })
}

fn unreported(run: &RunEntry, what: &str) -> Failure {
    Failure::Failed(format!(
        "run {} succeeded and did not report {what}; is the server of this version?",
        run.run
    ))
}

#[cfg(test)]
mod tests {
    use spool::DeclaredSource;

    use super::{File, source_of};
    use crate::Failure;

    /// What `source_of` says of files called `files` in a project with
    /// `sources`: the source, or why there is none.
    fn said(sources: &[&str], named: Option<&str>, files: &[&str]) -> String {
        let sources: Vec<DeclaredSource> = sources
            .iter()
            .map(|name| DeclaredSource {
                name: (*name).to_owned(),
                key: None,
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
            Err(Failure::Failed(message) | Failure::Usage(message)) => message,
        }
    }

    #[test]
    fn the_source_is_the_one_named_the_only_one_or_the_one_the_files_are_named_after() {
        assert_eq!(said(&["notes"], None, &["call.txt"]), "notes");
        assert_eq!(said(&["crm", "notes"], None, &["notes-1.txt"]), "notes");
        assert_eq!(
            said(&["crm", "notes"], None, &["notes-1.txt", "notes.md"]),
            "notes"
        );
        assert_eq!(said(&["crm", "notes"], Some("crm"), &["call.txt"]), "crm");
        // With no file, the extractor reads what the server holds.
        assert_eq!(said(&["crm", "notes"], None, &[]), "every source");
        assert_eq!(said(&["crm", "notes"], Some("notes"), &[]), "notes");
    }

    #[test]
    fn a_source_that_cannot_be_told_is_asked_for() {
        assert_eq!(
            said(&["crm", "notes"], None, &["call.txt"]),
            "the project has the sources `crm` and `notes`, and call.txt is named after \
             neither: say which its documents belong to with `--source`"
        );
        assert_eq!(
            said(
                &["crm", "notes", "wire"],
                None,
                &["notes-1.txt", "call.txt"]
            ),
            "the project has the sources `crm`, `notes` and `wire`, and call.txt is named after \
             none of them: say which its documents belong to with `--source`"
        );
        assert_eq!(
            said(&["crm", "notes"], None, &["notes-1.txt", "crm-1.txt"]),
            "the files are named after different sources, `notes` and `crm`: extract them one \
             source at a time, or say which they all belong to with `--source`"
        );
        assert_eq!(
            said(&["crm", "notes"], Some("news"), &["call.txt"]),
            "the project has no source `news`: there is no sources/news.yaml. It has `crm` and \
             `notes`."
        );
        assert_eq!(
            said(&[], None, &["call.txt"]),
            "the project declares no source, so there is nothing a document can belong to. A \
             source is a file sources/<name>.yaml that lists what a record found in its \
             documents may say."
        );
    }
}
