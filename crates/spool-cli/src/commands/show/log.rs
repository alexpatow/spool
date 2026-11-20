//! `spool show log`: the changeset log.

use spool::LoggedChangeset;

use super::{LogArgs, Query, View};
use crate::Failure;
use crate::commands::remote::typed;
use crate::output::Table;

impl View<'_> {
    pub(super) fn log(&self, what: &LogArgs) -> Result<(), Failure> {
        let query = Query::default()
            .add("after", what.after.as_ref())
            .add("limit", what.limit)
            .0;
        let answer = self.remote.list("/changesets", &query)?;
        let entries: Vec<LoggedChangeset> = typed(&answer["changesets"])?;
        // The actor decided the operations. Who asked for the run that
        // committed them is beside it, where a run did.
        let mut table = Table::new("changesets").columns([
            "position",
            "actor",
            "operations",
            "from",
            "started by",
            "reason",
        ]);
        for logged in &entries {
            let changeset = &logged.entry.changeset;
            table.row(vec![
                logged.entry.seq.to_string().into(),
                changeset.actor.to_string().into(),
                changeset.ops.len().to_string().into(),
                changeset
                    .run
                    .map_or_else(|| "-".to_owned(), |run| format!("reconcile run {run}"))
                    .into(),
                logged
                    .started_by
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), ToString::to_string)
                    .into(),
                changeset
                    .reason
                    .clone()
                    .unwrap_or_else(|| "-".to_owned())
                    .into(),
            ]);
        }
        self.out.items(&answer, "changesets", &table)?;
        if let (Some(last), Some(head)) = (entries.last(), answer["head"].as_u64())
            && last.entry.seq.0 < head
        {
            self.hint_more(&last.entry.seq.to_string());
        }
        Ok(())
    }
}
