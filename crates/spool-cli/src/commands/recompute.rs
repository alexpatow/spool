//! `spool recompute`: what the identities would be under the rules as they
//! are now, against the identities there are, and applying that when asked.

use anstream::println;
use clap::Args;
use serde_json::json;
use spool::core::IdentityId;
use spool::{
    After, Change, ChangeDetail, Contradiction, Fate, RecomputeApplied, RecomputeReport,
    TypeRecompute,
};

use super::remote::{Remote, RemoteArgs, typed};
use super::show::count;
use crate::Failure;
use crate::output::{Output, OutputArgs, aligned, fields, hint, table};
use crate::progress;

/// Show what the current rules would change in the identities
///
/// Shows what the identities would be if every record were matched again,
/// all at once, by the rules as they are now, against the identities there
/// are. It changes nothing. Identities keep their members when rules change,
/// so this is how a better rule reaches records that are already placed.
///
/// What people decided counts for more than the rules: a `must_link` or a
/// `cannot_link` holds, records a person or an agent put together stay
/// together, and records a reviewer dismissed from review stay without an
/// identity. Records whose source took them back are left out.
///
/// An identity keeps its ID with the group that gets most of its records.
/// Where several identities have their most in one group, the one with the
/// most records there keeps its ID and the others are merged into it, so that
/// their IDs stand for it. A group the rules join but something in it says
/// differ waits for review without an identity, as records do after a match.
///
/// The summary comes first, then the largest changes. It ends with the
/// command that applies it, which names a plan: a hash of where each record
/// is and would go. Applying works the result out again and refuses if it is
/// not that plan, so what is applied is what was shown.
#[derive(Args)]
#[command(
    after_help = "Examples:\n  spool recompute --detail\n  spool recompute --apply <plan> --reason \"stricter name rule\""
)]
pub struct RecomputeArgs {
    /// One entity type [default: every type]
    #[arg(long = "type", value_name = "NAME")]
    entity_type: Option<String>,
    /// Show the change that holds an identity, record by record
    #[arg(long, value_name = "ID", conflicts_with = "apply")]
    identity: Option<IdentityId>,
    /// List every change, not only the largest
    #[arg(long, conflicts_with = "apply")]
    detail: bool,
    /// How many changes to list [default: 10, or 1000 with --detail]
    #[arg(long, conflicts_with = "apply")]
    limit: Option<u64>,
    /// Pass over this many of the largest changes first
    #[arg(long, value_name = "N", conflicts_with = "apply")]
    offset: Option<u64>,
    /// Make the identities what the preview with this plan showed
    ///
    /// The change is a changeset in the log, by the engine, from runs started
    /// in your name.
    #[arg(long, value_name = "PLAN")]
    apply: Option<String>,
    /// Why, kept with the changeset
    #[arg(long, value_name = "TEXT", requires = "apply")]
    reason: Option<String>,
    #[command(flatten)]
    output: OutputArgs,
    #[command(flatten)]
    remote: RemoteArgs,
}

/// How many changes `--detail` lists when `--limit` does not say.
const DETAIL_LIMIT: u64 = 1_000;

pub fn run(arguments: RecomputeArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool recompute`")?;
    let remote = Remote::new(&arguments.remote);
    let entity_type = arguments.entity_type.as_deref();
    if let Some(name) = entity_type {
        let info = remote.info()?;
        let types = &info.store.entity_types;
        if !types.iter().any(|declared| declared.name.as_str() == name) {
            let declared: Vec<String> = types
                .iter()
                .map(|declared| format!("`{}`", declared.name))
                .collect();
            return Err(Failure::failed(match declared.is_empty() {
                true => format!("the project has no entity type `{name}`, and declares none"),
                false => format!(
                    "the project has no entity type `{name}`. It has {}.",
                    declared.join(", ")
                ),
            }));
        }
    }

    if let Some(plan) = &arguments.apply {
        let mut request = json!({ "plan": plan });
        if let Some(name) = entity_type {
            request["entity_type"] = json!(name);
        }
        if let Some(reason) = &arguments.reason {
            request["reason"] = json!(reason);
        }
        let waiting = progress::spinner("Applying the plan");
        let answer = remote.post("/recompute/apply", &request);
        waiting.finish_and_clear();
        let answer = answer?;
        match out.is_table() {
            true => print_applied(&typed(&answer)?),
            false => out.json(&answer),
        }
        return Ok(());
    }

    let detail = arguments.detail;
    let limit = arguments.limit.unwrap_or(if detail {
        DETAIL_LIMIT
    } else {
        spool::DEFAULT_CHANGES
    });
    let mut request = json!({ "limit": limit });
    if let Some(offset) = arguments.offset {
        request["offset"] = json!(offset);
    }
    if let Some(name) = entity_type {
        request["entity_type"] = json!(name);
    }
    let identity = arguments.identity;
    if let Some(identity) = identity {
        request["identity"] = json!(identity);
    }
    let waiting = progress::spinner("Recomputing the identities");
    let answer = remote.post("/recompute", &request);
    waiting.finish_and_clear();
    let answer = answer?;
    if !out.is_table() {
        out.json(&answer);
        return Ok(());
    }
    let report: RecomputeReport = typed(&answer)?;
    match identity {
        Some(identity) => print_detail(&report, identity, entity_type),
        None => print_report(&report, entity_type),
    }
}

fn print_report(report: &RecomputeReport, entity_type: Option<&str>) -> Result<(), Failure> {
    if report.types.is_empty() {
        println!("the project declares no entity type, so there is nothing to recompute");
        return Ok(());
    }
    let several = report.types.len() > 1;
    let mut first = true;
    for recomputed in &report.types {
        if recomputed.changes_total == 0 && !several {
            continue;
        }
        if !first {
            println!();
        }
        first = false;
        print_type(recomputed);
    }
    let changes: u64 = report.types.iter().map(|t| t.changes_total).sum();
    if changes == 0 {
        if !first {
            println!();
        }
        println!("nothing would change: the identities are what the rules say they are");
        return Ok(());
    }
    let mut apply = format!("spool recompute --apply {}", report.plan);
    if let Some(name) = entity_type {
        apply = format!("spool recompute --type {name} --apply {}", report.plan);
    }
    hint(format!("nothing was changed. This applies it:\n{apply}"));
    Ok(())
}

fn print_type(recomputed: &TypeRecompute) {
    let summary = &recomputed.summary;
    println!(
        "type `{}`: {}, in {} now and {} after",
        recomputed.entity_type,
        count(summary.records, "record"),
        count(summary.identities_now, "identity"),
        summary.identities_after,
    );
    if recomputed.changes_total == 0 {
        println!("nothing would change");
        return;
    }
    let mut rows: Vec<(&str, String)> = vec![(
        "unchanged",
        format!(
            "{} would keep exactly their records",
            count(summary.unchanged, "identity")
        ),
    )];
    let mut add = |label: &'static str, number: u64, text: String| {
        if number > 0 {
            rows.push((label, text));
        }
    };
    add(
        "split",
        summary.splitting,
        format!(
            "{} would go to more than one identity",
            records_of(summary.splitting)
        ),
    );
    add(
        "merged",
        summary.merging,
        format!(
            "{} would take records from more than one; {} merged into another, \
             its ID standing for it",
            count(summary.merging, "identity"),
            count(summary.merged_away, "identity")
        ),
    );
    add(
        "gain",
        summary.gaining,
        format!("{} would gain records", count(summary.gaining, "identity")),
    );
    add(
        "lose",
        summary.losing,
        format!("{} would lose records", count(summary.losing, "identity")),
    );
    add(
        "new",
        summary.new_identities,
        format!(
            "{} would be made",
            count(summary.new_identities, "identity")
        ),
    );
    add(
        "emptied",
        summary.emptied,
        format!(
            "{} would be left with no records, every one waiting for review",
            count(summary.emptied, "identity")
        ),
    );
    add(
        "moved",
        summary.records_moved,
        format!(
            "{} would change identity",
            count(summary.records_moved, "record")
        ),
    );
    add(
        "placed",
        summary.records_placed,
        format!(
            "{} without an identity would get one",
            count(summary.records_placed, "record")
        ),
    );
    add(
        "waiting",
        summary.records_waiting,
        format!(
            "{} would wait for review in {}, {} of them with an identity now",
            count(summary.records_waiting, "record"),
            count(summary.review_items, "conflict item"),
            summary.records_unplaced
        ),
    );
    add(
        "resolved",
        summary.review_items_closed,
        format!(
            "{} would close, its records placed",
            count(summary.review_items_closed, "open review item")
        ),
    );
    rows.push((
        "pairs",
        format!(
            "{} share an identity now, {} would after, {} of them both",
            count(summary.pairs_now, "pair"),
            summary.pairs_after,
            summary.pairs_kept
        ),
    ));
    fields(&rows);

    let decisions = &recomputed.decisions;
    let mut rows: Vec<(&str, String)> = Vec::new();
    if decisions.cannot_link > 0 || decisions.must_link > 0 {
        rows.push((
            "constraints",
            format!(
                "{} and {} in force, each holding whatever the rules say",
                count(decisions.must_link, "must_link"),
                count(decisions.cannot_link, "cannot_link")
            ),
        ));
    }
    if decisions.blocked_by_constraints > 0 {
        rows.push((
            "blocked",
            format!(
                "{} the rules join would wait for review because a `cannot_link` says two \
                 of its records differ",
                count(decisions.blocked_by_constraints, "group")
            ),
        ));
    }
    if decisions.blocked_by_rules > 0 {
        rows.push((
            "contradicted",
            format!(
                "{} the rules join would wait for review because a no_match rule holds \
                 inside it",
                count(decisions.blocked_by_rules, "group")
            ),
        ));
    }
    if decisions.held_together > 0 || decisions.dismissed_splits > 0 {
        let mut text = format!(
            "{} that people put together would stay together",
            count(decisions.held_together, "group")
        );
        if decisions.dismissed_splits > 0 {
            text += &format!(
                ", and the records of {}",
                count(decisions.dismissed_splits, "dismissed possible_split item")
            );
        }
        rows.push(("held", text));
    }
    if decisions.changes_shaped_by_people > 0 {
        rows.push((
            "shaped",
            format!(
                "{} touch an identity that a person or an agent changed, and may undo it",
                count(decisions.changes_shaped_by_people, "change")
            ),
        ));
    }
    if decisions.changes_with_approved_drafts > 0 {
        rows.push((
            "approved",
            format!(
                "{} touch records an approved draft placed, which binds no more than its run",
                count(decisions.changes_with_approved_drafts, "change")
            ),
        ));
    }
    if !rows.is_empty() {
        println!();
        println!("decisions");
        indent(&rows);
    }

    println!();
    let shown = recomputed.changes.len() as u64;
    let first = recomputed.changes.first().map_or(0, |change| change.rank);
    if shown == 0 {
        println!(
            "{} in all, none from there",
            count(recomputed.changes_total, "change")
        );
        return;
    }
    let heading = if shown == recomputed.changes_total {
        format!("{}, the largest first", count(shown, "change"))
    } else {
        format!(
            "changes {} to {} of {}, the largest first",
            first,
            first + shown - 1,
            recomputed.changes_total
        )
    };
    println!("{heading}");
    let rows: Vec<Vec<String>> = recomputed.changes.iter().map(change_row).collect();
    table(
        "changes",
        &["#", "kind", "records", "identity", "now", "after"],
        rows,
    );
    if shown < recomputed.changes_total {
        hint(format!(
            "`--detail` lists every change, and `--offset {}` goes on from here",
            first + shown - 1
        ));
    }
    hint("`--identity <id>` shows a change record by record");
}

fn indent(rows: &[(&str, String)]) {
    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    for (label, text) in rows {
        println!("  {label:width$}  {text}");
    }
}

fn records_of(identities: u64) -> String {
    format!("the records of {}", count(identities, "identity"))
}

fn change_row(change: &Change) -> Vec<String> {
    let identity = change
        .before
        .iter()
        .find_map(|before| before.identity)
        .map_or_else(|| "-".to_owned(), |identity| identity.to_string());
    let now = listed(
        change
            .before
            .iter()
            .map(|before| match before.identity {
                Some(_) => format!("{} ({})", name(&before.label), before.records),
                None => format!("{} without an identity", before.records),
            })
            .collect(),
    );
    let mut after = listed(change.after.iter().map(after_cell).collect());
    let mut notes = Vec::new();
    if let Some(contradiction) = change.blocked_by.first() {
        notes.push(format!("blocked by {}", contradiction_text(contradiction)));
    }
    if !change.shaped_by.is_empty() {
        let seqs: Vec<String> = change
            .shaped_by
            .iter()
            .map(|decision| format!("{} at {}", decision.actor, decision.seq))
            .collect();
        notes.push(format!("shaped by {}", listed(seqs)));
    }
    if change.approved_drafts > 0 {
        notes.push(format!(
            "{} placed by an approved draft",
            count(change.approved_drafts, "record")
        ));
    }
    if !notes.is_empty() {
        after += &format!("; {}", notes.join("; "));
    }
    vec![
        change.rank.to_string(),
        change.kind.as_str().to_owned(),
        change.records.to_string(),
        identity,
        now,
        after,
    ]
}

fn name(label: &Option<String>) -> String {
    match label {
        Some(label) => label.clone(),
        None => "unnamed".to_owned(),
    }
}

fn after_cell(after: &After) -> String {
    match after.fate {
        Fate::Kept if after.merged.is_empty() => {
            format!("{} ({}, keeps its ID)", name(&after.label), after.records)
        }
        Fate::Kept => format!(
            "{} ({}, keeps its ID, {} merged in)",
            name(&after.label),
            after.records,
            count(after.merged.len() as u64, "identity")
        ),
        Fate::New => format!("{} ({}, new)", name(&after.label), after.records),
        Fate::Waiting => format!("{} waiting for review", after.records),
    }
}

/// At most three items, then how many more.
fn listed(items: Vec<String>) -> String {
    const SHOWN: usize = 3;
    if items.len() <= SHOWN {
        return items.join(", ");
    }
    format!(
        "{}, and {} more",
        items[..SHOWN].join(", "),
        items.len() - SHOWN
    )
}

fn contradiction_text(contradiction: &Contradiction) -> String {
    match &contradiction.constraint {
        Some(decision) => format!(
            "a cannot_link by {} at log position {}",
            decision.actor, decision.seq
        ),
        None if contradiction.rules.is_empty() => "a no_match".to_owned(),
        None => {
            let rules: Vec<String> = contradiction
                .rules
                .iter()
                .map(|rule| format!("`{rule}`"))
                .collect();
            format!("no_match rule {}", rules.join(", "))
        }
    }
}

fn print_detail(
    report: &RecomputeReport,
    identity: IdentityId,
    entity_type: Option<&str>,
) -> Result<(), Failure> {
    let Some((recomputed, detail)) = report
        .types
        .iter()
        .find_map(|recomputed| Some((recomputed, recomputed.detail.as_ref()?)))
    else {
        println!("identity {identity} would not change: it is what the rules say it is");
        return Ok(());
    };
    print_change(recomputed, detail);
    let mut apply = format!("spool recompute --apply {}", report.plan);
    if let Some(name) = entity_type {
        apply = format!("spool recompute --type {name} --apply {}", report.plan);
    }
    hint(format!(
        "nothing was changed. This applies every change, not only this one:\n{apply}"
    ));
    Ok(())
}

fn print_change(recomputed: &TypeRecompute, detail: &ChangeDetail) {
    let change = &detail.change;
    fields(&[
        (
            "change",
            format!(
                "{} of {} in type `{}`, the largest first",
                change.rank, recomputed.changes_total, recomputed.entity_type
            ),
        ),
        ("kind", change.kind.as_str().to_owned()),
        (
            "records",
            format!("{}, {} of them would move", change.records, change.moved),
        ),
    ]);
    println!();
    println!("now");
    let rows: Vec<Vec<String>> = change
        .before
        .iter()
        .map(|before| match before.identity {
            Some(identity) => vec![
                identity.to_string(),
                name(&before.label),
                count(before.records, "record"),
            ],
            None => vec![format!(
                "{} without an identity",
                count(before.records, "record")
            )],
        })
        .collect();
    aligned(&rows, 2);
    println!();
    println!("after");
    let mut rows: Vec<Vec<String>> = Vec::new();
    for (at, after) in change.after.iter().enumerate() {
        let what = match (after.fate, after.identity) {
            (Fate::Kept, Some(identity)) => format!("{identity}, keeps its ID"),
            (Fate::New, _) => "a new identity".to_owned(),
            _ => "waits for review without an identity".to_owned(),
        };
        rows.push(vec![
            (at + 1).to_string(),
            what,
            name(&after.label),
            count(after.records, "record"),
        ]);
    }
    aligned(&rows, 2);
    for (at, after) in change.after.iter().enumerate() {
        for merged in &after.merged {
            println!(
                "  {merged} is merged into {}, and its ID stands for it",
                at + 1
            );
        }
    }
    println!();
    let rows: Vec<Vec<String>> = detail
        .records
        .iter()
        .map(|record| {
            vec![
                record.record.to_string(),
                match &record.key {
                    Some(key) => format!("{}:{key}", record.source),
                    None => record.source.clone(),
                },
                record.label.clone().unwrap_or_else(|| "-".to_owned()),
                record
                    .from
                    .map_or_else(|| "-".to_owned(), |identity| identity.to_string()),
                (record.to + 1).to_string(),
            ]
        })
        .collect();
    table(
        "records",
        &["record", "source and key", "label", "now", "after"],
        rows,
    );
    if !change.blocked_by.is_empty() {
        println!();
        println!("what stops a group from being an identity");
        for contradiction in &change.blocked_by {
            println!(
                "  {} and {}: {}",
                contradiction.records[0],
                contradiction.records[1],
                contradiction_text(contradiction)
            );
        }
    }
    if !detail.constraints.is_empty() {
        println!();
        println!("constraints between its records");
        for note in &detail.constraints {
            println!(
                "  {} {} and {}, by {} at log position {}{}",
                note.kind.as_str(),
                note.records[0],
                note.records[1],
                note.decision.actor,
                note.decision.seq,
                reason(&note.decision.reason)
            );
        }
    }
    if !change.shaped_by.is_empty() {
        println!();
        println!("changed by people or agents: this change may undo what they did");
        for decision in &change.shaped_by {
            println!(
                "  log position {} by {}{}",
                decision.seq,
                decision.actor,
                reason(&decision.reason)
            );
        }
    }
    if !detail.approved_drafts.is_empty() {
        println!();
        println!("placed by approved drafts, which bind no more than the run that drafted them");
        for placement in &detail.approved_drafts {
            println!(
                "  {} by draft {} at log position {}",
                placement.record, placement.draft, placement.seq
            );
        }
    }
}

fn reason(reason: &Option<String>) -> String {
    match reason {
        Some(reason) => format!(" (\"{reason}\")"),
        None => String::new(),
    }
}

fn print_applied(applied: &RecomputeApplied) {
    for (at, done) in applied.types.iter().enumerate() {
        if at > 0 {
            println!();
        }
        let summary = &done.summary;
        let mut rows = vec![
            (
                "type",
                format!(
                    "`{}`, match run {}, reconcile run {}",
                    done.entity_type, done.match_run, done.reconcile_run
                ),
            ),
            (
                "changeset",
                match &done.committed {
                    Some(committed) => format!(
                        "at log position {}, by the engine: `spool show log` has it",
                        committed.seq
                    ),
                    None => "none: nothing changed".to_owned(),
                },
            ),
            (
                "identities",
                format!(
                    "{} now, from {}: {} made, {} merged away",
                    summary.identities_after,
                    summary.identities_now,
                    summary.new_identities,
                    summary.merged_away
                ),
            ),
            (
                "records",
                format!(
                    "{} moved, {} placed, {} left an identity",
                    summary.records_moved, summary.records_placed, summary.records_unplaced
                ),
            ),
        ];
        if !done.review.is_empty() {
            rows.push((
                "review",
                format!(
                    "{} wait in {}: `spool show review --status open` lists them",
                    count(summary.records_waiting, "record"),
                    count(done.review.len() as u64, "conflict item")
                ),
            ));
        }
        fields(&rows);
    }
    println!();
    println!("applied plan {}", applied.plan);
}
