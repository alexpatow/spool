//! `spool load`: sends the rows of a file to a running server.
//!
//! The file is read here and sent as Arrow, a part at a time, so a file of
//! any size goes in requests the server takes. What the server does with a
//! part is what it does with any ingest.

use std::io::Read;
use std::path::Path;

use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use serde_json::{Value, json};
use spool::{DeclaredSource, IngestReport, RunStatus};
use spool_cli::api::{ARROW_STREAM, Info};
use spool_cli::delimited::{EMPTY, Reader};

use super::remote::{self, Remote, WAIT_MS, print_json, unsuccessful};
use super::show::{count, fields};
use crate::Failure;
use crate::args::Spec;

pub const USAGE: &str = "\
Usage: spool load <file> [options]

Sends the rows of a CSV file to a running server, which keeps each row as a
record. The first line of the file names the columns, and each has to be a
column the source lists in its sources/<name>.yaml. Every value is text, and
an empty field is a missing value.

Loading does not match: `spool match` places the records in identities.
Loading the same rows again adds nothing. A row that comes again under the
same key with other content is a new version of that row.

  <file>             the file to read; `-` reads standard input. A file whose
                     name ends in .tsv or .tab has tabs between its fields

Options:
  --source <name>    the source the rows come from (default: the only source
                     the project declares, or the one the file is named
                     after: crm-1.csv holds rows of `crm`)
  --type <name>      the entity type they are records of (default: the only
                     type whose mapping reads the source)
  --delimiter <c>    the character between the fields, or `tab` (default: a
                     comma, or a tab for .tsv and .tab)
  --batch <n>        send at most this many rows in one request (default: as
                     many as make about 32 MiB)
  --url <url>        the server (default: $SPOOL_URL, or http://127.0.0.1:7765)
  --token <token>    a token of that server (default: $SPOOL_TOKEN); `spool dev`
                     asks for none
  --json             print the ingest runs the server answered with, as JSON
";

/// About how much one request holds. A server reads 256 MiB unless it was
/// told otherwise, and this is well inside that.
const PART_BYTES: usize = 32 * 1024 * 1024;

/// The characters a file may have between its fields when it is not the
/// one it was read with.
const OTHER_DELIMITERS: [char; 4] = [';', '\t', '|', ','];

pub fn load(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &[
                "--source",
                "--type",
                "--delimiter",
                "--batch",
                remote::VALUES[0],
                remote::VALUES[1],
            ],
            flags: &["--json"],
            positional: 1,
        },
    )?;
    let Some(file) = arguments.positional.first() else {
        return Err(Failure::Usage("name the file to load".to_owned()));
    };
    let most_rows = match arguments.number("--batch").map_err(Failure::Usage)? {
        Some(0) => return Err(Failure::Usage("`--batch` needs at least 1".to_owned())),
        Some(rows) => usize::try_from(rows).unwrap_or(usize::MAX),
        None => usize::MAX,
    };
    let delimiter = delimiter(arguments.value("--delimiter"), file)?;

    // The file first: what is wrong with it can be said without a server.
    let (input, name) = open(file)?;
    let unreadable = |why: &dyn std::fmt::Display| {
        Failure::Failed(format!(
            "{name} cannot be read as {}: {why}",
            format(delimiter)
        ))
    };
    // A text file that was not said to hold rows may be a document.
    let may_be_text = arguments.value("--delimiter").is_none() && named_as_text(file);
    let mut reader = Reader::new(input, delimiter).map_err(|why| {
        let (Failure::Failed(said) | Failure::Usage(said)) = unreadable(&why);
        Failure::Failed(match may_be_text && why.0 != EMPTY {
            true => format!("{said}\n{}", extract_reads(&name)),
            false => said,
        })
    })?;
    let columns: Vec<String> = reader
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect();

    let remote = Remote::new(&arguments);
    let info = remote.info()?;
    // A first line that names no column of any source is prose, unless
    // it is the columns with another character between them.
    let listed = |column: &str| {
        let lists = |source: &DeclaredSource| source.columns.iter().any(|known| known == column);
        info.store.sources.iter().any(lists)
    };
    if may_be_text
        && !columns.iter().any(|column| {
            listed(column)
                || column
                    .split(OTHER_DELIMITERS)
                    .any(|part| listed(part.trim()))
        })
    {
        return Err(Failure::Failed(format!(
            "{name} does not look like rows: its first line names no column that a source of \
             the project lists. `spool load` reads CSV, with the names of the columns on its \
             first line.\n{}",
            extract_reads(&name)
        )));
    }
    let (source, entity_type) = target(
        &info,
        (file != "-").then_some(file.as_str()),
        arguments.value("--source"),
        arguments.value("--type"),
    )?;
    check_columns(&name, &columns, delimiter, source)?;

    let json = arguments.flag("--json");
    if let Some(key) = &source.key
        && !columns.contains(key)
        && !json
    {
        eprintln!(
            "note: {name} has no column `{key}`, which is the key of source `{}`. Its rows are \
             kept without a key: none can be named as {}:<key>, and a changed row does not \
             replace the one loaded before.",
            source.name, source.name
        );
    }

    // What is kept of a part is its counts, so that a file of any size is
    // loaded in the memory one part takes. Only `--json` prints the runs.
    let mut runs: Vec<Value> = Vec::new();
    let mut total = Total::default();
    loop {
        let batch = match reader.next_batch(most_rows, PART_BYTES) {
            Ok(Some(batch)) => batch,
            Ok(None) => break,
            Err(why) => return Err(total.before(unreadable(&why), &name)),
        };
        if total.rows > 0 && !json {
            eprintln!("{} sent", count(total.rows, "row"));
        }
        let (answer, report) = send(&remote, &source.name, entity_type, &batch)
            .map_err(|failure| total.before(failure, &name))?;
        total.add(batch.num_rows() as u64, &report);
        if json {
            runs.push(answer);
        }
    }

    if json {
        print_json(&json!({
            "source": source.name,
            "entity_type": entity_type,
            "rows": total.rows,
            "runs": runs,
        }));
        return Ok(());
    }
    if total.rows == 0 {
        println!("loaded  0 rows: {name} has no rows after its first line");
        return Ok(());
    }
    let mut rows = vec![(
        "loaded",
        format!(
            "{} into source `{}`, type `{entity_type}`: {}",
            count(total.rows, "row"),
            source.name,
            total.outcome()
        ),
    )];
    let waiting = total.waiting();
    if waiting > 0 {
        let command = match info.store.entity_types.len() {
            0 | 1 => "spool match".to_owned(),
            _ => format!("spool match --type {entity_type}"),
        };
        let places = if waiting == 1 {
            "places it in an identity"
        } else {
            "places them in identities"
        };
        rows.push(("next", format!("`{command}` {places}")));
    }
    fields(&rows);
    Ok(())
}

/// What the parts of a file came to.
#[derive(Default)]
struct Total {
    rows: u64,
    /// Records created that are not a new version of a row.
    new: u64,
    versions: u64,
    held: u64,
    restored: u64,
    returned: u64,
    /// How many ingest runs there were, with the first and the last, for a
    /// failure that comes after some succeeded.
    runs: Option<(u64, u64, u64)>,
}

impl Total {
    fn add(&mut self, rows: u64, report: &IngestReport) {
        self.rows += rows;
        self.new += report.inserted.saturating_sub(report.superseded);
        self.versions += report.superseded;
        self.held += report.duplicates;
        self.restored += report.restored;
        self.returned += report.returned;
        self.runs = Some(match self.runs {
            Some((count, first, _)) => (count + 1, first, report.run.0),
            None => (1, report.run.0, report.run.0),
        });
    }

    /// What became of the rows, in words.
    fn outcome(&self) -> String {
        let mut parts = vec![format!("{} new", self.new)];
        if self.versions > 0 {
            parts.push(format!(
                "{} of a row held before",
                count(self.versions, "new version")
            ));
        }
        if self.held > 0 {
            parts.push(format!("{} already held", self.held));
        }
        if self.restored > 0 {
            parts.push(format!(
                "{} held again that the source had taken back",
                count(self.restored, "record")
            ));
        }
        parts.join(", ")
    }

    /// How many records the load left for a match run to place.
    fn waiting(&self) -> u64 {
        self.new + self.versions + self.restored.saturating_sub(self.returned)
    }

    /// A failure, with what was loaded before it if anything was.
    fn before(&self, failure: Failure, name: &str) -> Failure {
        let (Failure::Failed(why) | Failure::Usage(why)) = failure;
        if self.rows == 0 {
            return Failure::Failed(why);
        }
        let runs = match self.runs {
            Some((1, run, _)) => format!("run {run}"),
            Some((count, first, last)) => format!("{count} runs, {first} to {last}"),
            None => "no run".to_owned(),
        };
        Failure::Failed(format!(
            "{why}\nThis was after the first {} of {name} {} loaded, in {runs}. In what the \
             server says, rows are counted from the one after them. Loading the file again \
             sends them once more and adds nothing.",
            count(self.rows, "row"),
            if self.rows == 1 { "was" } else { "were" },
        ))
    }
}

/// Sends one part and waits for its ingest run.
fn send(
    remote: &Remote,
    source: &str,
    entity_type: &str,
    batch: &RecordBatch,
) -> Result<(Value, IngestReport), Failure> {
    let cannot_write = |error: arrow::error::ArrowError| {
        Failure::Failed(format!("the rows cannot be sent: {error}"))
    };
    let mut body = Vec::new();
    let mut writer = StreamWriter::try_new(&mut body, &batch.schema()).map_err(cannot_write)?;
    writer.write(batch).map_err(cannot_write)?;
    writer.finish().map_err(cannot_write)?;
    drop(writer);

    let answer = remote.send(
        "/ingest",
        &[
            ("source", source.to_owned()),
            ("entity_type", entity_type.to_owned()),
            ("wait_ms", WAIT_MS.to_string()),
        ],
        ARROW_STREAM,
        &body,
    )?;
    let unexpected = || {
        Failure::Failed(
            "the server took the rows and reported no ingest run; is it of this version?"
                .to_owned(),
        )
    };
    let (answer, run) = remote.ended(answer)?.ok_or_else(unexpected)?;
    if run.status != RunStatus::Succeeded {
        return Err(unsuccessful(&run));
    }
    let report = run.ingest.ok_or_else(unexpected)?;
    Ok((answer["run"].clone(), report))
}

/// The file's rows, and what to call the file.
fn open(file: &str) -> Result<(Box<dyn Read>, String), Failure> {
    if file == "-" {
        return Ok((
            Box::new(std::io::stdin().lock()),
            "standard input".to_owned(),
        ));
    }
    if Path::new(file).is_dir() {
        return Err(Failure::Failed(format!(
            "`{file}` is a directory: name a file of rows"
        )));
    }
    match std::fs::File::open(file) {
        Ok(opened) => Ok((Box::new(opened), file.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(Failure::Failed(format!("there is no file `{file}`")))
        }
        Err(error) => Err(Failure::Failed(format!(
            "`{file}` cannot be opened: {error}"
        ))),
    }
}

/// The character between the fields: the one that was asked for, or the one
/// the file's name says.
fn delimiter(asked: Option<&str>, file: &str) -> Result<u8, Failure> {
    match asked {
        Some("tab" | "\\t" | "\t") => Ok(b'\t'),
        Some(other) => match other.as_bytes() {
            [one] if one.is_ascii() && !matches!(one, b'"' | b'\n' | b'\r') => Ok(*one),
            _ => Err(Failure::Usage(format!(
                "`--delimiter` needs one character such as `;`, or `tab`, not `{other}`"
            ))),
        },
        None => {
            let extension = Path::new(file)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase);
            match extension.as_deref() {
                Some("tsv" | "tab") => Ok(b'\t'),
                _ => Ok(b','),
            }
        }
    }
}

/// Whether a file's name says it is text for a person to read, which
/// `spool extract` stores as a document.
fn named_as_text(file: &str) -> bool {
    Path::new(file)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["txt", "text", "md", "markdown"]
                .iter()
                .any(|text| extension.eq_ignore_ascii_case(text))
        })
}

/// Where a document goes: said of a file that is no rows.
fn extract_reads(name: &str) -> String {
    format!(
        "If it is a document, `spool extract {name}` reads it: an extractor finds the records \
         in its text."
    )
}

/// What a file with this character between its fields is called.
fn format(delimiter: u8) -> String {
    match delimiter {
        b',' => "CSV".to_owned(),
        b'\t' => "tab-separated text".to_owned(),
        other => format!("text with `{}` between its fields", other as char),
    }
}

/// The source the rows come from and the entity type they are records of:
/// what was asked for, or the only choice the project leaves. Of several
/// sources, it is the one `file` is named after.
fn target<'a>(
    info: &'a Info,
    file: Option<&str>,
    source: Option<&str>,
    entity_type: Option<&'a str>,
) -> Result<(&'a DeclaredSource, &'a str), Failure> {
    let sources = &info.store.sources;
    let quoted = |names: Vec<&str>| -> String {
        names
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let source_names = || quoted(sources.iter().map(|source| source.name.as_str()).collect());
    if sources.is_empty() {
        return Err(Failure::Failed(
            "the project declares no source, so there is nothing rows can be loaded as. A \
             source is a file sources/<name>.yaml that lists the columns of its rows."
                .to_owned(),
        ));
    }
    if let Some(entity_type) = entity_type
        && !info
            .store
            .entity_types
            .iter()
            .any(|declared| declared.name.as_str() == entity_type)
    {
        return Err(Failure::Failed(format!(
            "the project has no entity type `{entity_type}`. It has {}.",
            quoted(
                info.store
                    .entity_types
                    .iter()
                    .map(|declared| declared.name.as_str())
                    .collect()
            )
        )));
    }

    let reads = |source: &DeclaredSource, entity_type: &str| {
        source
            .entity_types
            .iter()
            .any(|reader| reader.as_str() == entity_type)
    };
    let source = match source {
        Some(name) => sources
            .iter()
            .find(|source| source.name == name)
            .ok_or_else(|| {
                Failure::Failed(format!(
                    "the project has no source `{name}`: there is no sources/{name}.yaml. It \
                     has {}.",
                    source_names()
                ))
            })?,
        None => {
            // The type that was asked for narrows the choice.
            let fitting: Vec<&DeclaredSource> = sources
                .iter()
                .filter(|source| entity_type.is_none_or(|entity_type| reads(source, entity_type)))
                .collect();
            match (fitting.as_slice(), entity_type) {
                ([only], _) => *only,
                ([], Some(entity_type)) => {
                    return Err(Failure::Failed(format!(
                        "the mapping of `{entity_type}` reads no source, so no rows can be \
                         loaded as `{entity_type}` records. Add a source to the `mapping` in \
                         entities/{entity_type}/entity.yaml."
                    )));
                }
                (several, _) => {
                    let names: Vec<&str> =
                        several.iter().map(|source| source.name.as_str()).collect();
                    let after = file.and_then(|file| remote::named_after(file, &names));
                    if let Some(after) = after
                        && let Some(named) = several.iter().find(|source| source.name == after)
                    {
                        *named
                    } else {
                        let has = match entity_type {
                            Some(entity_type) => format!("`{entity_type}` reads"),
                            None => "the project has".to_owned(),
                        };
                        let listed = remote::listed(&names);
                        return Err(Failure::Failed(match file {
                            Some(file) => format!(
                                "{has} the sources {listed}, and {file} is named after {}: say \
                                 which its rows come from with `--source`",
                                match names.len() {
                                    2 => "neither",
                                    _ => "none of them",
                                }
                            ),
                            None => format!(
                                "{has} the sources {listed}: say which the rows come from with \
                                 `--source`"
                            ),
                        }));
                    }
                }
            }
        }
    };

    let readers: Vec<&str> = source
        .entity_types
        .iter()
        .map(|reader| reader.as_str())
        .collect();
    let entity_type = match (entity_type, readers.as_slice()) {
        (Some(entity_type), _) if reads(source, entity_type) => entity_type,
        (Some(entity_type), []) => {
            return Err(Failure::Failed(format!(
                "the mapping of `{entity_type}` has nothing for source `{}`: add it to \
                 entities/{entity_type}/entity.yaml",
                source.name
            )));
        }
        (Some(entity_type), _) => {
            return Err(Failure::Failed(format!(
                "the mapping of `{entity_type}` has nothing for source `{}`, so its rows \
                 cannot be `{entity_type}` records. They can be loaded as {}.",
                source.name,
                quoted(readers)
            )));
        }
        (None, [only]) => *only,
        (None, []) => {
            return Err(Failure::Failed(format!(
                "no entity type reads source `{}`, so its rows cannot be loaded yet. Add the \
                 source to the `mapping` in an entities/<type>/entity.yaml.",
                source.name
            )));
        }
        (None, _) => {
            return Err(Failure::Failed(format!(
                "say which entity type the rows are records of with `--type`: source `{}` is \
                 read by {}",
                source.name,
                quoted(readers)
            )));
        }
    };
    Ok((source, entity_type))
}

/// Refuses a file with a column its source does not list, as the server
/// would, before any of it is sent.
fn check_columns(
    name: &str,
    columns: &[String],
    delimiter: u8,
    source: &DeclaredSource,
) -> Result<(), Failure> {
    let unknown: Vec<String> = columns
        .iter()
        .filter(|column| !source.columns.contains(column))
        .map(|column| format!("`{column}`"))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let listed = match source.columns.as_slice() {
        [] => "no columns".to_owned(),
        columns => columns
            .iter()
            .map(|column| format!("`{column}`"))
            .collect::<Vec<_>>()
            .join(", "),
    };
    // A first line that was not split is one column with the other
    // character in it.
    if let [only] = columns
        && let Some(other) = OTHER_DELIMITERS
            .into_iter()
            .find(|other| *other as u32 != u32::from(delimiter) && only.contains(*other))
    {
        let (between, option) = match other {
            '\t' => ("a tab".to_owned(), "tab".to_owned()),
            other => (format!("`{other}`"), format!("'{other}'")),
        };
        return Err(Failure::Failed(format!(
            "the first line of {name} is one column, `{only}`, and sources/{}.yaml lists \
             {listed}. If {between} stands between its fields, add `--delimiter {option}`.",
            source.name,
        )));
    }
    Err(Failure::Failed(format!(
        "{name} has {} {}, which sources/{}.yaml does not list. It lists {listed}. Add {} to \
         that file to keep {}, or leave {} out of the rows.",
        if unknown.len() == 1 {
            "a column"
        } else {
            "the columns"
        },
        unknown.join(", "),
        source.name,
        if unknown.len() == 1 {
            "the column"
        } else {
            "them"
        },
        if unknown.len() == 1 { "it" } else { "them" },
        if unknown.len() == 1 { "it" } else { "them" },
    )))
}
