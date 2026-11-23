//! `spool show runs`: the list of runs, and one run with what it reported.

use anstream::println;
use spool::core::Committed;
use spool::core::ReviewItem;
use spool::{AppliedType, ExtractReport, LoadReport, RetractReport, RunEntry, TypeResolution};
use spool_cli::text::count;

use super::{Query, RunsArgs, View, format_enum_word, format_timestamp};
use crate::Failure;
use crate::commands::remote::typed;
use crate::output::{Cell, Table, fields};

impl View<'_> {
    pub(super) fn runs(&self, what: &RunsArgs) -> Result<(), Failure> {
        let query = Query::default()
            .add("entity_type", what.entity_type.as_ref())
            .add("status", what.status.as_ref())
            .add("kind", what.kind.as_ref())
            .add("limit", what.limit)
            .0;
        let answer = self.remote.list("/runs", &query)?;
        let runs: Vec<RunEntry> = typed(&answer["runs"])?;
        let mut table =
            Table::new("runs").columns(["run", "kind", "status", "about", "started by", "started"]);
        for run in &runs {
            let about = [
                run.entity_type.as_ref().map(ToString::to_string),
                run.strategy.as_ref().map(|strategy| strategy.name.clone()),
                run.extractor
                    .as_ref()
                    .map(|extractor| extractor.name.clone()),
            ];
            table.row(vec![
                run.run.to_string().into(),
                format_enum_word(&run.kind).into(),
                run.status.to_string().into(),
                about
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
                run.started_by.to_string().into(),
                Cell::shortened(format_timestamp(&run.started_at), &run.started_at),
            ]);
        }
        self.out.items(&answer, "runs", &table)?;
        Ok(())
    }

    pub(super) fn run(&self, run: u64) -> Result<(), Failure> {
        let answer = self.remote.get(&format!("/runs/{run}"), &[], || {
            format!("there is no run {run}")
        })?;
        if !self.out.is_table() {
            self.out.json(&answer);
            return Ok(());
        }
        let run: RunEntry = typed(&answer["run"])?;
        let mut rows: Vec<(&'static str, String)> = vec![
            ("run", run.run.to_string()),
            ("kind", format_enum_word(&run.kind)),
            ("status", run.status.to_string()),
        ];
        if let Some(entity_type) = &run.entity_type {
            rows.push(("type", entity_type.to_string()));
        }
        if let Some(strategy) = &run.strategy {
            rows.push((
                "strategy",
                format_name_and_version(&strategy.name, strategy.version.as_deref()),
            ));
        }
        if let Some(extractor) = &run.extractor {
            rows.push((
                "extractor",
                format_name_and_version(&extractor.name, Some(&extractor.version)),
            ));
        }
        rows.push(("started by", run.started_by.to_string()));
        rows.push(("started", format_timestamp(&run.started_at)));
        if let Some(finished) = &run.finished_at {
            rows.push(("finished", format_timestamp(finished)));
        }
        if let Some(error) = &run.error {
            rows.push(("error", error.clone()));
        }
        if let Some(report) = &run.load {
            load_rows(report, &mut rows);
        }
        if let Some(report) = &run.retraction {
            retraction_rows(report, &mut rows);
        }
        if let Some(report) = &run.extraction {
            extraction_rows(report, &mut rows);
        }
        if let Some(report) = &run.recompute {
            recompute_rows(report, &mut rows);
        }
        let suggestions = match &run.resolution {
            Some(report) => resolution_rows(report, &mut rows),
            None => Vec::new(),
        };
        fields(&rows);
        if !suggestions.is_empty() {
            println!();
        }
        for line in suggestions {
            println!("{line}");
        }
        Ok(())
    }
}

/// What a load run reports.
fn load_rows(report: &LoadReport, rows: &mut Vec<(&'static str, String)>) {
    rows.push((
        "rows",
        format!(
            "{} received: {} new, {} known, {} new versions of a known row",
            report.received, report.inserted, report.duplicates, report.superseded
        ),
    ));
    if report.restored > 0 {
        rows.push((
            "came back",
            format!(
                "{} the source had taken back, {} of them to the identity they left",
                count(report.restored, "record"),
                report.returned
            ),
        ));
    }
    if report.possible_splits > 0 {
        rows.push(("left for review", left_for_review(report.possible_splits)));
    }
    if let Some(committed) = &report.committed {
        rows.push(("committed", format_committed(committed)));
    }
}

/// What a run that took rows back reports.
fn retraction_rows(report: &RetractReport, rows: &mut Vec<(&'static str, String)>) {
    rows.push((
        "keys",
        format!("{} named, {} unknown", report.received, report.unknown),
    ));
    rows.push((
        "taken back",
        format!(
            "{}, {} of them left an identity",
            count(report.retracted, "record"),
            report.detached
        ),
    ));
    if report.possible_splits > 0 {
        rows.push(("left for review", left_for_review(report.possible_splits)));
    }
    if let Some(committed) = &report.committed {
        rows.push(("committed", format_committed(committed)));
    }
}

/// What an extract run reports.
fn extraction_rows(report: &ExtractReport, rows: &mut Vec<(&'static str, String)>) {
    rows.push(("documents", format!("{} read", report.documents)));
    rows.push((
        "mentions",
        format!(
            "{} found: {} new, {} known, {} new versions of a known mention",
            report.mentions, report.inserted, report.duplicates, report.superseded
        ),
    ));
    if let Some(load_run) = report.load_run {
        rows.push(("stored by", format!("load run {load_run}")));
    }
}

/// What a match run that applied a recomputation reports.
fn recompute_rows(report: &AppliedType, rows: &mut Vec<(&'static str, String)>) {
    let summary = &report.summary;
    rows.push((
        "recompute",
        format!(
            "every record judged again and applied: {} in {} identities, from {}",
            count(summary.records, "record"),
            summary.identities_after,
            summary.identities_now
        ),
    ));
    rows.push((
        "moved",
        format!(
            "{}, {} placed, {} left an identity to wait for review",
            count(summary.records_moved, "record"),
            summary.records_placed,
            summary.records_unplaced
        ),
    ));
    rows.push(("reconciled by", format!("run {}", report.reconcile_run)));
    rows.push((
        "committed",
        match &report.committed {
            Some(committed) => format_committed(committed),
            None => "nothing to commit".to_owned(),
        },
    ));
}

/// What a match run reports, and the lines it has for a reviewer: its
/// review items and the merges and splits it suggests.
fn resolution_rows(report: &TypeResolution, rows: &mut Vec<(&'static str, String)>) -> Vec<String> {
    let mut suggestions = Vec::new();
    rows.push((
        "examined",
        format!(
            "{}, {} of them left for review before",
            count(report.new_records, "record"),
            report.reexamined
        ),
    ));
    rows.push(("candidates", count(report.candidates, "pair")));
    rows.push((
        "judgements",
        format!(
            "{} match, {} no_match, {} unsure",
            report.judgements.match_, report.judgements.no_match, report.judgements.unsure
        ),
    ));
    rows.push(("placed", format_placed_records(report)));
    rows.push((
        "left out",
        format!(
            "{} in {}",
            count(report.records_unassigned, "record"),
            count(report.review.len() as u64, "review item")
        ),
    ));
    rows.push(("reconciled by", format!("run {}", report.reconcile_run)));
    match (&report.committed, report.draft) {
        (Some(committed), _) => rows.push(("committed", format_committed(committed))),
        (None, Some(draft)) => rows.push(("draft", draft.to_string())),
        (None, None) => rows.push(("committed", "nothing to commit".to_owned())),
    }
    for item in &report.review {
        suggestions.push(match item {
            ReviewItem::PossibleMerge {
                records,
                identities,
            } => format!(
                "review: {} matched {}, which may be one",
                count(records.len() as u64, "record"),
                count(identities.len() as u64, "identity"),
            ),
            ReviewItem::Conflict { records, .. } => format!(
                "review: the evidence about {} contradicts itself",
                count(records.len() as u64, "record"),
            ),
        });
    }
    for merge in report.suggestions.iter().flat_map(|found| &found.merges) {
        suggestions.push(format!(
            "suggested merge: {} and {} ({} match, {} no_match between their members)",
            merge.identities[0], merge.identities[1], merge.matches, merge.no_matches
        ));
    }
    for split in report.suggestions.iter().flat_map(|found| &found.splits) {
        suggestions.push(format!(
            "suggested split: {} ({} no_match, {} match between its members)",
            split.identity, split.no_matches, split.matches
        ));
    }
    suggestions
}

/// Where a run's changeset went in the log.
fn format_committed(committed: &Committed) -> String {
    format!(
        "log position {}, changeset {}",
        committed.seq, committed.changeset
    )
}

/// What a run says of the `possible_split` items it raised.
fn left_for_review(identities: u64) -> String {
    format!(
        "{} with records that nothing holds together now (`possible_split`)",
        count(identities, "identity")
    )
}

fn format_name_and_version(name: &str, version: Option<&str>) -> String {
    match version {
        Some(version) => format!("{name}, version {version}"),
        None => name.to_owned(),
    }
}

/// What a match run placed, and the earlier versions it moved along.
pub(in crate::commands) fn format_placed_records(report: &spool::TypeResolution) -> String {
    let mut placed = format!(
        "{}, {} created",
        count(report.records_assigned, "record"),
        count(report.identities_created, "identity")
    );
    if report.versions_moved > 0 {
        placed += &format!(
            ", {} moved along",
            count(report.versions_moved, "earlier version")
        );
    }
    placed
}
