//! What a command prints, and where.
//!
//! Every command that prints a result takes `-o table|json|ndjson|csv`, and
//! `--json` for `-o json`. Standard output carries the result and nothing
//! else. Hints, notes and what to run next go to standard error, and `-q`
//! leaves them out.
//!
//! - `table` is for people. Its columns may change between releases.
//! - `json` is the server's answer, or for a local command its report, as
//!   one document.
//! - `ndjson` is a list one item per line, and anything else its JSON on one
//!   line.
//! - `csv` is for lists: the table's columns, with the IDs. A command that
//!   prints a report refuses it before it does anything.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Args, ValueEnum};
use comfy_table::presets::NOTHING;
use comfy_table::{ContentArrangement, Table as Layout};
use serde::Serialize;
use serde_json::Value;

use crate::Failure;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Columns for people
    Table,
    /// One JSON document
    Json,
    /// One JSON value per line: each item of a list
    Ndjson,
    /// Comma-separated values, for lists
    Csv,
}

/// The options that choose the format.
#[derive(Args, Debug)]
#[command(next_help_heading = "Output options")]
pub struct OutputArgs {
    /// Output format
    #[arg(short, long, value_enum, default_value_t = Format::Table, global = true)]
    output: Format,
    /// Shorthand for `--output json`
    #[arg(long, global = true, conflicts_with = "output")]
    json: bool,
}

impl OutputArgs {
    fn format(&self) -> Format {
        match self.json {
            true => Format::Json,
            false => self.output,
        }
    }
}

/// Whether `-q` was given: nothing but the result and errors is printed.
static QUIET: AtomicBool = AtomicBool::new(false);

pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

pub fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// Says something beside the result, on standard error, unless `-q` was
/// given: a hint, a note, what to run next.
pub fn hint(text: impl AsRef<str>) {
    if !quiet() {
        eprintln!("{}", text.as_ref());
    }
}

/// Where a command prints its result, in the format that was asked for.
#[derive(Clone, Copy, Debug)]
pub struct Output {
    pub format: Format,
}

impl Output {
    /// For a command that prints a list, in any format.
    pub fn list(options: &OutputArgs) -> Output {
        Output {
            format: options.format(),
        }
    }

    /// For a command that prints a report, which is not a list: CSV is
    /// refused before anything is done.
    pub fn report(options: &OutputArgs, what: &str) -> Result<Output, Failure> {
        let output = Output::list(options);
        output.refuse_csv(what)?;
        Ok(output)
    }

    /// Refuses CSV for what is not a list, such as one thing of `spool show`.
    pub fn refuse_csv(&self, what: &str) -> Result<(), Failure> {
        match self.format {
            Format::Csv => Err(Failure::Usage(format!(
                "{what} is a report, not a list, so it cannot be CSV: use `-o json` or `-o ndjson`"
            ))),
            _ => Ok(()),
        }
    }

    /// Whether the result is for people.
    pub fn is_table(&self) -> bool {
        self.format == Format::Table
    }

    /// Whether the result is JSON, in one document or by lines.
    pub fn is_json(&self) -> bool {
        matches!(self.format, Format::Json | Format::Ndjson)
    }

    /// Prints a report in a machine format: one document, or one line.
    pub fn json(&self, value: &impl Serialize) {
        match self.format {
            Format::Ndjson => println!(
                "{}",
                serde_json::to_string(value).expect("a report serialises to JSON")
            ),
            _ => println!(
                "{}",
                serde_json::to_string_pretty(value).expect("a report serialises to JSON")
            ),
        }
    }

    /// Prints a list: the answer as it is, its items one per line, its table
    /// as CSV, or the table.
    pub fn items(&self, answer: &Value, items: &str, table: &Table) {
        match self.format {
            Format::Table => table.print(),
            Format::Json => self.json(answer),
            Format::Ndjson => match answer[items].as_array() {
                Some(items) => {
                    for item in items {
                        self.json(item);
                    }
                }
                None => self.json(answer),
            },
            Format::Csv => table.print_csv(),
        }
    }
}

/// A cell of a table: what people are shown, and for CSV the whole value
/// where that is not it.
#[derive(Clone, Debug)]
pub struct Cell {
    shown: String,
    whole: Option<String>,
}

impl Cell {
    /// A cell that shows less than the value, as a name cut short does.
    pub fn shortened(shown: String, whole: impl Into<String>) -> Cell {
        Cell {
            shown,
            whole: Some(whole.into()),
        }
    }
}

impl From<String> for Cell {
    fn from(shown: String) -> Self {
        Cell { shown, whole: None }
    }
}

impl From<&str> for Cell {
    fn from(shown: &str) -> Self {
        Cell::from(shown.to_owned())
    }
}

#[derive(Clone, Debug)]
struct Column {
    name: String,
    /// In CSV only: a column people do not need, such as an ID.
    hidden: bool,
}

/// Rows under column names.
#[derive(Clone, Debug)]
pub struct Table {
    /// What the rows are, for the line that says there are none.
    what: String,
    columns: Vec<Column>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn new(what: &str) -> Table {
        Table {
            what: what.to_owned(),
            columns: Vec::new(),
            rows: Vec::new(),
        }
    }

    pub fn column(mut self, name: &str) -> Table {
        self.columns.push(Column {
            name: name.to_owned(),
            hidden: false,
        });
        self
    }

    /// A column that only CSV has, or the table too when `shown`.
    pub fn column_if(mut self, name: &str, shown: bool) -> Table {
        self.columns.push(Column {
            name: name.to_owned(),
            hidden: !shown,
        });
        self
    }

    pub fn columns<'a>(mut self, names: impl IntoIterator<Item = &'a str>) -> Table {
        for name in names {
            self = self.column(name);
        }
        self
    }

    /// A row, a cell for each column.
    pub fn row(&mut self, cells: Vec<Cell>) {
        debug_assert_eq!(cells.len(), self.columns.len(), "a cell for each column");
        self.rows.push(cells);
    }

    /// The rows under their names, or a line that says there are none.
    pub fn print(&self) {
        if self.rows.is_empty() {
            println!("no {}", self.what);
            return;
        }
        let shown: Vec<usize> = (0..self.columns.len())
            .filter(|&column| !self.columns[column].hidden)
            .collect();
        let header: Vec<String> = shown
            .iter()
            .map(|&column| self.columns[column].name.to_uppercase())
            .collect();
        let rows: Vec<Vec<String>> = self
            .rows
            .iter()
            .map(|row| {
                shown
                    .iter()
                    .map(|&column| one_line(&row[column].shown))
                    .collect()
            })
            .collect();
        for line in render(Some(header), &rows, 0) {
            println!("{line}");
        }
    }

    /// The rows as CSV: a header of the columns' names, then each row with
    /// its whole values.
    pub fn print_csv(&self) {
        let mut writer = csv::Writer::from_writer(std::io::stdout().lock());
        let header = self
            .columns
            .iter()
            .map(|column| column.name.replace(' ', "_"));
        let written = writer.write_record(header).and_then(|()| {
            for row in &self.rows {
                writer.write_record(
                    row.iter()
                        .map(|cell| cell.whole.as_deref().unwrap_or(&cell.shown)),
                )?;
            }
            writer.flush().map_err(csv::Error::from)
        });
        // A reader that has gone ends the command, as SIGPIPE does.
        if let Err(error) = written {
            eprintln!("spool: {error}");
        }
    }
}

/// A value on one line: a line break or a tab in it would break the table.
fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect()
}

/// Lines of cells in columns, two spaces apart, each line indented by
/// `indent` spaces. On a terminal, a table wider than it is wrapped to fit.
/// A row may have fewer cells than others.
pub fn render(header: Option<Vec<String>>, rows: &[Vec<String>], indent: usize) -> Vec<String> {
    let mut layout = Layout::new();
    layout.load_style(NOTHING);
    if std::io::stdout().is_terminal() {
        layout.set_content_arrangement(ContentArrangement::Dynamic);
        if let Some(width) = layout.width() {
            layout.set_width(width.saturating_sub(indent as u16));
        }
    } else {
        layout.set_content_arrangement(ContentArrangement::Disabled);
    }
    if let Some(header) = header {
        layout.set_header(header);
    }
    for row in rows {
        layout.add_row(row.clone());
    }
    let columns = layout.column_count();
    for (at, column) in layout.column_iter_mut().enumerate() {
        column.set_padding((0, if at + 1 < columns { 2 } else { 0 }));
    }
    let indent = " ".repeat(indent);
    layout
        .lines()
        .map(|line| format!("{indent}{}", line.trim_end()))
        .collect()
}

/// Label and value, one pair per line, the values in one column.
pub fn fields(rows: &[(&str, String)]) {
    pairs(rows, 0);
}

/// As [`fields`], indented.
pub fn indented(rows: &[(&str, String)]) {
    pairs(rows, 2);
}

fn pairs(rows: &[(&str, String)], indent: usize) {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|(label, value)| vec![(*label).to_owned(), value.clone()])
        .collect();
    aligned(&rows, indent);
}

/// Prints rows with their cells in columns. The last cell is as long as it
/// is.
pub fn aligned(rows: &[Vec<String>], indent: usize) {
    for line in render(None, rows, indent) {
        println!("{line}");
    }
}

/// Prints rows under their column names, within a report for people.
pub fn table(what: &str, headers: &[&str], rows: Vec<Vec<String>>) {
    let mut table = Table::new(what).columns(headers.iter().copied());
    for row in rows {
        table.row(row.into_iter().map(Cell::from).collect());
    }
    table.print();
}
