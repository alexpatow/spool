//! `spool show drafts`: the list of drafts, and one draft with the changeset
//! it would commit.

use anstream::println;
use spool::core::ChangesetId;
use spool::{DraftEntry, DraftSummary};
use spool_cli::text::count;

use super::{DraftsArgs, Query, View, describe_op, format_timestamp};
use crate::Failure;
use crate::commands::remote::typed;
use crate::output::{Cell, Table, fields};

impl View<'_> {
    pub(super) fn drafts(&self, what: &DraftsArgs) -> Result<(), Failure> {
        let query = Query::default()
            .add("entity_type", what.entity_type.as_ref())
            .add("status", what.status.as_ref())
            .0;
        let answer = self.remote.list("/drafts", &query)?;
        let drafts: Vec<DraftSummary> = typed(&answer["drafts"])?;
        let mut table =
            Table::new("drafts").columns(["draft", "type", "status", "operations", "from"]);
        for draft in &drafts {
            table.row(vec![
                draft.id.to_string().into(),
                draft.entity_type.to_string().into(),
                draft.status.to_string().into(),
                draft.operations.to_string().into(),
                Cell::shortened(
                    format!("match run {}", draft.match_run),
                    draft.match_run.to_string(),
                ),
            ]);
        }
        self.out.items(&answer, "drafts", &table)?;
        Ok(())
    }

    pub(super) fn draft(&self, draft: ChangesetId) -> Result<(), Failure> {
        let answer = self.remote.get(&format!("/drafts/{draft}"), &[], || {
            format!("there is no draft {draft}")
        })?;
        if !self.out.is_table() {
            self.out.json(&answer);
            return Ok(());
        }
        let entry: DraftEntry = typed(&answer)?;
        let draft = &entry.draft;
        let mut rows = vec![
            ("draft", draft.id.to_string()),
            ("type", draft.entity_type.to_string()),
            ("status", draft.status.to_string()),
            (
                "from",
                format!(
                    "match run {}, reconcile run {}",
                    draft.match_run, draft.reconcile_run
                ),
            ),
            ("written against", format!("log position {}", draft.base)),
            (
                "would place",
                format!(
                    "{}, {} created",
                    count(draft.records_assigned, "record"),
                    count(draft.identities_created, "identity")
                ),
            ),
        ];
        if let Some(seq) = draft.committed_seq {
            rows.push(("committed at", format!("log position {seq}")));
        }
        if let Some(run) = draft.superseded_by {
            rows.push(("superseded by", format!("reconcile run {run}")));
        }
        if let Some(at) = &draft.rejected_at {
            let mut rejected = format_timestamp(at);
            if let Some(reason) = &draft.rejected_reason {
                rejected.push_str(&format!(", \"{reason}\""));
            }
            rows.push(("rejected", rejected));
        }
        fields(&rows);
        println!("\noperations");
        for op in &entry.changeset.ops {
            println!("  {}", describe_op(op));
        }
        Ok(())
    }
}
