use std::io::Read;

use serde_json::{Map, Value, json};
use spool::AddRecordReport;

use super::remote::{Remote, RemoteArgs, typed};
use crate::Failure;
use crate::output::{Output, OutputArgs, fields};

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
