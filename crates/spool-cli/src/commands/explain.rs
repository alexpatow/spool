//! `spool explain`: why two records share an identity or do not, and what
//! holds an identity together.
//!
//! It reads through the server's API and prints what it answered. With
//! `--json` it prints the answer itself. Nothing here decides anything: a
//! sentence that says why comes from the engine.

use anstream::println;
use clap::Args;
use spool::core::IdentityId;
use spool::{Explanation, IdentityExplanation, Link, Named, RuleResult, Split, how_linked};
use spool_cli::text::count;

use super::remote::{Remote, RemoteArgs, typed};
use super::show::records::find_record;
use super::show::{label, plain, word};
use crate::output::{Output, OutputArgs, aligned, fields, table};
use crate::{Failure, Kind};

/// Say why two records share an identity, or what holds one together
///
/// With two records, it says why they share an identity or don't. You get
/// where each one is and what placed it there, what each run that compared
/// them concluded, the constraints and review items that name them, and what
/// the current rules say about the pair. When they share an identity, it also
/// shows the chain of links between them, with the rule and values behind
/// each link, the rules every chain passes through, and the fewest links
/// whose removal would separate them.
///
/// With one identity, it shows how much of it is held together only through
/// chains, the pieces it would split into without each rule's links, and its
/// weakest links: the fewest links whose removal would split it, with the
/// two sides each would leave. That's where a wrong merge came in. Name an
/// identity by its ID or by any of its records.
///
/// Name a record by its ID or as <source>:<key>, which means the latest
/// version of that row. Name a record found in a document the way lists show
/// it, <source>:<document>[<start>-<end>].
#[derive(Args)]
#[command(after_help = "Examples:\n  spool explain crm:c-1 crm:c-2\n  spool explain crm:c-1")]
pub struct ExplainArgs {
    /// A record; alone, an identity, by its ID or by any of its records
    #[arg(value_name = "RECORD")]
    first: String,
    /// A second record, to say why the two share an identity or do not
    #[arg(value_name = "RECORD")]
    second: Option<String>,
    #[command(flatten)]
    output: OutputArgs,
    #[command(flatten)]
    remote: RemoteArgs,
}

pub fn explain(arguments: ExplainArgs) -> Result<(), Failure> {
    let remote = Remote::new(&arguments.remote);
    let out = Output::report(&arguments.output, "`spool explain`")?;
    let (a, b) = match &arguments.second {
        None => return explain_identity(&remote, &arguments.first, out),
        Some(second) => (&arguments.first, second),
    };
    let a = find_record(&remote, a)?;
    let b = find_record(&remote, b)?;
    let answer = remote.list(
        "/explain",
        &[("a", a.id.to_string()), ("b", b.id.to_string())],
    )?;
    match out.is_table() {
        true => print_explanation(&typed(&answer)?),
        false => out.json(&answer),
    }
    Ok(())
}

/// `spool explain <identity>`: by its ID, or by a record of it.
fn explain_identity(remote: &Remote, reference: &str, out: Output) -> Result<(), Failure> {
    let identity = match reference.parse::<IdentityId>() {
        Ok(identity)
            if remote
                .find(&format!("/identities/{identity}"), &[])?
                .is_some() =>
        {
            identity
        }
        _ => {
            let record = find_record(remote, reference).map_err(|failure| {
                match failure.kind() == Kind::Failed && reference.parse::<IdentityId>().is_ok() {
                    true => Failure::failed(format!("there is no identity or record {reference}")),
                    false => failure,
                }
            })?;
            record
                .identity
                .ok_or_else(|| Failure::failed(format!("`{}` has no identity", record.name())))?
        }
    };
    let answer = remote.get(&format!("/identities/{identity}/explain"), &[], || {
        format!("there is no identity {identity}")
    })?;
    match out.is_table() {
        true => print_identity_explanation(&typed(&answer)?),
        false => out.json(&answer),
    }
    Ok(())
}

#[expect(clippy::too_many_lines)]
fn print_explanation(explanation: &Explanation) {
    for sentence in &explanation.summary {
        println!("{sentence}");
    }

    println!();
    let sides = [("left", &explanation.left), ("right", &explanation.right)];
    let rows: Vec<Vec<String>> = sides
        .iter()
        .map(|(side, explained)| {
            let record = &explained.record;
            vec![
                (*side).to_owned(),
                record.id.to_string(),
                record.name(),
                record.identity.map_or_else(
                    || "no identity".to_owned(),
                    |identity| format!("identity {identity}"),
                ),
            ]
        })
        .collect();
    aligned(&rows, 0);

    let mut names: Vec<&String> = Vec::new();
    for (_, explained) in &sides {
        for name in explained
            .features
            .iter()
            .flat_map(|features| features.keys())
        {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if !names.is_empty() {
        println!("\nfeatures");
        let mut rows = vec![vec![String::new(), "left".to_owned(), "right".to_owned()]];
        for name in names {
            let value = |side: usize| {
                sides[side]
                    .1
                    .features
                    .as_ref()
                    .and_then(|features| features.get(name))
                    .map_or_else(|| "-".to_owned(), plain)
            };
            rows.push(vec![name.clone(), value(0), value(1)]);
        }
        aligned(&rows, 2);
    }

    if let Some(rules) = &explanation.rules {
        println!("\nrules as they are now");
        let mut rows = Vec::new();
        for (group, results) in [
            ("block", &rules.block),
            ("no_match", &rules.no_match),
            ("match", &rules.match_),
        ] {
            for RuleResult { name, holds, .. } in results {
                rows.push(vec![
                    group.to_owned(),
                    name.clone(),
                    match holds {
                        Some(true) => "holds",
                        Some(false) => "does not hold",
                        None if group != "block" && rules.failed.is_some() => {
                            "cannot be applied to these values"
                        }
                        None => "cannot be decided: a feature it reads is null",
                    }
                    .to_owned(),
                ]);
            }
        }
        if rows.is_empty() {
            println!("  the entity type has none");
        }
        aligned(&rows, 2);
        if let Some(failed) = &rules.failed {
            println!("  applying the `no_match` and `match` rules failed: {failed}");
        }
    }

    if !explanation.comparisons.is_empty() {
        println!("\nruns that compared them");
        let rows: Vec<Vec<String>> = explanation
            .comparisons
            .iter()
            .map(|comparison| {
                let judged: Vec<String> = comparison
                    .judgements
                    .iter()
                    .map(|judgement| match judgement.score {
                        Some(score) => format!("{} ({score})", word(&judgement.verdict)),
                        None => word(&judgement.verdict),
                    })
                    .collect();
                vec![
                    format!("run {}", comparison.run),
                    comparison
                        .strategy
                        .as_ref()
                        .map_or_else(|| "-".to_owned(), |strategy| strategy.name.clone()),
                    match comparison.draft {
                        Some(draft) => format!("{}, draft {draft}", comparison.status),
                        None => comparison.status.to_string(),
                    },
                    match comparison.blocked_by.is_empty() {
                        true => "no block rule".to_owned(),
                        false => format!("block: {}", comparison.blocked_by.join(", ")),
                    },
                    match judged.is_empty() {
                        true => "no judgement".to_owned(),
                        false => judged.join(", "),
                    },
                ]
            })
            .collect();
        aligned(&rows, 2);
    }

    if !explanation.constraints.is_empty() {
        println!("\nconstraints between them");
        for record in &explanation.constraints {
            let until = match record.to {
                Some(to) => format!(" until position {to}"),
                None => String::new(),
            };
            println!(
                "  {} from log position {}{until}",
                word(&record.constraint.kind),
                record.from
            );
        }
    }

    if !explanation.review_items.is_empty() {
        println!("\nreview items that name them");
        for item in &explanation.review_items {
            println!(
                "  item {} ({}, {})",
                item.id,
                word(&item.kind),
                word(&item.status)
            );
        }
    }

    if explanation.path.len() > 1 {
        println!("\nwhat joins them");
        print_chain(&explanation.path);
    }

    if let Some(cut) = explanation
        .cut
        .as_ref()
        .filter(|_| explanation.path.len() > 1)
    {
        println!("\nwhat would part them");
        print_split(cut, ["with left", "with right"]);
    }
}

/// A chain of links as numbered steps: each record by its label and name,
/// and under it how it is linked to the next.
fn print_chain(path: &[Link]) {
    let Some(first) = path.first() else { return };
    let mut rows = vec![step(1, &first.from_record)];
    for (at, link) in path.iter().enumerate() {
        rows.push(vec![String::new(), format!("  {}", how_linked(link))]);
        rows.push(step(at + 2, &link.to_record));
    }
    for row in rows {
        println!("  {}", row.join(" ").trim_end());
    }
}

fn step(number: usize, record: &Named) -> Vec<String> {
    vec![format!("{number}."), said_record(record)]
}

fn said_record(record: &Named) -> String {
    match &record.label {
        Some(label) => format!("{label}  {}", record.name),
        None => record.name.clone(),
    }
}

/// The most links of a split that are printed.
const SPLIT_LINKS: usize = 5;

/// The links of a split, each from the first side to the second, and
/// what each side would hold.
fn print_split(split: &Split, titles: [&str; 2]) {
    for link in split.links.iter().take(SPLIT_LINKS) {
        println!(
            "  {} – {}",
            said_record(&link.from_record),
            said_record(&link.to_record)
        );
        println!("      {}", how_linked(link));
    }
    if split.links.len() > SPLIT_LINKS {
        println!(
            "  and {} more (--json lists them)",
            count((split.links.len() - SPLIT_LINKS) as u64, "link")
        );
    }
    for (title, side) in titles.iter().zip(&split.sides) {
        let names: Vec<String> = side
            .names
            .iter()
            .map(|name| format!("{} × {}", name.records, name.name))
            .collect();
        println!(
            "  {title}: {}{}",
            count(side.records, "record"),
            match names.is_empty() {
                true => String::new(),
                false => format!(" ({})", names.join(", ")),
            }
        );
    }
}

fn print_identity_explanation(explanation: &IdentityExplanation) {
    for sentence in &explanation.summary {
        println!("{sentence}");
    }
    println!();
    let mut rows = vec![("identity", explanation.current.to_string())];
    if explanation.current != explanation.id {
        rows.push(("asked for", explanation.id.to_string()));
    }
    rows.extend([
        ("type", explanation.entity_type.to_string()),
        ("name", label(explanation.label.as_deref())),
        ("records", explanation.records.to_string()),
        ("names", explanation.names.to_string()),
        ("pairs", explanation.pairs.to_string()),
        ("linked directly", explanation.linked_pairs.to_string()),
        ("only through chains", explanation.chained_pairs.to_string()),
        (
            "longest chain",
            format!("at least {}", count(explanation.longest_chain, "link")),
        ),
    ]);
    if explanation.parts > 1 {
        rows.push(("parts", explanation.parts.to_string()));
    }
    fields(&rows);

    if !explanation.top_names.is_empty() {
        println!();
        table(
            "names",
            &["name", "records"],
            explanation
                .top_names
                .iter()
                .map(|name| vec![name.name.clone(), name.records.to_string()])
                .collect(),
        );
    }
    if !explanation.rules.is_empty() {
        println!();
        table(
            "rules",
            &[
                "rule",
                "links",
                "alone",
                "pieces without it",
                "largest pieces",
            ],
            explanation
                .rules
                .iter()
                .map(|hold| {
                    vec![
                        hold.rule.clone(),
                        hold.links.to_string(),
                        hold.alone.to_string(),
                        hold.pieces.to_string(),
                        hold.largest
                            .iter()
                            .map(|piece| {
                                let mut names: Vec<&str> = piece
                                    .names
                                    .iter()
                                    .take(2)
                                    .map(|name| name.name.as_str())
                                    .collect();
                                if piece.names.len() > 2 {
                                    names.push("…");
                                }
                                match names.is_empty() {
                                    true => piece.records.to_string(),
                                    false => format!("{} ({})", piece.records, names.join(", ")),
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    ]
                })
                .collect(),
        );
    }
    for (at, split) in explanation.weakest.iter().enumerate() {
        println!(
            "\nweakest {}: {} would part it",
            at + 1,
            count(split.links.len() as u64, "link")
        );
        print_split(split, ["one side", "the other"]);
    }
}
