//! The sentences of an explanation: the summary of a pair and of an
//! identity, and how a link, a placement, a comparison and a reading of the
//! rules are said.

use spool_core::{ConstraintKind, RecordId, RunId, Verdict};

use super::{
    Comparison, Explanation, IdentityExplanation, Link, LinkKind, LinkValue, Named, Outcome,
    Placement, RuleReading, RuleResult, Side,
};
use crate::drafts::DraftStatus;
use crate::engine::StrategyRef;
use crate::links::MOST_CUT;
use crate::review::{ReviewKind, ReviewStatus};
use crate::runs::RunStatus;

/// A record in a sentence: its label and its name, or its name alone.
fn format_record_name(named: &Named) -> String {
    match &named.label {
        Some(label) => format!("`{label}` ({})", named.name),
        None => format!("`{}`", named.name),
    }
}

/// How a link joins its two records, in words.
pub fn how_linked(link: &Link) -> String {
    let mut how = match (link.kind, link.run) {
        (LinkKind::MustLink, _) => "a `must_link` constraint".to_owned(),
        (LinkKind::Version, _) => "a version of the same row".to_owned(),
        (LinkKind::Match, Some(run)) => format!("`match` in run {run}"),
        (LinkKind::Match, None) => "`match`".to_owned(),
    };
    if !link.rules.is_empty() {
        how.push_str(&format!(
            ", {} {}",
            rule_or_rules(&link.rules.iter().map(String::as_str).collect::<Vec<_>>()),
            join_quoted_names(link.rules.iter().map(String::as_str))
        ));
    } else if let Some(score) = link.score {
        how.push_str(&format!(", score {score}"));
    }
    let values: Vec<String> = link.values.iter().map(value_said).collect();
    if !values.is_empty() {
        how.push_str(&format!(": {}", values.join(", ")));
    }
    how
}

/// A feature's values at the two ends of a link: once when they are the
/// same, and both when they are not.
fn value_said(value: &LinkValue) -> String {
    let plain = |value: &serde_json::Value| match value {
        serde_json::Value::Null => "null".to_owned(),
        other => other.to_string(),
    };
    match value.from == value.to {
        true => format!("{} {}", value.feature, plain(&value.from)),
        false => format!(
            "{} {} and {}",
            value.feature,
            plain(&value.from),
            plain(&value.to)
        ),
    }
}

fn names_said(side: &Side) -> String {
    let names: Vec<String> = side
        .names
        .iter()
        .map(|name| format!("{} × `{}`", name.records, name.name))
        .collect();
    match names.is_empty() {
        true => String::new(),
        false => format!(" ({})", names.join(", ")),
    }
}

/// A number of things, as a sentence says it: `1 link`, `2 links`.
fn format_count(number: u64, what: &str) -> String {
    match number {
        1 => format!("1 {what}"),
        number => format!("{number} {what}s"),
    }
}

pub(super) fn identity_summary(explanation: &IdentityExplanation) -> Vec<String> {
    let mut said = Vec::new();
    let called = match &explanation.label {
        Some(label) => format!("`{label}`"),
        None => format!("Identity {}", explanation.current),
    };
    if explanation.current != explanation.id {
        said.push(format!(
            "Identity {} was merged into {}, which is explained here.",
            explanation.id, explanation.current
        ));
    }
    said.push(format!(
        "{called} is a `{}` identity of {} with {} different {}.",
        explanation.entity_type,
        format_count(explanation.records, "record"),
        explanation.names,
        if explanation.names == 1 {
            "name"
        } else {
            "names"
        },
    ));
    if explanation.pairs > 0 {
        said.push(format!(
            "Of its {} pairs of records, {} are linked directly and {} are together only through a chain of others{}.",
            explanation.pairs,
            explanation.linked_pairs,
            explanation.chained_pairs,
            match explanation.longest_chain {
                0 | 1 => String::new(),
                steps => format!(", the longest of at least {steps} links"),
            }
        ));
    }
    if explanation.parts > 1 {
        said.push(format!(
            "No chain of matches, constraints or versions holds all of it together: it is in {} parts. What joined them has since been removed, or a changeset placed records directly.",
            explanation.parts
        ));
    }
    for hold in &explanation.rules {
        if hold.pieces < 2 {
            continue;
        }
        let pieces: Vec<String> = hold
            .largest
            .iter()
            .map(|piece| format!("{}{}", piece.records, names_said(piece)))
            .collect();
        let more = match hold.pieces as usize - hold.largest.len() {
            0 => String::new(),
            1 => " and 1 more".to_owned(),
            count => format!(" and {count} more"),
        };
        said.push(format!(
            "Without the {} only rule `{}` made, it would fall into {} pieces: {}{more}.",
            format_count(hold.alone, "link"),
            hold.rule,
            hold.pieces,
            pieces.join(", "),
        ));
    }
    match explanation.weakest.first() {
        None if explanation.records > 1 => said.push(format!(
            "No single link holds a part of it on, and more than {MOST_CUT} links hold its two records farthest apart together."
        )),
        None => {}
        Some(_) => {
            for split in &explanation.weakest {
                let [small, large] = &split.sides[..] else {
                    continue;
                };
                said.push(format!(
                    "Removing {} would part {}{} from the other {}{}.",
                    format_count(split.links.len() as u64, "link"),
                    format_count(small.records, "record"),
                    names_said(small),
                    large.records,
                    names_said(large),
                ));
            }
        }
    }
    said
}

fn join_quoted_names<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    names
        .into_iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn find_holding_rules(results: &[RuleResult]) -> Vec<&str> {
    results
        .iter()
        .filter(|result| result.holds == Some(true))
        .map(|result| result.name.as_str())
        .collect()
}

fn rule_or_rules(names: &[&str]) -> &'static str {
    if names.len() == 1 { "rule" } else { "rules" }
}

#[expect(clippy::too_many_lines)]
pub(super) fn pair_summary(explanation: &Explanation) -> Vec<String> {
    let (left, right) = (&explanation.left.record, &explanation.right.record);
    // Two versions of one row have the same source and key.
    let (l, r) = match (left.name(), right.name()) {
        (l, r) if l == r => (left.id.to_string(), right.id.to_string()),
        names => names,
    };
    let name_of = |id: RecordId| if id == left.id { &l } else { &r };
    let mut said = Vec::new();

    match (explanation.outcome, left.identity, right.identity) {
        (Outcome::DifferentTypes, _, _) => {
            said.push(format!(
                "`{l}` is a `{}` and `{r}` is a `{}`. Records of different entity types are never compared.",
                left.entity_type, right.entity_type
            ));
        }
        (Outcome::SameIdentity, Some(identity), _) => {
            said.push(format!(
                "`{l}` and `{r}` belong to the same identity, {identity}."
            ));
        }
        (_, Some(one), Some(other)) => {
            said.push(format!(
                "`{l}` belongs to identity {one} and `{r}` to identity {other}."
            ));
        }
        (_, None, None) => said.push(format!("Neither `{l}` nor `{r}` has an identity.")),
        (_, Some(identity), None) => said.push(format!(
            "`{l}` belongs to identity {identity}. `{r}` has no identity."
        )),
        (_, None, Some(identity)) => said.push(format!(
            "`{l}` has no identity. `{r}` belongs to identity {identity}."
        )),
    }

    for record in [left, right] {
        if let Some(run) = record.retracted {
            said.push(format!(
                "`{}` was taken back by its source in run {run}: it was taken out of its identity, and no run examines it until the source sends the row again.",
                name_of(record.id)
            ));
        }
    }

    for (later, earlier) in [(right, left), (left, right)] {
        if later.supersedes == Some(earlier.id) {
            said.push(format!(
                "`{}` is a later version of `{}`: the source sent the row again under the same key with other content. Versions of one row are kept together unless a constraint says otherwise.",
                name_of(later.id),
                name_of(earlier.id)
            ));
        }
    }

    for record in &explanation.constraints {
        let kind = record.constraint.kind.as_str();
        said.push(match (record.to, record.constraint.kind) {
            (Some(to), _) => format!(
                "A `{kind}` constraint between them was in force from log position {} until position {to}.",
                record.from
            ),
            (None, ConstraintKind::MustLink) => format!(
                "A `must_link` constraint committed at log position {} says they belong together, whatever a strategy judges.",
                record.from
            ),
            (None, ConstraintKind::CannotLink) => format!(
                "A `cannot_link` constraint committed at log position {} keeps them apart, whatever a strategy judges.",
                record.from
            ),
        });
    }

    match explanation.comparisons.split_last() {
        None if explanation.outcome != Outcome::DifferentTypes => {
            said.push("No run has compared them.".to_owned());
        }
        None => {}
        Some((latest, earlier)) => {
            said.push(describe_comparison(latest));
            match earlier.len() {
                0 => {}
                1 => said.push("One earlier run compared them as well.".to_owned()),
                count => said.push(format!("{count} earlier runs compared them as well.")),
            }
        }
    }

    if explanation.outcome == Outcome::SameIdentity {
        match explanation.path.as_slice() {
            [] => said.push(
                "No chain of matches, constraints or versions joins them within the identity as it is now. Either what joined them has since been removed, or a changeset placed one of them directly."
                    .to_owned(),
            ),
            [_] => {}
            chain => {
                let mut steps = format!("`{}`", name_of(left.id));
                for link in chain {
                    let to = match link.to {
                        id if id == right.id => format!("`{r}`"),
                        _ => format_record_name(&link.to_record),
                    };
                    steps.push_str(&format!(" → {to} ({})", how_linked(link)));
                }
                let between = chain.len() - 1;
                said.push(format!(
                    "Nothing links the two directly. They are joined through {between} other {}: {steps}.",
                    if between == 1 { "member" } else { "members" }
                ));
            }
        }
        for rule in explanation
            .held_by
            .iter()
            .filter(|_| explanation.path.len() > 1)
        {
            said.push(format!(
                "Without the links only rule `{rule}` made, they would not be together."
            ));
        }
        // A link of their own parts nothing that the sentence above has
        // not said.
        match &explanation.cut {
            _ if explanation.path.len() < 2 => {}
            Some(cut) => {
                if let [with_left, with_right] = &cut.sides[..] {
                    said.push(format!(
                        "Removing {} would part them, leaving {} with `{l}`{} and {} with `{r}`{}.",
                        format_count(cut.links.len() as u64, "link"),
                        format_count(with_left.records, "record"),
                        names_said(with_left),
                        format_count(with_right.records, "record"),
                        names_said(with_right),
                    ));
                }
            }
            None => said.push(format!(
                "More than {MOST_CUT} links hold them together, so no few links part them."
            )),
        }
    }

    match (&explanation.left.placed, &explanation.right.placed) {
        (Some(one), Some(other)) if one.seq == other.seq => {
            said.push(format!("Both were placed {}.", describe_placement(one)));
        }
        (one, other) => {
            for (name, placement) in [(&l, one), (&r, other)] {
                if let Some(placement) = placement {
                    said.push(format!(
                        "`{name}` was placed {}.",
                        describe_placement(placement)
                    ));
                }
            }
        }
    }

    for item in &explanation.review_items {
        let named: Vec<&str> = [left.id, right.id]
            .into_iter()
            .filter(|id| item.records.contains(id))
            .map(|id| name_of(id).as_str())
            .collect();
        let named = join_quoted_names(named);
        let kind = item.kind.as_str();
        // In a `possible_split`, whether the two are in different groups:
        // then the item asks about this very pair.
        let group_of = |id| item.groups.iter().position(|group| group.contains(&id));
        let asked_about = matches!(
            (group_of(left.id), group_of(right.id)),
            (Some(one), Some(other)) if one != other
        );
        match (item.status, &item.dismissed) {
            (ReviewStatus::Open, _) => said.push(format!(
                "Review item {} (`{kind}`) is open and names {named}: {}",
                item.id,
                match item.kind {
                    ReviewKind::PossibleMerge =>
                        "what it matched belongs to more than one identity, and merging identities is left to a reviewer.",
                    ReviewKind::Conflict =>
                        "the evidence about it contradicts itself, which is left to a reviewer.",
                    ReviewKind::PossibleSplit if asked_about =>
                        "they share an identity, and what held them together went through a record that its source took back. Nothing the store knows says they are one thing now. Whether to split the identity is left to a reviewer.",
                    ReviewKind::PossibleSplit =>
                        "the identity lost a record that its source took back, and nothing the store knows holds all of the records that are left together. Whether to split the identity is left to a reviewer.",
                }
            )),
            (ReviewStatus::Dismissed, Some(dismissal))
                if item.kind == ReviewKind::PossibleSplit =>
            {
                said.push(format!(
                    "Review item {} (`{kind}`), which names {named}, was dismissed by `{}`: it asked whether to split their identity, and the reviewer left it as it is.",
                    item.id, dismissal.by
                ));
            }
            (ReviewStatus::Dismissed, Some(dismissal)) => said.push(format!(
                "Review item {} (`{kind}`), which names {named}, was dismissed by `{}`. No run looks at a dismissed item's records again.",
                item.id, dismissal.by
            )),
            _ => {}
        }
    }

    if let Some(rules) = &explanation.rules {
        said.push(describe_rule_reading(rules));
        if let Some(run) = read_otherwise(&explanation.comparisons, rules) {
            said.push(format!(
                "That is not what run {run} recorded, so the rules or the features have changed since."
            ));
        }
        match (explanation.outcome, rules.verdict) {
            (Outcome::DifferentIdentities, Some(Verdict::Match)) => said.push(
                "Both already have an identity, and a run compares a pair only when one of the two is new to it. A rematch compares every pair and lists a pair like this one as a suggested merge."
                    .to_owned(),
            ),
            (Outcome::SameIdentity, Some(Verdict::NoMatch)) => said.push(
                "No run takes an identity apart. A rematch lists a pair like this one as a suggested split."
                    .to_owned(),
            ),
            _ => {}
        }
    }
    said
}

fn describe_placement(placement: &Placement) -> String {
    let mut details = Vec::new();
    if let Some(run) = placement.run {
        details.push(format!("reconcile run {run}"));
    }
    if let Some(reason) = &placement.reason {
        details.push(format!("\"{reason}\""));
    }
    let details = match details.is_empty() {
        true => String::new(),
        false => format!(" ({})", details.join(", ")),
    };
    format!(
        "at log position {} by `{}`{details}",
        placement.seq, placement.actor
    )
}

fn describe_comparison(comparison: &Comparison) -> String {
    let strategy = match &comparison.strategy {
        Some(strategy) => format!(" (strategy `{}`)", strategy.name),
        None => String::new(),
    };
    let mut text = match comparison.judgements.as_slice() {
        [] => format!(
            "Run {}{strategy} made them a candidate pair (block {} {}) and gave no judgement.",
            comparison.run,
            rule_or_rules(
                &comparison
                    .blocked_by
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            ),
            join_quoted_names(comparison.blocked_by.iter().map(String::as_str)),
        ),
        judgements => {
            let verdicts: Vec<String> = judgements
                .iter()
                .map(|judgement| {
                    let mut verdict = format!("`{}`", judgement.verdict.as_str());
                    if let Some(score) = judgement.score {
                        verdict.push_str(&format!(" with score {score}"));
                    }
                    let held = |group: &str| -> Vec<&str> {
                        judgement
                            .detail
                            .as_ref()
                            .and_then(|detail| detail.get(group))
                            .and_then(serde_json::Value::as_array)
                            .map(|names| {
                                names.iter().filter_map(serde_json::Value::as_str).collect()
                            })
                            .unwrap_or_default()
                    };
                    let group = match judgement.verdict {
                        Verdict::Match => "match",
                        Verdict::NoMatch => "no_match",
                        Verdict::Unsure => "",
                    };
                    let names = held(group);
                    if !names.is_empty() {
                        verdict.push_str(&format!(
                            " ({group} {} {} held)",
                            rule_or_rules(&names),
                            join_quoted_names(names.iter().copied())
                        ));
                    }
                    verdict
                })
                .collect();
            format!(
                "Run {}{strategy} judged them {}.",
                comparison.run,
                verdicts.join(" and ")
            )
        }
    };
    match (comparison.status, comparison.draft) {
        (RunStatus::Failed, _) => text.push_str(" The run failed, so nothing came of it."),
        (RunStatus::Open | RunStatus::Running, _) => {
            text.push_str(" The run has not been reconciled.");
        }
        (_, Some(DraftStatus::Open)) => text.push_str(" Its draft waits for a decision."),
        (_, Some(DraftStatus::Rejected)) => text.push_str(" Its draft was rejected."),
        (_, Some(DraftStatus::Superseded)) => {
            text.push_str(" Its draft was superseded by a later run before it was committed.");
        }
        _ => {}
    }
    text
}

/// The latest run of the rules whose record of the pair is not what the
/// rules say of it now.
fn read_otherwise(comparisons: &[Comparison], rules: &RuleReading) -> Option<RunId> {
    let recorded = comparisons.iter().rev().find(|comparison| {
        matches!(comparison.status, RunStatus::Succeeded | RunStatus::Drafted)
            && comparison
                .strategy
                .as_ref()
                .is_some_and(|strategy| strategy.name == StrategyRef::RULES)
    })?;
    let judged = recorded
        .judgements
        .last()
        .map(|judgement| judgement.verdict);
    (!rules.candidate || judged != rules.verdict).then_some(recorded.run)
}

fn describe_rule_reading(rules: &RuleReading) -> String {
    let block = find_holding_rules(&rules.block);
    if rules.block.is_empty() {
        return "The entity type has no block rule, so a run of the rules would not compare them."
            .to_owned();
    }
    if block.is_empty() {
        let undecided: Vec<&str> = rules
            .block
            .iter()
            .filter(|result| result.holds.is_none())
            .map(|result| result.name.as_str())
            .collect();
        let mut text =
            "By the rules as they are now, no block rule holds for the pair, so a run would not compare them."
                .to_owned();
        if !undecided.is_empty() {
            text.push_str(&format!(
                " Block {} {} cannot be decided, because a feature it reads is null.",
                rule_or_rules(&undecided),
                join_quoted_names(undecided.iter().copied())
            ));
        }
        return text;
    }
    let found = format!(
        "By the rules as they are now, block {} {} {} them a candidate pair",
        rule_or_rules(&block),
        join_quoted_names(block.iter().copied()),
        if block.len() == 1 { "makes" } else { "make" }
    );
    let decided = |group: &str, results: &[RuleResult]| {
        let names = find_holding_rules(results);
        format!(
            "{found} and {group} {} {} {}, so they would be judged `{group}`.",
            rule_or_rules(&names),
            join_quoted_names(names.iter().copied()),
            if names.len() == 1 { "holds" } else { "hold" }
        )
    };
    if let Some(failed) = &rules.failed {
        return format!(
            "{found}, and the `no_match` and `match` rules cannot be applied to the pair's values, so a run of the rules that compared them would fail ({failed})."
        );
    }
    match rules.verdict {
        Some(Verdict::NoMatch) => decided("no_match", &rules.no_match),
        Some(Verdict::Match) => decided("match", &rules.match_),
        _ => format!(
            "{found} and no `no_match` or `match` rule holds, so they would get no judgement."
        ),
    }
}
