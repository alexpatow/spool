//! `spool show identities`: the list of identities, and one identity with
//! its members, their constraints and its history.

use std::collections::HashMap;

use anstream::println;
use serde_json::json;
use spool::core::{ConstraintEntry, DocumentId, IdentityId, RecordId};
use spool::{HistoryEntry, IdentitySummary, IdentityView, Member, RecordEntry};

use super::{
    IdentitiesArgs, Query, View, describe_op, format_enum_word, format_log_position,
    format_source_key, format_timestamp, has_mixed_types, label_cell,
};
use crate::Failure;
use crate::commands::remote::typed;
use crate::output::{Cell, Table, fields, table};

impl View<'_> {
    pub(super) fn identities(&self, what: &IdentitiesArgs) -> Result<(), Failure> {
        let query = Query::default()
            .add("entity_type", what.entity_type.as_ref())
            .add("sort", what.sort.as_ref())
            .add("order", what.order.as_ref())
            .add("after", what.after.as_ref())
            .add("limit", what.limit)
            .0;
        let answer = self.remote.list("/identities", &query)?;
        let identities: Vec<IdentitySummary> = typed(&answer["identities"])?;
        // The type is said once, by the option that chose it or by there
        // being one, and per row only when the rows differ.
        let typed_rows = has_mixed_types(identities.iter().map(|identity| &identity.entity_type));
        let mut table = Table::new("identities")
            .column("identity")
            .column_if("type", typed_rows)
            .columns(["name", "members", "names", "unlinked pairs", "created at"]);
        for identity in &identities {
            table.row(vec![
                identity.id.to_string().into(),
                identity.entity_type.to_string().into(),
                label_cell(identity.label.as_deref()),
                identity.members.to_string().into(),
                identity.names.to_string().into(),
                Cell::shortened(
                    format_unlinked_share(identity.unlinked_pairs, identity.linked_pairs),
                    identity.unlinked_pairs.to_string(),
                ),
                format_log_position(identity.created),
            ]);
        }
        self.out.items(&answer, "identities", &table)?;
        self.more(&answer);
        Ok(())
    }

    pub(super) fn identity(&self, identity: IdentityId) -> Result<(), Failure> {
        let missing = || format!("there is no identity {identity}");
        let view = self
            .remote
            .get(&format!("/identities/{identity}"), &[], missing)?;
        let history = self
            .remote
            .get(&format!("/identities/{identity}/history"), &[], missing)?;
        // A merged identity answers with the members of the one that stands
        // for it, and the constraints are those on the members shown.
        let current = view["current"]
            .as_str()
            .map_or_else(|| identity.to_string(), str::to_owned);
        let constraints = self
            .remote
            .list("/constraints", &[("identity", current.clone())])?;
        if !self.out.is_table() {
            self.out.json(&json!({
                "identity": view,
                "constraints": constraints["constraints"],
                "history": history["history"],
            }));
            return Ok(());
        }
        let view: IdentityView = typed(&view)?;
        let history: Vec<HistoryEntry> = typed(&history["history"])?;
        let constraints: Vec<ConstraintEntry> = typed(&constraints["constraints"])?;

        let mut rows = vec![("identity", view.id.to_string())];
        if view.current != view.id {
            rows.push(("merged into", view.current.to_string()));
        }
        rows.push(("type", view.entity_type.to_string()));
        rows.push(("created at", format!("log position {}", view.created)));
        fields(&rows);

        let mentions = self.mentions_of(&current, &view.members)?;
        println!();
        table(
            "members",
            &["record", "source and key", "since"],
            view.members
                .iter()
                .map(|member| {
                    vec![
                        member.record.to_string(),
                        match mentions.get(&member.record) {
                            Some(mention) => mention.clone(),
                            None => format_source_key(&member.source, member.key.as_deref()),
                        },
                        format!("log position {}", member.since),
                    ]
                })
                .collect(),
        );
        if !constraints.is_empty() {
            println!();
            table(
                "constraints",
                &["constraint", "between", "and", "since"],
                constraints
                    .iter()
                    .map(|record| {
                        vec![
                            format_enum_word(&record.constraint.kind),
                            record.constraint.a.to_string(),
                            record.constraint.b.to_string(),
                            format!("log position {}", record.from),
                        ]
                    })
                    .collect(),
            );
        }
        println!("\nhistory");
        for entry in &history {
            let mut line = format!(
                "  {}  {}  {}",
                entry.seq,
                format_timestamp(&entry.committed_at),
                entry.actor
            );
            if let Some(run) = entry.run {
                line.push_str(&format!("  reconcile run {run}"));
            }
            if let Some(reason) = &entry.reason {
                line.push_str(&format!("  \"{reason}\""));
            }
            println!("{line}");
            for op in &entry.ops {
                println!("      {}", describe_op(op));
            }
        }
        Ok(())
    }

    /// What to call the members of an identity that are mentions: their
    /// document and their place in it, as a list of records calls them. A
    /// member says its key, and the key of a mention names its document by
    /// ID, so the records are asked for when a member is one.
    fn mentions_of(
        &self,
        identity: &str,
        members: &[Member],
    ) -> Result<HashMap<RecordId, String>, Failure> {
        let is_mention = |member: &Member| {
            member
                .key
                .as_deref()
                .and_then(|key| key.split_once('#'))
                .is_some_and(|(document, _)| document.parse::<DocumentId>().is_ok())
        };
        let mut names = HashMap::new();
        if !members.iter().any(is_mention) {
            return Ok(names);
        }
        let mut after: Option<String> = None;
        loop {
            let mut query = vec![("identity", identity.to_owned())];
            if let Some(after) = after {
                query.push(("after", after));
            }
            let page = self.remote.list("/records", &query)?;
            let records: Vec<RecordEntry> = typed(&page["records"])?;
            for record in &records {
                let is_mention = record
                    .evidence
                    .as_ref()
                    .is_some_and(|evidence| evidence.get("document").is_some());
                if is_mention {
                    names.insert(record.id, record.name());
                }
            }
            match page["next"].as_str() {
                Some(next) => after = Some(next.to_owned()),
                None => return Ok(names),
            }
        }
    }
}

/// The part of an identity's pairs of members that no link joins, as a
/// percentage: none when it has no pairs, and never 0% or 100% unless it is.
fn format_unlinked_share(unlinked: u64, linked: u64) -> String {
    let pairs = unlinked + linked;
    if pairs == 0 {
        return "-".to_owned();
    }
    let percent = (unlinked as f64 * 100.0 / pairs as f64).round() as u64;
    match percent {
        0 if unlinked > 0 => "<1%".to_owned(),
        100 if linked > 0 => ">99%".to_owned(),
        percent => format!("{percent}%"),
    }
}
