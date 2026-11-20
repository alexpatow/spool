//! `spool show review`: the list of review items, and one item with its
//! records.

use anstream::println;
use serde_json::{Value, json};
use spool::{RecordEntry, ReviewEntry, ReviewKind, ReviewStatus};
use spool_cli::text::count;

use super::{Query, ReviewArgs, View, format_enum_word, format_timestamp, record_name};
use crate::Failure;
use crate::commands::remote::typed;
use crate::output::{Cell, Table, aligned, fields, indented, table};

impl View<'_> {
    pub(super) fn review_items(&self, what: &ReviewArgs) -> Result<(), Failure> {
        let query = Query::default()
            .add("entity_type", what.entity_type.as_ref())
            .add("status", what.status.as_ref())
            .add("after", what.after.as_ref())
            .add("limit", what.limit)
            .0;
        let answer = self.remote.list("/review", &query)?;
        let items: Vec<ReviewEntry> = typed(&answer["items"])?;
        let mut table = Table::new("review items").columns([
            "item",
            "kind",
            "status",
            "type",
            "raised by",
            "about",
        ]);
        for item in &items {
            table.row(vec![
                item.id.to_string().into(),
                format_enum_word(&item.kind).into(),
                format_enum_word(&item.status).into(),
                item.entity_type.to_string().into(),
                Cell::shortened(format!("run {}", item.run), item.run.to_string()),
                describe_review_item(item).into(),
            ]);
        }
        self.out.items(&answer, "items", &table)?;
        self.more(&answer);
        Ok(())
    }

    pub(super) fn review_item(&self, item: u64) -> Result<(), Failure> {
        let answer = self.remote.get(&format!("/review/{item}"), &[], || {
            format!("there is no review item {item}")
        })?;
        let item: ReviewEntry = typed(&answer)?;
        let mut records = Vec::new();
        for id in &item.records {
            if let Some(found) = self.remote.find(&format!("/records/{id}"), &[])? {
                records.push(found);
            }
        }
        let split = item.kind == ReviewKind::PossibleSplit;
        // The records whose retraction left the question are part of the
        // case, not of the item.
        let taken_back = match split {
            true => self
                .remote
                .get(&format!("/review/{}/case", item.id), &[], || {
                    format!("there is no review item {}", item.id)
                })?["taken_back"]
                .clone(),
            false => Value::Null,
        };
        if !self.out.is_table() {
            let mut bundle = json!({ "item": answer, "records": records });
            if split {
                bundle["taken_back"] = taken_back;
            }
            self.out.json(&bundle);
            return Ok(());
        }
        let records: Vec<RecordEntry> = typed(&Value::Array(records))?;

        let mut rows = vec![
            ("review item", item.id.to_string()),
            ("kind", format_enum_word(&item.kind)),
            ("status", format_enum_word(&item.status)),
            ("type", item.entity_type.to_string()),
            (
                "raised by",
                match split {
                    // A retract run, or the load run that brought a record back.
                    true => format!("run {}", item.run),
                    false => format!("reconcile run {}", item.run),
                },
            ),
        ];
        if let (true, Some(identity)) = (split, item.identities.first()) {
            rows.push(("identity", identity.to_string()));
        }
        if let Some(seq) = item.resolved_seq {
            rows.push(("resolved at", format!("log position {seq}")));
        }
        if let Some(run) = item.superseded_by {
            rows.push(match split {
                true => ("closed by", format!("run {run}")),
                false => ("raised again by", format!("reconcile run {run}")),
            });
        }
        if let Some(dismissal) = &item.dismissed {
            let mut by = format!("{} at {}", dismissal.by, format_timestamp(&dismissal.at));
            if let Some(reason) = &dismissal.reason {
                by.push_str(&format!(", \"{reason}\""));
            }
            rows.push(("dismissed by", by));
        }
        fields(&rows);
        println!();
        if split {
            let taken_back: Vec<RecordEntry> = typed(&taken_back)?;
            possible_split(&item, &records, &taken_back);
            return Ok(());
        }
        table(
            "records left without an identity",
            &["record", "source and key", "now"],
            records
                .iter()
                .map(|record| {
                    vec![
                        record.id.to_string(),
                        record_name(record),
                        match (record.identity, record.retracted) {
                            (Some(identity), _) => format!("in identity {identity}"),
                            (None, Some(run)) => {
                                format!(
                                    "taken back by its source in run {run}, so it does not wait"
                                )
                            }
                            (None, None) => "waits for an identity".to_owned(),
                        },
                    ]
                })
                .collect(),
        );
        if !item.identities.is_empty() {
            println!("\nidentities they matched");
            for identity in &item.identities {
                println!("  {identity}");
            }
        }
        Ok(())
    }
}

/// What a review item is about, in a few words.
fn describe_review_item(item: &ReviewEntry) -> String {
    if item.kind == ReviewKind::PossibleSplit {
        return format!(
            "{} of one identity in {} that nothing holds together",
            count(item.records.len() as u64, "record"),
            count(item.groups.len() as u64, "group")
        );
    }
    let records = count(item.records.len() as u64, "record");
    match item.identities.len() {
        0 => format!("{records} without an identity, which match each other"),
        matched => format!(
            "{records} without an identity, which matched {}",
            count(matched as u64, "identity")
        ),
    }
}

/// A `possible_split` item: the question it asks, the groups its records
/// fall into, the records whose retraction left the question, and what
/// answers it.
fn possible_split(item: &ReviewEntry, records: &[RecordEntry], taken_back: &[RecordEntry]) {
    let identity = item
        .identities
        .first()
        .map_or_else(|| "-".to_owned(), ToString::to_string);
    println!("Are these records still one thing?");
    println!(
        "They share an identity, and a record that held them together was taken back by its source."
    );
    println!(
        "Each group below still holds together. Nothing the store knows joins one group to another:"
    );
    println!("no match found by a run, no `must_link`, and no decision by a reviewer or an agent.");
    for (number, group) in item.groups.iter().enumerate() {
        println!("\ngroup {}", number + 1);
        let rows: Vec<Vec<String>> = group
            .iter()
            .filter_map(|id| records.iter().find(|record| record.id == *id))
            .map(|record| {
                vec![
                    record.id.to_string(),
                    record_name(record),
                    match (record.identity, record.retracted) {
                        (Some(now), _) if now.to_string() == identity => {
                            "still in the identity".to_owned()
                        }
                        (Some(now), _) => format!("now in identity {now}"),
                        (None, Some(run)) => format!("taken back by its source in run {run}"),
                        (None, None) => "now without an identity".to_owned(),
                    },
                ]
            })
            .collect();
        aligned(&rows, 2);
    }
    if !taken_back.is_empty() {
        println!("\ntaken back by their source, and no longer in the identity");
        let rows: Vec<Vec<String>> = taken_back
            .iter()
            .map(|record| {
                vec![
                    record.id.to_string(),
                    record_name(record),
                    record
                        .retracted
                        .map_or_else(String::new, |run| format!("in run {run}")),
                ]
            })
            .collect();
        aligned(&rows, 2);
    }
    if item.status != ReviewStatus::Open {
        return;
    }
    println!("\nwhat answers it");
    indented(&[
        (
            "they are different things",
            format!(
                "split the identity: POST /v1/identities/{identity}/split with the records of a group"
            ),
        ),
        (
            "they are one thing",
            "say so with a `must_link` between a record of each group: POST /v1/changesets"
                .to_owned(),
        ),
        (
            "leave it as it is",
            format!("dismiss the item: POST /v1/review/{}/dismiss", item.id),
        ),
    ]);
    println!("The review screen of the studio does each of these.");
}
