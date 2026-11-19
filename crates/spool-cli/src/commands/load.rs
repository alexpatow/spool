//! `spool load`: sends the rows of a file to a running server.
//!
//! The file is read here and sent as Arrow, a part at a time, so a file of
//! any size goes in requests the server takes. What the server does with a
//! part is what it does with any load.

use std::io::Read;
use std::path::Path;

use anstream::println;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use indicatif::ProgressBar;
use serde_json::{Value, json};
use spool::{DeclaredSource, LoadReport, RunStatus};
use spool_cli::api::{ARROW_STREAM, Info};
use spool_cli::delimited::{EMPTY, Reader};

use super::remote::{self, Remote, RemoteArgs, WAIT_MS, unsuccessful};
use super::show::count;
use crate::Failure;
use crate::output::{Output, OutputArgs, fields, hint};
use crate::progress;

/// Send the rows of a file to a running server
///
/// Sends the rows of a CSV file to a running server, which keeps each row as
/// a record. The first line of the file names the columns, and each has to be
/// a column the source lists in its sources/<name>.yaml. Every value is text,
/// and an empty field is a missing value.
///
/// Loading does not match: `spool match` places the records in identities.
/// Loading the same rows again adds nothing. A row that comes again under the
/// same key with other content is a new version of that row.
#[derive(clap::Args)]
#[command(
    after_help = "Examples:\n  spool load samples/crm-1.csv\n  spool load export.tsv --source crm\n  cat rows.csv | spool load - --source crm"
)]
pub struct LoadArgs {
    /// The file to read; `-` reads standard input
    ///
    /// A file whose name ends in .tsv or .tab has tabs between its fields.
    file: String,
    /// The source the rows come from
    ///
    /// The default is the only source the project declares, the one the file
    /// is named after, as crm-1.csv is after `crm`, or else the one that lists
    /// most of the file's columns.
    #[arg(long, value_name = "NAME")]
    source: Option<String>,
    /// The entity type they are records of [default: the only type whose mapping reads the source]
    #[arg(long = "type", value_name = "NAME")]
    entity_type: Option<String>,
    /// The character between the fields, or `tab` [default: a comma, or a tab for .tsv and .tab]
    #[arg(long, value_name = "CHAR")]
    delimiter: Option<String>,
    /// Send at most this many rows in one request [default: as many as make about 32 MiB]
    #[arg(long, value_name = "ROWS", value_parser = clap::value_parser!(u64).range(1..))]
    batch: Option<u64>,
    #[command(flatten)]
    output: OutputArgs,
    #[command(flatten)]
    remote: RemoteArgs,
}

/// About how much one request holds. A server reads 256 MiB unless it was
/// told otherwise, and this is well inside that.
const PART_BYTES: usize = 32 * 1024 * 1024;

/// The characters a file may have between its fields when it is not the
/// one it was read with.
const OTHER_DELIMITERS: [char; 4] = [';', '\t', '|', ','];

pub fn load(arguments: LoadArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool load`")?;
    let file = &arguments.file;
    let most_rows = match arguments.batch {
        Some(rows) => usize::try_from(rows).unwrap_or(usize::MAX),
        None => usize::MAX,
    };
    let delimiter = delimiter(arguments.delimiter.as_deref(), file)?;

    // The file first: what is wrong with it can be said without a server.
    let (input, name) = open(file)?;
    let bar = match std::fs::metadata(file) {
        Ok(found) if file != "-" && found.is_file() => progress::bytes("Loading", found.len()),
        _ => progress::spinner(format!("Loading {name}")),
    };
    let input = Counted {
        inner: input,
        bar: bar.clone(),
    };
    let unreadable = |why: &dyn std::fmt::Display| {
        Failure::failed(format!(
            "{name} cannot be read as {}: {why}",
            format(delimiter)
        ))
    };
    // A text file that was not said to hold rows may be a document.
    let may_be_text = arguments.delimiter.is_none() && named_as_text(file);
    let mut reader = Reader::new(input, delimiter).map_err(|why| {
        let said = unreadable(&why);
        match may_be_text && why.0 != EMPTY {
            true => said.hint(extract_reads(&name)),
            false => said,
        }
    })?;
    let columns: Vec<String> = reader
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect();

    let remote = Remote::new(&arguments.remote);
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
        return Err(Failure::failed(format!(
            "{name} does not look like rows: its first line names no column that a source of \
             the project lists. `spool load` reads CSV, with the names of the columns on its \
             first line."
        ))
        .hint(extract_reads(&name)));
    }
    let (source, entity_type) = target(
        &info,
        (file != "-").then_some(file.as_str()),
        &columns,
        arguments.source.as_deref(),
        arguments.entity_type.as_deref(),
    )?;
    check_columns(&name, &columns, delimiter, source)?;

    let json = !out.is_table();
    if let Some(key) = &source.key
        && !columns.contains(key)
    {
        hint(format!(
            "note: {name} has no column `{key}`, which is the key of source `{}`. Its rows are \
             kept without a key: none can be named as {}:<key>, and a changed row does not \
             replace the one loaded before.",
            source.name, source.name
        ));
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
        let (answer, report) = send(&remote, &source.name, entity_type, &batch)
            .map_err(|failure| total.before(failure, &name))?;
        total.add(batch.num_rows() as u64, &report);
        bar.set_message(format!(", {} sent", count(total.rows, "row")));
        if json {
            runs.push(answer);
        }
    }

    bar.finish_and_clear();
    if json {
        out.json(&json!({
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
    fields(&[(
        "loaded",
        format!(
            "{} into source `{}`, type `{entity_type}`: {}",
            count(total.rows, "row"),
            source.name,
            total.outcome()
        ),
    )]);
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
        hint(format!("next: `{command}` {places}"));
    }
    Ok(())
}

/// The file's rows, read as they are sent, moving the bar as they are.
struct Counted {
    inner: Box<dyn Read>,
    bar: ProgressBar,
}

impl Read for Counted {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.bar.inc(read as u64);
        Ok(read)
    }
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
    /// How many load runs there were, with the first and the last, for a
    /// failure that comes after some succeeded.
    runs: Option<(u64, u64, u64)>,
}

impl Total {
    fn add(&mut self, rows: u64, report: &LoadReport) {
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
        if self.rows == 0 {
            return failure;
        }
        let runs = match self.runs {
            Some((1, run, _)) => format!("run {run}"),
            Some((count, first, last)) => format!("{count} runs, {first} to {last}"),
            None => "no run".to_owned(),
        };
        failure.and(format!(
            "This was after the first {} of {name} {} loaded, in {runs}. In what the server \
             says, rows are counted from the one after them. Loading the file again sends them \
             once more and adds nothing.",
            count(self.rows, "row"),
            if self.rows == 1 { "was" } else { "were" },
        ))
    }
}

/// Sends one part and waits for its load run.
fn send(
    remote: &Remote,
    source: &str,
    entity_type: &str,
    batch: &RecordBatch,
) -> Result<(Value, LoadReport), Failure> {
    let cannot_write = |error: arrow::error::ArrowError| {
        Failure::failed(format!("the rows cannot be sent: {error}"))
    };
    let mut body = Vec::new();
    let mut writer = StreamWriter::try_new(&mut body, &batch.schema()).map_err(cannot_write)?;
    writer.write(batch).map_err(cannot_write)?;
    writer.finish().map_err(cannot_write)?;
    drop(writer);

    let answer = remote.send(
        "/load",
        &[
            ("source", source.to_owned()),
            ("entity_type", entity_type.to_owned()),
            ("wait_ms", WAIT_MS.to_string()),
        ],
        ARROW_STREAM,
        &body,
    )?;
    let unexpected = || {
        Failure::failed(
            "the server took the rows and reported no load run; is it of this version?".to_owned(),
        )
    };
    let (answer, run) = remote.ended(answer)?.ok_or_else(unexpected)?;
    if run.status != RunStatus::Succeeded {
        return Err(unsuccessful(&run));
    }
    let report = run.load.ok_or_else(unexpected)?;
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
        return Err(Failure::failed(format!(
            "`{file}` is a directory: name a file of rows"
        )));
    }
    match std::fs::File::open(file) {
        Ok(opened) => Ok((Box::new(opened), file.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(Failure::failed(format!("there is no file `{file}`")))
        }
        Err(error) => Err(Failure::failed(format!(
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
            _ => Err(Failure::usage(format!(
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
        "if it is a document, `spool extract {name}` reads it: an extractor finds the records \
         in its text"
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
/// sources, it is the one `file` is named after, or else the one that lists
/// more of the file's `columns` than any other: a new project has a source
/// of rows and a source of what is found in documents, and a file of rows
/// is the first's whatever it is called.
fn target<'a>(
    info: &'a Info,
    file: Option<&str>,
    columns: &[String],
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
        return Err(Failure::failed(
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
        return Err(Failure::failed(format!(
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
                Failure::failed(format!(
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
                    return Err(Failure::failed(format!(
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
                    } else if let Some(fits) = best_fit(several, columns) {
                        fits
                    } else {
                        let has = match entity_type {
                            Some(entity_type) => format!("`{entity_type}` reads"),
                            None => "the project has".to_owned(),
                        };
                        let listed = remote::listed(&names);
                        return Err(Failure::failed(match file {
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
            return Err(Failure::failed(format!(
                "the mapping of `{entity_type}` has nothing for source `{}`: add it to \
                 entities/{entity_type}/entity.yaml",
                source.name
            )));
        }
        (Some(entity_type), _) => {
            return Err(Failure::failed(format!(
                "the mapping of `{entity_type}` has nothing for source `{}`, so its rows \
                 cannot be `{entity_type}` records. They can be loaded as {}.",
                source.name,
                quoted(readers)
            )));
        }
        (None, [only]) => *only,
        (None, []) => {
            return Err(Failure::failed(format!(
                "no entity type reads source `{}`, so its rows cannot be loaded yet. Add the \
                 source to the `mapping` in an entities/<type>/entity.yaml.",
                source.name
            )));
        }
        (None, _) => {
            return Err(Failure::failed(format!(
                "say which entity type the rows are records of with `--type`: source `{}` is \
                 read by {}",
                source.name,
                quoted(readers)
            )));
        }
    };
    Ok((source, entity_type))
}

/// The source that lists more of a file's columns than any other does, if
/// one does. A first line that another character divides counts by its
/// parts, so that the file is still told whose it is, and then how to read
/// it.
fn best_fit<'a>(sources: &[&'a DeclaredSource], columns: &[String]) -> Option<&'a DeclaredSource> {
    let listed = |source: &DeclaredSource| {
        columns
            .iter()
            .flat_map(|column| column.split(OTHER_DELIMITERS))
            .filter(|part| source.columns.iter().any(|known| known == part.trim()))
            .count()
    };
    let counts: Vec<usize> = sources.iter().map(|source| listed(source)).collect();
    let most = counts.iter().copied().max()?;
    let mut best = sources
        .iter()
        .zip(&counts)
        .filter(|(_, count)| **count == most);
    match (most, best.next(), best.next()) {
        (1.., Some((only, _)), None) => Some(*only),
        _ => None,
    }
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
        return Err(Failure::failed(format!(
            "the first line of {name} is one column, `{only}`, and sources/{}.yaml lists \
             {listed}. If {between} stands between its fields, add `--delimiter {option}`.",
            source.name,
        )));
    }
    Err(Failure::failed(format!(
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
