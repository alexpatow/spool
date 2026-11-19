use std::io::Read;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use spool::{AddDocumentReport, AddRecordReport, NewDocument};

use super::remote::{Remote, RemoteArgs, typed};
use crate::Failure;
use crate::output::{Output, OutputArgs, fields, table};

/// Add one row and get its identity
///
/// Sends one row to a running server. The project's rules match its record
/// against existing identities. A record that fits several identities is
/// left for review, and the answer gives the review item.
#[derive(clap::Args)]
#[command(
    after_help = "Example:\n  spool add --source crm --type company '{\"id\":\"c-1\",\"name\":\"Acorn Tools\"}' --json"
)]
pub struct AddArgs {
    /// A JSON object of column values, or `-` to read it from standard input
    row: String,
    /// The source that supplies the row
    #[arg(long, value_name = "NAME")]
    source: String,
    /// The entity type the row describes
    #[arg(long = "type", value_name = "NAME")]
    entity_type: String,
    /// Allow a new version under an existing key
    #[arg(long)]
    replace: bool,
    #[command(flatten)]
    output: OutputArgs,
    #[command(flatten)]
    remote: RemoteArgs,
}

pub fn add(arguments: AddArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool add`")?;
    let mut row = arguments.row;
    if row == "-" {
        row.clear();
        std::io::stdin()
            .read_to_string(&mut row)
            .map_err(|error| Failure::failed(format!("standard input: {error}")))?;
    }
    let row: Map<String, Value> = serde_json::from_str(&row)
        .map_err(|error| Failure::usage(format!("the row must be a JSON object: {error}")))?;
    let answer = Remote::new(&arguments.remote).post(
        "/records",
        &json!({
            "source": arguments.source,
            "entity_type": arguments.entity_type,
            "row": row,
            "replace": arguments.replace,
        }),
    )?;
    if !out.is_table() {
        out.json(&answer);
        return Ok(());
    }
    let report: AddRecordReport = typed(&answer)?;
    let placement = match (report.identity, report.review) {
        (Some(identity), _) => ("identity", identity.to_string()),
        (_, Some(item)) => (
            "review",
            format!(
                "{} ({}, {})",
                item.id,
                item.kind.as_str(),
                item.status.as_str()
            ),
        ),
        (None, None) => ("identity", "unassigned".to_owned()),
    };
    fields(&[("record", report.record.to_string()), placement]);
    Ok(())
}

/// Add one document and get its records' identities
///
/// Stores the text on a running server, extracts its mentions, and matches
/// their records. A record that needs review comes back with its review item.
#[derive(clap::Args)]
#[command(after_help = "Example:\n  spool add-document --source notes notes-1.txt --json")]
pub struct AddDocumentArgs {
    /// A text file, or `-` to read text from standard input
    path: PathBuf,
    /// The declared source that supplies the document
    #[arg(long, value_name = "NAME")]
    source: String,
    /// The key within the source. Defaults to the file name and is required with `-`
    #[arg(long)]
    key: Option<String>,
    /// The extractor to run. Defaults to the project's only extractor
    #[arg(long, value_name = "NAME")]
    extractor: Option<String>,
    /// Where the source keeps the document
    #[arg(long)]
    uri: Option<String>,
    #[command(flatten)]
    output: OutputArgs,
    #[command(flatten)]
    remote: RemoteArgs,
}

pub fn add_document(arguments: AddDocumentArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool add-document`")?;
    let stdin = arguments.path.as_os_str() == "-";
    let key = arguments
        .key
        .or_else(|| {
            if stdin {
                return None;
            }
            arguments.path.file_name()?.to_str().map(str::to_owned)
        })
        .ok_or_else(|| Failure::usage("give the document a key with `--key`"))?;
    let text = if stdin {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| Failure::failed(format!("standard input: {error}")))?;
        text
    } else {
        std::fs::read_to_string(&arguments.path)
            .map_err(|error| Failure::failed(format!("{}: {error}", arguments.path.display())))?
    };
    // The server gives the extractor its own deadline.
    let answer = Remote::with_timeout(&arguments.remote, None).post(
        "/documents/add",
        &json!({
            "source": arguments.source,
            "document": NewDocument { key, text, uri: arguments.uri },
            "extractor": arguments.extractor,
        }),
    )?;
    if !out.is_table() {
        out.json(&answer);
        return Ok(());
    }
    let report: AddDocumentReport = typed(&answer)?;
    fields(&[("document", report.document.to_string())]);
    let rows = report
        .records
        .into_iter()
        .map(|mention| {
            vec![
                format!("{}-{}", mention.start, mention.end),
                mention.entity_type.to_string(),
                mention.record.to_string(),
                mention
                    .identity
                    .map_or_else(|| "-".to_owned(), |id| id.to_string()),
                mention
                    .review
                    .map_or_else(|| "-".to_owned(), |item| item.id.to_string()),
            ]
        })
        .collect();
    table(
        "records",
        &["SPAN", "TYPE", "RECORD", "IDENTITY", "REVIEW"],
        rows,
    );
    Ok(())
}
