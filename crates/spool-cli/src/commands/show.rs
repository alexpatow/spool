//! `spool show` and `spool explain`: what a running server holds, and why
//! two records share an identity or do not.
//!
//! Both read through the server's API and print what it answered. With
//! `--json` they print the answer itself. Nothing here decides anything: a
//! sentence that says why comes from the engine.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::{Value, json};
use spool::core::{ChangesetId, ConstraintEntry, DocumentId, IdentityId, Op, RecordId, ReviewItem};
use spool::{
    DocumentEntry, DraftEntry, DraftSummary, Explanation, HistoryEntry, IdentitySummary,
    IdentityView, LoggedChangeset, Member, RecordEntry, ReviewEntry, ReviewKind, ReviewStatus,
    RuleResult, RunEntry,
};
use spool_cli::api::{DocumentList, Info, RecordList};

use super::remote::{self, Remote, print_json, typed};
use crate::Failure;
use crate::args::{Arguments, Spec};

pub const SHOW_USAGE: &str = "\
Usage: spool show [<what> [<id>]] [options]

Shows what a running server holds. Without <what>: its entity types and how
much there is of each.

  runs [<run>]                   the latest runs, or one run and what it reported
  identities [<identity>]        identities, or one with its members, the
                                 constraints on them and its history
  records [<record>]             records by source and key, or one with its
                                 payload, its evidence and, for a mention, the
                                 place in the document it was found in
  documents [<document>]         documents, or one with its text
  review [<item>]                review items, or one with its records
  drafts [<draft>]               drafts, or one with the changeset it would commit
  log                            the changeset log

A record is named by its ID or as <source>:<key>, which names the
latest version of that row. A record found in a document is named as the
lists name it, <source>:<document>[<start>-<end>].

Options:
  --type <name>      only this entity type (runs, identities, records,
                     review, drafts)
  --status <status>  only this status (runs, review, drafts)
  --kind <kind>      only runs of this kind: load, extract, match, reconcile,
                     retract
  --source <name>    only this source (records, documents)
  --key <key>        only this key within its source (records, documents)
  --document <id>    only the mentions found in this document (records)
  --unassigned       only records that wait for an identity: they have
                     none, and their source has not taken them back
  --latest           only the version of each document that nothing replaced
  --ids              list each record's ID as well (records)
  --after <id>       continue a list after this ID, or the log after this
                     position
  --limit <n>        list at most this many
  --url <url>        the server (default: $SPOOL_URL, or http://127.0.0.1:7765)
  --token <token>    a token of that server (default: $SPOOL_TOKEN); `spool dev`
                     asks for none
  --json             print what the server answered, as JSON
";

pub const EXPLAIN_USAGE: &str = "\
Usage: spool explain <record> <record> [options]

Says why two records share an identity, or why they do not: where each
is and what placed it there, what each run that compared them concluded, the
constraints and review items that name them, and what the project's rules
say about the pair as it is now.

A record is named by its ID or as <source>:<key>, which names the
latest version of that row. A record found in a document is named as the
lists name it, <source>:<document>[<start>-<end>].

Options:
  --url <url>        the server (default: $SPOOL_URL, or http://127.0.0.1:7765)
  --token <token>    a token of that server (default: $SPOOL_TOKEN); `spool dev`
                     asks for none
  --json             print what the server answered, as JSON
";

/// The options that narrow a list, each with the query parameter it becomes
/// and whether it takes a value.
const FILTERS: [(&str, &str, bool); 10] = [
    ("--type", "entity_type", true),
    ("--status", "status", true),
    ("--kind", "kind", true),
    ("--source", "source", true),
    ("--key", "key", true),
    ("--document", "document", true),
    ("--after", "after", true),
    ("--limit", "limit", true),
    ("--unassigned", "unassigned", false),
    ("--latest", "latest", false),
];

/// How much of a document is shown on either side of a mention, in
/// characters.
const CONTEXT: usize = 80;

pub fn show(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &[
                "--type",
                "--status",
                "--kind",
                "--source",
                "--key",
                "--document",
                "--after",
                "--limit",
                remote::VALUES[0],
                remote::VALUES[1],
            ],
            flags: &["--unassigned", "--latest", "--ids", "--json"],
            positional: 2,
        },
    )?;
    let remote = Remote::new(&arguments);
    let view = View {
        remote: &remote,
        arguments: &arguments,
        json: arguments.flag("--json"),
    };
    let what = arguments.positional.first().map(String::as_str);
    let id = arguments.positional.get(1).map(String::as_str);
    if arguments.flag("--ids") && !matches!((what, id), (Some("record" | "records"), None)) {
        return Err(Failure::Usage(
            "`--ids` goes with the list of records".to_owned(),
        ));
    }
    match (what, id) {
        (None, _) => view.overview(),
        (Some("run" | "runs"), None) => view.runs(),
        (Some("run" | "runs"), Some(id)) => view.run(id),
        (Some("identity" | "identities"), None) => view.identities(),
        (Some("identity" | "identities"), Some(id)) => view.identity(id),
        (Some("record" | "records"), None) => view.records(),
        (Some("record" | "records"), Some(id)) => view.record(id),
        (Some("document" | "documents"), None) => view.documents(),
        (Some("document" | "documents"), Some(id)) => view.document(id),
        (Some("review" | "reviews"), None) => view.review_items(),
        (Some("review" | "reviews"), Some(id)) => view.review_item(id),
        (Some("draft" | "drafts"), None) => view.drafts(),
        (Some("draft" | "drafts"), Some(id)) => view.draft(id),
        (Some("log"), None) => view.log(),
        (Some("log"), Some(id)) => Err(Failure::Usage(format!("unexpected argument `{id}`"))),
        (Some(other), _) => Err(Failure::Usage(format!(
            "`{other}` is not something `show` shows"
        ))),
    }
}

pub fn explain(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &remote::VALUES,
            flags: &["--json"],
            positional: 2,
        },
    )?;
    let [a, b] = &arguments.positional[..] else {
        return Err(Failure::Usage("`explain` needs two records".to_owned()));
    };
    let remote = Remote::new(&arguments);
    let a = find_record(&remote, a)?;
    let b = find_record(&remote, b)?;
    let answer = remote.list(
        "/explain",
        &[("a", a.id.to_string()), ("b", b.id.to_string())],
    )?;
    if arguments.flag("--json") {
        print_json(&answer);
    } else {
        print_explanation(&typed(&answer)?);
    }
    Ok(())
}

/// The record a reference names: an ID, or `<source>:<key>`, which
/// names the version of the row that nothing has replaced, or a mention as
/// the lists name it, `<source>:<document>[<start>-<end>]`.
fn find_record(remote: &Remote, reference: &str) -> Result<RecordEntry, Failure> {
    if reference.parse::<RecordId>().is_ok() {
        let found = remote.get(&format!("/records/{reference}"), &[], || {
            format!("there is no record {reference}")
        })?;
        return typed(&found);
    }
    let Some((source, key)) = reference.split_once(':') else {
        return Err(Failure::Usage(format!(
            "`{reference}` does not name a record: give its ID, or <source>:<key>"
        )));
    };
    // Versions are listed oldest first, in pages. The latest is the one no
    // other replaced.
    let mut query = vec![("source", source.to_owned()), ("key", key.to_owned())];
    let mut last = None;
    loop {
        let found = remote.list("/records", &query)?;
        let page: RecordList = typed(&found)?;
        for version in page.records {
            if version.superseded_by.is_none() {
                return Ok(version);
            }
            last = Some(version);
        }
        match page.next {
            Some(next) => {
                query.truncate(2);
                query.push(("after", next.to_string()));
            }
            None => break,
        }
    }
    if let Some(last) = last {
        return Ok(last);
    }
    if let Some(mention) = find_mention(remote, reference, source, key)? {
        return Ok(mention);
    }
    Err(Failure::Failed(format!(
        "there is no record with key `{key}` from source `{source}`"
    )))
}

/// The record found at a place in a document, named as
/// [`RecordEntry::name`] names it. Of the versions of the document, it is
/// the one in the latest that has such a record.
fn find_mention(
    remote: &Remote,
    reference: &str,
    source: &str,
    place: &str,
) -> Result<Option<RecordEntry>, Failure> {
    let Some((document, span)) = place
        .strip_suffix(']')
        .and_then(|place| place.rsplit_once('['))
    else {
        return Ok(None);
    };
    let is_span = span
        .split_once('-')
        .is_some_and(|(start, end)| start.parse::<u64>().is_ok() && end.parse::<u64>().is_ok());
    if !is_span {
        return Ok(None);
    }
    let mut versions: Vec<DocumentEntry> = Vec::new();
    let mut query = vec![("source", source.to_owned()), ("key", document.to_owned())];
    loop {
        let page: DocumentList = typed(&remote.list("/documents", &query)?)?;
        versions.extend(page.documents);
        let Some(next) = page.next else { break };
        query.truncate(2);
        query.push(("after", next.to_string()));
    }
    for version in newest_first(versions) {
        let mut found = Vec::new();
        let mut query = vec![("document", version.id.to_string())];
        loop {
            let page: RecordList = typed(&remote.list("/records", &query)?)?;
            found.extend(
                page.records
                    .into_iter()
                    .filter(|record| record.name() == reference),
            );
            let Some(next) = page.next else { break };
            query.truncate(1);
            query.push(("after", next.to_string()));
        }
        // A mention found again with other values is a version of the same
        // record, at the same place: the name is the one no other replaced.
        let ids: Vec<RecordId> = found.iter().map(|record| record.id).collect();
        found.retain(|record| {
            record
                .superseded_by
                .is_none_or(|later| !ids.contains(&later))
        });
        match found.len() {
            0 => {}
            1 => return Ok(found.pop()),
            several => {
                let ids: Vec<String> = found.iter().map(|record| record.id.to_string()).collect();
                return Err(Failure::Failed(format!(
                    "{several} records are named `{reference}`: name one by its ID, of {}",
                    ids.join(", ")
                )));
            }
        }
    }
    Ok(None)
}

/// The versions of one document, the one nothing replaced first and then
/// the one each replaced. A list of documents is in the order of their
/// IDs, which are hashes of their text and say nothing of which is newer.
fn newest_first(versions: Vec<DocumentEntry>) -> Vec<DocumentEntry> {
    let mut left: BTreeMap<DocumentId, DocumentEntry> = versions
        .into_iter()
        .map(|version| (version.id, version))
        .collect();
    let mut ordered = Vec::with_capacity(left.len());
    let mut next = left
        .values()
        .find(|version| version.superseded_by.is_none())
        .map(|version| version.id);
    while let Some(version) = next.and_then(|id| left.remove(&id)) {
        next = version.supersedes;
        ordered.push(version);
    }
    // A chain that is whole leaves none.
    ordered.extend(left.into_values());
    ordered
}

struct View<'a> {
    remote: &'a Remote,
    arguments: &'a Arguments,
    json: bool,
}

impl View<'_> {
    /// The query of a list, from the options that narrow it. An option that
    /// says nothing about `what` is a mistake, not something to ignore.
    fn query(&self, what: &str, allowed: &[&str]) -> Result<Vec<(&'static str, String)>, Failure> {
        let mut query = Vec::new();
        for (option, parameter, takes_value) in FILTERS {
            let value = match takes_value {
                true => self.arguments.value(option).map(str::to_owned),
                false => self.arguments.flag(option).then(|| "true".to_owned()),
            };
            let Some(value) = value else { continue };
            if !allowed.contains(&option) {
                return Err(Failure::Usage(format!(
                    "`{option}` does not go with `{what}`"
                )));
            }
            query.push((parameter, value));
        }
        self.arguments.number("--limit").map_err(Failure::Usage)?;
        Ok(query)
    }

    /// A single thing takes no options that narrow a list.
    fn one(&self, what: &str) -> Result<(), Failure> {
        self.query(what, &[]).map(|_| ())
    }

    fn overview(&self) -> Result<(), Failure> {
        self.one("the overview")?;
        let answer = self.remote.list("/info", &[])?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let info: Info = typed(&answer)?;
        let store = &info.store;
        fields(&[
            ("server", self.remote.url.clone()),
            (
                "api",
                format!(
                    "{}, engine {}, store format {}",
                    info.api, store.engine_version, store.format_version
                ),
            ),
            (
                "manifest",
                store
                    .manifest_hash
                    .clone()
                    .unwrap_or_else(|| "none applied".to_owned()),
            ),
            ("head", format!("log position {}", store.head)),
            ("documents", store.documents.to_string()),
        ]);
        for entity_type in &store.entity_types {
            println!("\nentity type `{}`", entity_type.name);
            let mut rows = vec![
                (
                    "records",
                    match entity_type.retracted {
                        0 => format!(
                            "{}, {} without an identity",
                            entity_type.records, entity_type.unassigned
                        ),
                        retracted => format!(
                            "{}, {} without an identity, {retracted} taken back by their source",
                            entity_type.records, entity_type.unassigned
                        ),
                    },
                ),
                ("identities", entity_type.identities.to_string()),
                (
                    "review",
                    format!("{} open", count(entity_type.open_review_items, "item")),
                ),
            ];
            if let Some(draft) = entity_type.open_draft {
                rows.push(("draft", format!("{draft} waits for a decision")));
            }
            if !entity_type.strategies.is_empty() {
                rows.push((
                    "strategies",
                    entity_type
                        .strategies
                        .iter()
                        .map(|strategy| format!("{} ({})", strategy.name, strategy.runtime))
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            indented(&rows);
        }
        if !store.extractors.is_empty() {
            println!();
        }
        for extractor in &store.extractors {
            println!(
                "extractor `{}` ({}, {})",
                extractor.name, extractor.runtime, extractor.file
            );
        }
        Ok(())
    }

    fn runs(&self) -> Result<(), Failure> {
        let query = self.query("runs", &["--type", "--status", "--kind", "--limit"])?;
        let answer = self.remote.list("/runs", &query)?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let runs: Vec<RunEntry> = typed(&answer["runs"])?;
        table(
            "runs",
            &["run", "kind", "status", "about", "started by", "started"],
            runs.iter()
                .map(|run| {
                    let about = [
                        run.entity_type.as_ref().map(ToString::to_string),
                        run.strategy.as_ref().map(|strategy| strategy.name.clone()),
                        run.extractor
                            .as_ref()
                            .map(|extractor| extractor.name.clone()),
                    ];
                    vec![
                        run.run.to_string(),
                        word(&run.kind),
                        run.status.to_string(),
                        about.into_iter().flatten().collect::<Vec<_>>().join(", "),
                        run.started_by.to_string(),
                        when(&run.started_at),
                    ]
                })
                .collect(),
        );
        Ok(())
    }

    fn run(&self, id: &str) -> Result<(), Failure> {
        self.one("a run")?;
        let run: u64 = id
            .parse()
            .map_err(|_| Failure::Usage(format!("`{id}` is not a run ID")))?;
        let answer = self.remote.get(&format!("/runs/{run}"), &[], || {
            format!("there is no run {run}")
        })?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let run: RunEntry = typed(&answer["run"])?;
        let mut rows = vec![
            ("run", run.run.to_string()),
            ("kind", word(&run.kind)),
            ("status", run.status.to_string()),
        ];
        if let Some(entity_type) = &run.entity_type {
            rows.push(("type", entity_type.to_string()));
        }
        if let Some(strategy) = &run.strategy {
            rows.push((
                "strategy",
                versioned(&strategy.name, strategy.version.as_deref()),
            ));
        }
        if let Some(extractor) = &run.extractor {
            rows.push((
                "extractor",
                versioned(&extractor.name, Some(&extractor.version)),
            ));
        }
        rows.push(("started by", run.started_by.to_string()));
        rows.push(("started", when(&run.started_at)));
        if let Some(finished) = &run.finished_at {
            rows.push(("finished", when(finished)));
        }
        if let Some(error) = &run.error {
            rows.push(("error", error.clone()));
        }
        if let Some(report) = &run.load {
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
                rows.push((
                    "committed",
                    format!(
                        "log position {}, changeset {}",
                        committed.seq, committed.changeset
                    ),
                ));
            }
        }
        if let Some(report) = &run.retraction {
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
                rows.push((
                    "committed",
                    format!(
                        "log position {}, changeset {}",
                        committed.seq, committed.changeset
                    ),
                ));
            }
        }
        if let Some(report) = &run.extraction {
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
        let mut suggestions = Vec::new();
        if let Some(report) = &run.resolution {
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
            rows.push(("placed", placed(report)));
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
                (Some(committed), _) => rows.push((
                    "committed",
                    format!(
                        "log position {}, changeset {}",
                        committed.seq, committed.changeset
                    ),
                )),
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
        }
        fields(&rows);
        if !suggestions.is_empty() {
            println!();
        }
        for line in suggestions {
            println!("{line}");
        }
        Ok(())
    }

    fn identities(&self) -> Result<(), Failure> {
        let query = self.query("identities", &["--type", "--after", "--limit"])?;
        let answer = self.remote.list("/identities", &query)?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let identities: Vec<IdentitySummary> = typed(&answer["identities"])?;
        // The type is said once, by the option that chose it or by there
        // being one, and per row only when the rows differ.
        let typed_rows = mixed(identities.iter().map(|identity| &identity.entity_type));
        let mut headers = vec!["identity"];
        if typed_rows {
            headers.push("type");
        }
        headers.extend(["name", "members", "created at"]);
        table(
            "identities",
            &headers,
            identities
                .iter()
                .map(|identity| {
                    let mut row = vec![identity.id.to_string()];
                    if typed_rows {
                        row.push(identity.entity_type.to_string());
                    }
                    row.extend([
                        label(identity.label.as_deref()),
                        identity.members.to_string(),
                        format!("log position {}", identity.created),
                    ]);
                    row
                })
                .collect(),
        );
        more(&answer);
        Ok(())
    }

    fn identity(&self, id: &str) -> Result<(), Failure> {
        self.one("an identity")?;
        let identity: IdentityId = id
            .parse()
            .map_err(|_| Failure::Usage(format!("`{id}` is not an identity ID")))?;
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
        if self.json {
            print_json(&json!({
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
                            None => named(&member.source, member.key.as_deref()),
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
                            word(&record.constraint.kind),
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
                when(&entry.committed_at),
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
                println!("      {}", describe(op));
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

    fn records(&self) -> Result<(), Failure> {
        let query = self.query(
            "records",
            &[
                "--type",
                "--source",
                "--key",
                "--document",
                "--unassigned",
                "--after",
                "--limit",
            ],
        )?;
        let answer = self.remote.list("/records", &query)?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let records: Vec<RecordEntry> = typed(&answer["records"])?;
        if records.is_empty() && query.is_empty() {
            println!("no records: `spool load <file>` sends rows");
            return Ok(());
        }
        let ids = self.arguments.flag("--ids");
        let typed_rows = mixed(records.iter().map(|record| &record.entity_type));
        let mut headers = Vec::new();
        if ids {
            headers.push("record");
        }
        if typed_rows {
            headers.push("type");
        }
        headers.extend(["source and key", "name", "identity"]);
        table(
            "records",
            &headers,
            records
                .iter()
                .map(|record| {
                    let mut row = Vec::new();
                    if ids {
                        row.push(record.id.to_string());
                    }
                    if typed_rows {
                        row.push(record.entity_type.to_string());
                    }
                    row.extend([
                        record_name(record),
                        label(record.label.as_deref()),
                        record
                            .identity
                            .map_or_else(|| "-".to_owned(), |identity| identity.to_string()),
                    ]);
                    row
                })
                .collect(),
        );
        more(&answer);
        // A `-` in the last column says nothing about why, or what to do.
        let waiting = records
            .iter()
            .filter(|record| record.identity.is_none() && record.retracted.is_none())
            .count() as u64;
        if waiting > 0 {
            println!(
                "\n{} no identity. `spool match` places new records in identities,\n\
                 and `spool show review` lists the ones a match left out.",
                match waiting {
                    1 => "1 record has".to_owned(),
                    several => format!("{several} records have"),
                }
            );
        }
        Ok(())
    }

    fn record(&self, reference: &str) -> Result<(), Failure> {
        self.one("a record")?;
        let record = find_record(self.remote, reference)?;
        let review = self
            .remote
            .list("/review", &[("record", record.id.to_string())])?;
        let mention = self.mention(&record)?;
        if self.json {
            print_json(&json!({
                "record": record,
                "mention": mention,
                "review_items": review["items"],
            }));
            return Ok(());
        }
        let review: Vec<ReviewEntry> = typed(&review["items"])?;

        let mut rows = vec![
            ("record", record.id.to_string()),
            ("type", record.entity_type.to_string()),
            ("source", record.source.clone()),
        ];
        if let Some(key) = &record.key {
            rows.push(("key", key.clone()));
        }
        rows.push((
            "loaded",
            format!("{} by run {}", when(&record.loaded_at), record.load_run),
        ));
        if !record.found_by.is_empty() {
            rows.push((
                "found by",
                record
                    .found_by
                    .iter()
                    .map(|found| {
                        format!(
                            "{} version {} in run {}",
                            found.extractor,
                            short_hash(&found.version),
                            found.run
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        let found = !record.found_by.is_empty();
        rows.push((
            "identity",
            match (record.identity, record.retracted) {
                (Some(identity), _) => identity.to_string(),
                (None, Some(run)) if found => {
                    format!("none: taken back in run {run}, as no extractor finds it any more")
                }
                (None, Some(run)) => format!("none: its source took it back in run {run}"),
                (None, None) => "none".to_owned(),
            },
        ));
        if let Some(earlier) = record.supersedes {
            rows.push(("replaces", earlier.to_string()));
        }
        if let Some(later) = record.superseded_by {
            rows.push(("replaced by", later.to_string()));
        }
        for item in &review {
            rows.push((
                "review",
                format!(
                    "item {} ({}, {})",
                    item.id,
                    word(&item.kind),
                    word(&item.status)
                ),
            ));
        }
        fields(&rows);

        println!("\npayload");
        object(&record.payload);
        if let Some(evidence) = &record.evidence {
            println!("\nevidence");
            object(evidence);
        }
        if let Some(mention) = mention {
            println!(
                "\nin document {}, characters {} to {}",
                named(mention.source.as_str(), Some(mention.key.as_str())),
                mention.start,
                mention.end
            );
            println!(
                "  {}{}[{}]{}{}",
                if mention.cut_before { "…" } else { "" },
                mention.before,
                mention.text,
                mention.after,
                if mention.cut_after { "…" } else { "" },
            );
        }
        Ok(())
    }

    /// Where a mention was found: the text around it in its document. `None`
    /// for a record that is not a mention.
    fn mention(&self, record: &RecordEntry) -> Result<Option<Mention>, Failure> {
        let Some(evidence) = &record.evidence else {
            return Ok(None);
        };
        let (Some(document), Some(start), Some(end)) = (
            evidence["document"]
                .as_str()
                .and_then(|id| id.parse::<DocumentId>().ok()),
            evidence["start"].as_u64(),
            evidence["end"].as_u64(),
        ) else {
            return Ok(None);
        };
        let Some(found) = self.remote.find(&format!("/documents/{document}"), &[])? else {
            return Ok(None);
        };
        let entry: DocumentEntry = typed(&found)?;
        let text: Vec<char> = entry.text.unwrap_or_default().chars().collect();
        let (start, end) = (start as usize, end as usize);
        if start > end || end > text.len() {
            return Ok(None);
        }
        let from = start.saturating_sub(CONTEXT);
        let to = (end + CONTEXT).min(text.len());
        let line = |characters: &[char]| -> String {
            characters
                .iter()
                .map(|character| match character {
                    '\n' | '\r' | '\t' => ' ',
                    other => *other,
                })
                .collect()
        };
        Ok(Some(Mention {
            document,
            source: entry.source,
            key: entry.key,
            start: start as u64,
            end: end as u64,
            before: line(&text[from..start]),
            text: line(&text[start..end]),
            after: line(&text[end..to]),
            cut_before: from > 0,
            cut_after: to < text.len(),
        }))
    }

    fn documents(&self) -> Result<(), Failure> {
        let query = self.query(
            "documents",
            &["--source", "--key", "--latest", "--after", "--limit"],
        )?;
        let answer = self.remote.list("/documents", &query)?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let documents: Vec<DocumentEntry> = typed(&answer["documents"])?;
        table(
            "documents",
            &["document", "source and key", "characters", "read by"],
            documents
                .iter()
                .map(|document| {
                    vec![
                        document.id.to_string(),
                        named(&document.source, Some(&document.key)),
                        document.length.to_string(),
                        read_by(document),
                    ]
                })
                .collect(),
        );
        more(&answer);
        Ok(())
    }

    fn document(&self, id: &str) -> Result<(), Failure> {
        self.one("a document")?;
        let document: DocumentId = id
            .parse()
            .map_err(|_| Failure::Usage(format!("`{id}` is not a document ID")))?;
        let answer = self
            .remote
            .get(&format!("/documents/{document}"), &[], || {
                format!("there is no document {document}")
            })?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let document: DocumentEntry = typed(&answer)?;
        let mut rows = vec![
            ("document", document.id.to_string()),
            ("source", document.source.clone()),
            ("key", document.key.clone()),
        ];
        if let Some(uri) = &document.uri {
            rows.push(("uri", uri.clone()));
        }
        rows.push(("characters", document.length.to_string()));
        rows.push((
            "loaded",
            format!("{} by run {}", when(&document.loaded_at), document.load_run),
        ));
        if let Some(earlier) = document.supersedes {
            rows.push(("replaces", earlier.to_string()));
        }
        if let Some(later) = document.superseded_by {
            rows.push(("replaced by", later.to_string()));
        }
        rows.push(("read by", read_by(&document)));
        fields(&rows);
        println!("\n{}", document.text.unwrap_or_default().trim_end());
        Ok(())
    }

    fn review_items(&self) -> Result<(), Failure> {
        let query = self.query("review", &["--type", "--status"])?;
        let answer = self.remote.list("/review", &query)?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let items: Vec<ReviewEntry> = typed(&answer["items"])?;
        table(
            "review items",
            &["item", "kind", "status", "type", "raised by", "about"],
            items
                .iter()
                .map(|item| {
                    vec![
                        item.id.to_string(),
                        word(&item.kind),
                        word(&item.status),
                        item.entity_type.to_string(),
                        format!("run {}", item.run),
                        about(item),
                    ]
                })
                .collect(),
        );
        Ok(())
    }

    fn review_item(&self, id: &str) -> Result<(), Failure> {
        self.one("a review item")?;
        let item: u64 = id
            .parse()
            .map_err(|_| Failure::Usage(format!("`{id}` is not a review item ID")))?;
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
        if self.json {
            let mut bundle = json!({ "item": answer, "records": records });
            if split {
                bundle["taken_back"] = taken_back;
            }
            print_json(&bundle);
            return Ok(());
        }
        let records: Vec<RecordEntry> = typed(&Value::Array(records))?;

        let mut rows = vec![
            ("review item", item.id.to_string()),
            ("kind", word(&item.kind)),
            ("status", word(&item.status)),
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
            let mut by = format!("{} at {}", dismissal.by, when(&dismissal.at));
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

    fn drafts(&self) -> Result<(), Failure> {
        let query = self.query("drafts", &["--type", "--status"])?;
        let answer = self.remote.list("/drafts", &query)?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let drafts: Vec<DraftSummary> = typed(&answer["drafts"])?;
        table(
            "drafts",
            &["draft", "type", "status", "operations", "from"],
            drafts
                .iter()
                .map(|draft| {
                    vec![
                        draft.id.to_string(),
                        draft.entity_type.to_string(),
                        draft.status.to_string(),
                        draft.operations.to_string(),
                        format!("match run {}", draft.match_run),
                    ]
                })
                .collect(),
        );
        Ok(())
    }

    fn draft(&self, id: &str) -> Result<(), Failure> {
        self.one("a draft")?;
        let draft: ChangesetId = id
            .parse()
            .map_err(|_| Failure::Usage(format!("`{id}` is not a draft ID")))?;
        let answer = self.remote.get(&format!("/drafts/{draft}"), &[], || {
            format!("there is no draft {draft}")
        })?;
        if self.json {
            print_json(&answer);
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
            let mut rejected = when(at);
            if let Some(reason) = &draft.rejected_reason {
                rejected.push_str(&format!(", \"{reason}\""));
            }
            rows.push(("rejected", rejected));
        }
        fields(&rows);
        println!("\noperations");
        for op in &entry.changeset.ops {
            println!("  {}", describe(op));
        }
        Ok(())
    }

    fn log(&self) -> Result<(), Failure> {
        let query = self.query("log", &["--after", "--limit"])?;
        let answer = self.remote.list("/changesets", &query)?;
        if self.json {
            print_json(&answer);
            return Ok(());
        }
        let entries: Vec<LoggedChangeset> = typed(&answer["changesets"])?;
        // The actor decided the operations. Who asked for the run that
        // committed them is beside it, where a run did.
        table(
            "changesets",
            &[
                "position",
                "actor",
                "operations",
                "from",
                "started by",
                "reason",
            ],
            entries
                .iter()
                .map(|logged| {
                    let changeset = &logged.entry.changeset;
                    vec![
                        logged.entry.seq.to_string(),
                        changeset.actor.to_string(),
                        changeset.ops.len().to_string(),
                        changeset
                            .run
                            .map_or_else(|| "-".to_owned(), |run| format!("reconcile run {run}")),
                        logged
                            .started_by
                            .as_ref()
                            .map_or_else(|| "-".to_owned(), ToString::to_string),
                        changeset.reason.clone().unwrap_or_else(|| "-".to_owned()),
                    ]
                })
                .collect(),
        );
        if let (Some(last), Some(head)) = (entries.last(), answer["head"].as_u64())
            && last.entry.seq.0 < head
        {
            println!("\nmore: add `--after {}`", last.entry.seq);
        }
        Ok(())
    }
}

/// The text around a mention in its document.
#[derive(Serialize)]
struct Mention {
    document: DocumentId,
    source: String,
    key: String,
    start: u64,
    end: u64,
    before: String,
    text: String,
    after: String,
    /// Whether the document goes on before `before` and after `after`.
    cut_before: bool,
    cut_after: bool,
}

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
    aligned(&rows, "");

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
        aligned(&rows, "  ");
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
        aligned(&rows, "  ");
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
        aligned(&rows, "  ");
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
        for link in &explanation.path {
            let how = match (word(&link.kind).as_str(), link.run) {
                ("match", Some(run)) => format!("match in run {run}"),
                (kind, _) => kind.to_owned(),
            };
            println!("  {} and {}: {how}", link.from, link.to);
        }
    }
}

/// A changeset operation in words.
fn describe(op: &Op) -> String {
    match op {
        Op::CreateIdentity {
            identity,
            entity_type,
        } => format!("create {entity_type} identity {identity}"),
        Op::Assign {
            record,
            from: None,
            to: Some(to),
        } => format!("place record {record} in {to}"),
        Op::Assign {
            record,
            from: Some(from),
            to: Some(to),
        } => format!("move record {record} from {from} to {to}"),
        Op::Assign {
            record,
            from: Some(from),
            to: None,
        } => format!("take record {record} out of {from}"),
        Op::Assign {
            record,
            from: None,
            to: None,
        } => format!("leave record {record} without an identity"),
        Op::Merge { from, into } => format!("merge {from} into {into}"),
        Op::Constrain { kind, a, b } => format!("constrain {a} and {b}: {}", word(kind)),
        Op::Unconstrain { kind, a, b } => {
            format!("remove the {} constraint on {a} and {b}", word(kind))
        }
        Op::Reinstate { identity } => format!("reinstate identity {identity}"),
    }
}

/// What a run says of the `possible_split` items it raised.
fn left_for_review(identities: u64) -> String {
    format!(
        "{} with records that nothing holds together now (`possible_split`)",
        count(identities, "identity")
    )
}

/// What a review item is about, in a few words.
fn about(item: &ReviewEntry) -> String {
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
        aligned(&rows, "  ");
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
        aligned(&rows, "  ");
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

/// How an enum is written in the API, which is how the documentation names
/// it.
fn word<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(word)) => word,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

/// A JSON value on one line, text without its quotes.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "-".to_owned(),
        other => other.to_string(),
    }
}

/// A timestamp to the second.
fn when(timestamp: &str) -> String {
    match timestamp.split_once('.') {
        Some((seconds, _)) => format!("{seconds}Z"),
        None => timestamp.to_owned(),
    }
}

/// The most characters of a name a list shows. With a name of this
/// length and a key of ordinary length, a line of the list of records is
/// under a hundred characters.
const LABEL_CHARACTERS: usize = 40;

/// What a record or an identity is called, in a list: its label, cut to a
/// length that keeps the columns after it in sight.
fn label(label: Option<&str>) -> String {
    let Some(label) = label else {
        return "-".to_owned();
    };
    // A line break in a value would break the list.
    let line: String = label
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect();
    match line.chars().count() > LABEL_CHARACTERS {
        true => {
            let kept: String = line.chars().take(LABEL_CHARACTERS - 1).collect();
            format!("{}…", kept.trim_end())
        }
        false => line,
    }
}

/// Whether the rows of a list are of more than one entity type.
fn mixed<'a, T: PartialEq + 'a>(mut types: impl Iterator<Item = &'a T>) -> bool {
    match types.next() {
        Some(first) => types.any(|other| other != first),
        None => false,
    }
}

fn named(source: &str, key: Option<&str>) -> String {
    match key {
        Some(key) => format!("{source}:{key}"),
        None => format!("{source} (no key)"),
    }
}

/// What a list calls a record: its source and key, and a mention its
/// document and its place there, as an explanation does. The key of a
/// mention is the ID of its document and more, which nobody reads.
fn record_name(record: &RecordEntry) -> String {
    match &record.key {
        Some(_) => record.name(),
        None => named(&record.source, None),
    }
}

fn versioned(name: &str, version: Option<&str>) -> String {
    match version {
        Some(version) => format!("{name}, version {version}"),
        None => name.to_owned(),
    }
}

/// The first characters of a version's hash, which tell versions apart.
fn short_hash(hash: &str) -> &str {
    hash.get(..8).unwrap_or(hash)
}

/// Which extractors read a document, which could not and why, and which is
/// reading it now.
fn read_by(document: &DocumentEntry) -> String {
    let mut said: Vec<String> = document
        .extractions
        .iter()
        .map(|extraction| match &extraction.failure {
            Some(failure) => format!(
                "{} failed in run {}: {failure}",
                extraction.extractor, extraction.run
            ),
            None => format!(
                "{} in run {} ({})",
                extraction.extractor,
                extraction.run,
                count(extraction.mentions, "mention")
            ),
        })
        .collect();
    if let Some(reading) = &document.reading {
        said.push(format!(
            "being read by {} in run {}",
            reading.extractor, reading.run
        ));
    }
    match said.is_empty() {
        true => "not read yet".to_owned(),
        false => said.join(", "),
    }
}

/// What a match run placed, and the earlier versions it moved along.
pub(super) fn placed(report: &spool::TypeResolution) -> String {
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

pub(super) fn count(number: u64, what: &str) -> String {
    let plural = match what.strip_suffix('y') {
        _ if number == 1 => what.to_owned(),
        Some(stem) => format!("{stem}ies"),
        None => format!("{what}s"),
    };
    format!("{number} {plural}")
}

/// Label and value, one pair per line, the values in one column.
pub(super) fn fields(rows: &[(&str, String)]) {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|(label, value)| vec![(*label).to_owned(), value.clone()])
        .collect();
    aligned(&rows, "");
}

fn indented(rows: &[(&str, String)]) {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|(label, value)| vec![(*label).to_owned(), value.clone()])
        .collect();
    aligned(&rows, "  ");
}

/// The members of a JSON object, one per line. Anything else on one line.
fn object(value: &Value) {
    match value.as_object() {
        Some(members) => {
            let rows: Vec<Vec<String>> = members
                .iter()
                .map(|(name, value)| vec![name.clone(), plain(value)])
                .collect();
            aligned(&rows, "  ");
        }
        None => println!("  {}", plain(value)),
    }
}

/// A list under its column names, or a line that says there is nothing.
pub(super) fn table(what: &str, headers: &[&str], rows: Vec<Vec<String>>) {
    if rows.is_empty() {
        println!("no {what}");
        return;
    }
    let mut lines = vec![headers.iter().map(|header| (*header).to_owned()).collect()];
    lines.extend(rows);
    aligned(&lines, "");
}

/// Prints rows with their cells in columns. The last cell is as long as it
/// is.
fn aligned(rows: &[Vec<String>], indent: &str) {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in rows {
        let mut line = String::from(indent);
        for (column, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if column + 1 < row.len() {
                let padding = widths[column] - cell.chars().count() + 2;
                line.push_str(&" ".repeat(padding));
            }
        }
        println!("{}", line.trim_end());
    }
}

/// Says how to go on when a list has more than was shown.
fn more(answer: &Value) {
    if let Some(next) = answer["next"].as_str() {
        println!("\nmore: add `--after {next}`");
    }
}
