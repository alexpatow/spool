//! `spool show`: what a running server holds.
//!
//! It reads through the server's API and prints what it answered. With
//! `--json` it prints the answer itself. Each thing it shows is a file of
//! `show/`, named as the engine names it.

use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::Value;
use spool::RecordEntry;
use spool::core::{ChangesetId, DocumentId, IdentityId, Op};

use super::remote::{Remote, RemoteArgs};
use crate::Failure;
use crate::output::{Cell, Output, OutputArgs, hint};

mod attributes;
mod documents;
mod drafts;
mod identities;
mod log;
mod overview;
pub(super) mod records;
mod review;
pub(super) mod runs;

/// Show what a running server holds
///
/// Shows what a running server holds. Without a subcommand: its entity types
/// and how much there is of each.
///
/// A record is named by its ID or as <source>:<key>, which names the latest
/// version of that row. A record found in a document is named as the lists
/// name it, <source>:<document>[<start>-<end>].
#[derive(Args)]
#[command(
    after_help = "Examples:\n  spool show\n  spool show identities --sort records --order desc\n  spool show records crm:c-1"
)]
pub struct ShowArgs {
    #[command(subcommand)]
    pub what: Option<What>,
    #[command(flatten)]
    output: OutputArgs,
    #[command(flatten)]
    remote: RemoteArgs,
}

#[derive(Subcommand)]
pub enum What {
    Runs(RunsArgs),
    Identities(IdentitiesArgs),
    Records(RecordsArgs),
    Attributes(AttributesArgs),
    Documents(DocumentsArgs),
    Review(ReviewArgs),
    Drafts(DraftsArgs),
    Log(LogArgs),
}

impl What {
    pub fn name(&self) -> &'static str {
        match self {
            What::Runs(_) => "runs",
            What::Identities(_) => "identities",
            What::Records(_) => "records",
            What::Attributes(_) => "attributes",
            What::Documents(_) => "documents",
            What::Review(_) => "review",
            What::Drafts(_) => "drafts",
            What::Log(_) => "log",
        }
    }
}

/// List the latest runs, or show one and what it reported
#[derive(Args)]
pub struct RunsArgs {
    /// A run, to show what it reported
    run: Option<u64>,
    /// Only this entity type
    #[arg(long = "type", value_name = "NAME", conflicts_with = "run")]
    entity_type: Option<String>,
    /// Only this status
    #[arg(
        long,
        value_parser = ["running", "open", "drafted", "succeeded", "failed"],
        conflicts_with = "run"
    )]
    status: Option<String>,
    /// Only runs of this kind
    #[arg(
        long,
        value_parser = ["load", "extract", "match", "reconcile", "retract"],
        conflicts_with = "run"
    )]
    kind: Option<String>,
    /// List at most this many
    #[arg(long, conflicts_with = "run")]
    limit: Option<u64>,
}

/// List identities, or show one with its members and history
///
/// The list shows each identity's record count, its number of distinct names
/// (labels, ignoring case), and the share of its record pairs that no link
/// joins directly, which are together only through a chain. Many names and
/// mostly unlinked pairs usually means different things were joined. `spool
/// explain <identity>` shows which rule did it.
///
/// A single identity is shown with its members, their constraints and its
/// history.
#[derive(Args)]
#[command(
    after_help = "Examples:\n  spool show identities --sort records --order desc\n  spool show identities --type company --limit 10"
)]
pub struct IdentitiesArgs {
    /// An identity's ID, to show it
    identity: Option<IdentityId>,
    /// Only this entity type
    #[arg(long = "type", value_name = "NAME", conflicts_with = "identity")]
    entity_type: Option<String>,
    /// Sort by this column
    ///
    /// `records` is how many records each has, and `names` how many different
    /// labels.
    #[arg(
        long,
        value_parser = ["records", "names", "sources", "label", "entity_type", "created", "id"],
        conflicts_with = "identity"
    )]
    sort: Option<String>,
    /// The order of the sort [default: asc]
    #[arg(long, value_parser = ["asc", "desc"], conflicts_with = "identity")]
    order: Option<String>,
    /// Continue the list after this ID, in its order
    #[arg(long, value_name = "ID", conflicts_with = "identity")]
    after: Option<String>,
    /// List at most this many
    #[arg(long, conflicts_with = "identity")]
    limit: Option<u64>,
}

/// List records by source and key, or show one with its payload and evidence
///
/// A list shows the version of each record that nothing replaced;
/// `--all-versions` shows the versions that later loads replaced as well.
///
/// One record is shown with its payload, its evidence and, for a mention, the
/// place in the document it was found in. It is named by its ID or as
/// <source>:<key>, which names the latest version of that row. A record found
/// in a document is named as the lists name it,
/// <source>:<document>[<start>-<end>].
#[derive(Args)]
#[command(
    after_help = "Examples:\n  spool show records --source crm --attributes\n  spool show records --unassigned\n  spool show records crm:c-1"
)]
pub struct RecordsArgs {
    /// A record, by its ID or as <source>:<key>, to show it
    record: Option<String>,
    /// Only this entity type
    #[arg(long = "type", value_name = "NAME", conflicts_with = "record")]
    entity_type: Option<String>,
    /// Only this source
    #[arg(long, value_name = "NAME", conflicts_with = "record")]
    source: Option<String>,
    /// Only this key within its source
    #[arg(long, conflicts_with = "record")]
    key: Option<String>,
    /// Only the mentions found in this document
    #[arg(long, value_name = "ID", conflicts_with = "record")]
    document: Option<String>,
    /// Only records that wait for an identity
    ///
    /// They have none, and their source has not taken them back.
    #[arg(long, conflicts_with = "record")]
    unassigned: bool,
    /// Every version, the replaced ones too
    #[arg(long, conflicts_with = "record")]
    all_versions: bool,
    /// A column for each attribute of the records
    #[arg(long, conflicts_with = "record")]
    attributes: bool,
    /// List each record's ID as well
    #[arg(long, conflicts_with = "record")]
    ids: bool,
    /// Continue the list after this ID, in its order
    #[arg(long, value_name = "ID", conflicts_with = "record")]
    after: Option<String>,
    /// List at most this many
    #[arg(long, conflicts_with = "record")]
    limit: Option<u64>,
}

/// For each entity type and source, the attributes its records have
///
/// For each entity type and source, the attributes its records have: how many
/// have each, how many values it takes, the most common, and the features
/// made from it. Attributes are counted over the records that stand: the
/// latest version of each, and not one its source took back.
#[derive(Args)]
pub struct AttributesArgs {
    /// Only this entity type
    #[arg(long = "type", value_name = "NAME")]
    entity_type: Option<String>,
    /// Only this source
    #[arg(long, value_name = "NAME")]
    source: Option<String>,
}

/// List documents, or show one with its text
///
/// A list shows the version of each document that nothing replaced;
/// `--all-versions` shows the versions that later loads replaced as well.
#[derive(Args)]
pub struct DocumentsArgs {
    /// A document's ID, to show it with its text
    document: Option<DocumentId>,
    /// Only this source
    #[arg(long, value_name = "NAME", conflicts_with = "document")]
    source: Option<String>,
    /// Only this key within its source
    #[arg(long, conflicts_with = "document")]
    key: Option<String>,
    /// Every version, the replaced ones too
    #[arg(long, conflicts_with = "document")]
    all_versions: bool,
    /// Continue the list after this ID, in its order
    #[arg(long, value_name = "ID", conflicts_with = "document")]
    after: Option<String>,
    /// List at most this many
    #[arg(long, conflicts_with = "document")]
    limit: Option<u64>,
}

/// List review items, or show one with its records
#[derive(Args)]
pub struct ReviewArgs {
    /// A review item's number, to show it with its records
    item: Option<u64>,
    /// Only this entity type
    #[arg(long = "type", value_name = "NAME", conflicts_with = "item")]
    entity_type: Option<String>,
    /// Only this status
    #[arg(
        long,
        value_parser = ["open", "resolved", "superseded", "dismissed"],
        conflicts_with = "item"
    )]
    status: Option<String>,
    /// Continue the list after this item, in its order
    #[arg(long, value_name = "ITEM", conflicts_with = "item")]
    after: Option<String>,
    /// List at most this many
    #[arg(long, conflicts_with = "item")]
    limit: Option<u64>,
}

/// List drafts, or show one with the changeset it would commit
#[derive(Args)]
pub struct DraftsArgs {
    /// A draft's ID, to show the changeset it would commit
    draft: Option<ChangesetId>,
    /// Only this entity type
    #[arg(long = "type", value_name = "NAME", conflicts_with = "draft")]
    entity_type: Option<String>,
    /// Only this status
    #[arg(
        long,
        value_parser = ["open", "committed", "rejected", "superseded"],
        conflicts_with = "draft"
    )]
    status: Option<String>,
}

/// List the changeset log
#[derive(Args)]
pub struct LogArgs {
    /// Continue the log after this position
    #[arg(long, value_name = "POSITION")]
    after: Option<String>,
    /// List at most this many
    #[arg(long)]
    limit: Option<u64>,
}

pub fn show(arguments: ShowArgs) -> Result<(), Failure> {
    let remote = Remote::new(&arguments.remote);
    let out = Output::list(&arguments.output);
    // One thing is a report, and only a list is CSV.
    let one = match &arguments.what {
        None => Some("`spool show`"),
        Some(What::Runs(what)) => what.run.map(|_| "`spool show runs <RUN>`"),
        Some(What::Identities(what)) => what.identity.map(|_| "`spool show identities <IDENTITY>`"),
        Some(What::Records(what)) => what
            .record
            .as_ref()
            .map(|_| "`spool show records <RECORD>`"),
        Some(What::Documents(what)) => what.document.map(|_| "`spool show documents <DOCUMENT>`"),
        Some(What::Review(what)) => what.item.map(|_| "`spool show review <ITEM>`"),
        Some(What::Drafts(what)) => what.draft.map(|_| "`spool show drafts <DRAFT>`"),
        Some(What::Attributes(_) | What::Log(_)) => None,
    };
    if let Some(one) = one {
        out.refuse_csv(one)?;
    }
    let view = View {
        remote: &remote,
        out,
    };
    match &arguments.what {
        None => view.overview(),
        Some(What::Runs(what)) => match what.run {
            Some(run) => view.run(run),
            None => view.runs(what),
        },
        Some(What::Identities(what)) => match what.identity {
            Some(identity) => view.identity(identity),
            None => view.identities(what),
        },
        Some(What::Records(what)) => match &what.record {
            Some(record) => view.record(record),
            None => view.records(what),
        },
        Some(What::Attributes(what)) => view.attributes(what),
        Some(What::Documents(what)) => match what.document {
            Some(document) => view.document(document),
            None => view.documents(what),
        },
        Some(What::Review(what)) => match what.item {
            Some(item) => view.review_item(item),
            None => view.review_items(what),
        },
        Some(What::Drafts(what)) => match what.draft {
            Some(draft) => view.draft(draft),
            None => view.drafts(what),
        },
        Some(What::Log(what)) => view.log(what),
    }
}

struct View<'a> {
    remote: &'a Remote,
    out: Output,
}

/// The query of a list: a parameter for each option that was given.
#[derive(Default)]
struct Query(Vec<(&'static str, String)>);

impl Query {
    fn add(mut self, parameter: &'static str, value: Option<impl ToString>) -> Self {
        if let Some(value) = value {
            self.0.push((parameter, value.to_string()));
        }
        self
    }

    fn flag(self, parameter: &'static str, given: bool) -> Self {
        self.add(parameter, given.then_some("true"))
    }
}

impl View<'_> {
    /// Says how to go on when a list has more than was shown.
    fn more(&self, answer: &Value) {
        match &answer["next"] {
            Value::String(next) => self.hint_more(next),
            Value::Number(next) => self.hint_more(&next.to_string()),
            _ => {}
        }
    }

    fn hint_more(&self, after: &str) {
        if self.out.is_table() {
            hint(format!("more: add `--after {after}`"));
        }
    }

    /// Asks a list for the versions nothing replaced, unless it is to show
    /// every version.
    fn latest(query: &mut Query, all_versions: bool) {
        if !all_versions {
            query.0.push(("latest", "true".to_owned()));
        }
    }
}

/// A changeset operation in words.
fn describe(op: &Op) -> String {
    match op {
        Op::CreateIdentity {
            identity,
            entity_type,
        } => format!("create {entity_type} identity {identity}"),
        Op::Assign {
            record,
            from: None,
            to: Some(to),
        } => format!("place record {record} in {to}"),
        Op::Assign {
            record,
            from: Some(from),
            to: Some(to),
        } => format!("move record {record} from {from} to {to}"),
        Op::Assign {
            record,
            from: Some(from),
            to: None,
        } => format!("take record {record} out of {from}"),
        Op::Assign {
            record,
            from: None,
            to: None,
        } => format!("leave record {record} without an identity"),
        Op::Merge { from, into } => format!("merge {from} into {into}"),
        Op::Constrain { kind, a, b } => format!("constrain {a} and {b}: {}", word(kind)),
        Op::Unconstrain { kind, a, b } => {
            format!("remove the {} constraint on {a} and {b}", word(kind))
        }
        Op::Reinstate { identity } => format!("reinstate identity {identity}"),
    }
}

/// How an enum is written in the API, which is how the documentation names
/// it.
pub(super) fn word<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(word)) => word,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

/// A JSON value on one line, text without its quotes.
pub(super) fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "-".to_owned(),
        other => other.to_string(),
    }
}

/// A timestamp to the second.
fn when(timestamp: &str) -> String {
    match timestamp.split_once('.') {
        Some((seconds, _)) => format!("{seconds}Z"),
        None => timestamp.to_owned(),
    }
}

/// The most characters of a name a list shows. With a name of this
/// length and a key of ordinary length, a line of the list of records is
/// under a hundred characters.
const LABEL_CHARACTERS: usize = 40;

/// What a record or an identity is called, in a list: its label, cut to a
/// length that keeps the columns after it in sight.
pub(super) fn label(label: Option<&str>) -> String {
    let Some(label) = label else {
        return "-".to_owned();
    };
    // A line break in a value would break the list.
    let line: String = label
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect();
    match line.chars().count() > LABEL_CHARACTERS {
        true => {
            let kept: String = line.chars().take(LABEL_CHARACTERS - 1).collect();
            format!("{}…", kept.trim_end())
        }
        false => line,
    }
}

/// A label as a cell: cut short in a table, and whole in CSV.
fn label_cell(whole: Option<&str>) -> Cell {
    Cell::shortened(label(whole), whole.unwrap_or_default())
}

/// A position in the log as a cell: said as one in a table, and the
/// number in CSV.
fn position(seq: impl std::fmt::Display) -> Cell {
    Cell::shortened(format!("log position {seq}"), seq.to_string())
}

/// Whether the rows of a list are of more than one entity type.
fn mixed<'a, T: PartialEq + 'a>(mut types: impl Iterator<Item = &'a T>) -> bool {
    match types.next() {
        Some(first) => types.any(|other| other != first),
        None => false,
    }
}

fn named(source: &str, key: Option<&str>) -> String {
    match key {
        Some(key) => format!("{source}:{key}"),
        None => format!("{source} (no key)"),
    }
}

/// What a list calls a record: its source and key, and a mention its
/// document and its place there, as an explanation does. The key of a
/// mention is the ID of its document and more, which nobody reads.
fn record_name(record: &RecordEntry) -> String {
    match &record.key {
        Some(_) => record.name(),
        None => named(&record.source, None),
    }
}

/// What a list that shows every version says of each.
fn version(replaced: bool) -> String {
    match replaced {
        true => "replaced".to_owned(),
        false => "current".to_owned(),
    }
}
