//! `spool show` and `spool explain`: what a running server holds, and why
//! two records share an identity or do not.
//!
//! Both read through the server's API and print what it answered. With
//! `--json` they print the answer itself. Nothing here decides anything: a
//! sentence that says why comes from the engine.

use serde::Serialize;
use serde_json::{Value, json};
use spool::core::{
    ChangesetId, ConstraintEntry, DocumentId, IdentityId, LogEntry, Op, RecordId, ReviewItem,
};
use spool::{
    DocumentEntry, DraftEntry, DraftSummary, Explanation, HistoryEntry, IdentitySummary,
    IdentityView, RecordEntry, ReviewEntry, RuleResult, RunEntry,
};
use spool_cli::api::{Info, RecordList};

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
  records [<record>]   records, or one with its payload, its
                                 evidence and, for a mention, the place in the
                                 document it was found in
  documents [<document>]         documents, or one with its text
  review [<item>]                review items, or one with its records
  drafts [<draft>]               drafts, or one with the changeset it would commit
  log                            the changeset log

A record is named by its ID or as <source>:<key>, which names the
latest version of that row.

Options:
  --type <name>      only this entity type (runs, identities, records,
                     review, drafts)
  --status <status>  only this status (runs, review, drafts)
  --kind <kind>      only runs of this kind: ingest, extract, match, reconcile
  --source <name>    only this source (records, documents)
  --key <key>        only this key within its source (records, documents)
  --document <id>    only the mentions found in this document (records)
  --unassigned       only records without an identity
  --latest           only the version of each document that nothing replaced
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
latest version of that row.

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
            flags: &["--unassigned", "--latest", "--json"],
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
/// names the version of the row that nothing has replaced.
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
    // Versions are listed by ID, in pages, and the latest can be on any of
    // them: it is the one no other replaced.
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
    last.ok_or_else(|| {
        Failure::Failed(format!(
            "there is no record with key `{key}` from source `{source}`"
        ))
    })
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
                    format!(
                        "{}, {} without an identity",
                        entity_type.records, entity_type.unassigned
                    ),
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
            &["run", "kind", "status", "about", "started"],
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
        rows.push(("started", when(&run.started_at)));
        if let Some(finished) = &run.finished_at {
            rows.push(("finished", when(finished)));
        }
        if let Some(error) = &run.error {
            rows.push(("error", error.clone()));
        }
        if let Some(report) = &run.ingest {
            rows.push((
                "rows",
                format!(
                    "{} received: {} new, {} known, {} new versions of a known row",
                    report.received, report.inserted, report.duplicates, report.superseded
                ),
            ));
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
            if let Some(ingest_run) = report.ingest_run {
                rows.push(("stored by", format!("ingest run {ingest_run}")));
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
            rows.push((
                "placed",
                format!(
                    "{}, {} created",
                    count(report.records_assigned, "record"),
                    count(report.identities_created, "identity")
                ),
            ));
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
        table(
            "identities",
            &["identity", "type", "members", "created at"],
            identities
                .iter()
                .map(|identity| {
                    vec![
                        identity.id.to_string(),
                        identity.entity_type.to_string(),
                        identity.members.to_string(),
                        format!("log position {}", identity.created),
                    ]
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
        let constraints = self.remote.list("/constraints", &[("identity", current)])?;
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

        println!();
        table(
            "members",
            &["record", "source and key", "since"],
            view.members
                .iter()
                .map(|member| {
                    vec![
                        member.record.to_string(),
                        named(&member.source, member.key.as_deref()),
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
        table(
            "records",
            &["record", "type", "source and key", "identity"],
            records
                .iter()
                .map(|record| {
                    vec![
                        record.id.to_string(),
                        record.entity_type.to_string(),
                        named(&record.source, record.key.as_deref()),
                        record
                            .identity
                            .map_or_else(|| "-".to_owned(), |identity| identity.to_string()),
                    ]
                })
                .collect(),
        );
        more(&answer);
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
            "ingested",
            format!("{} by run {}", when(&record.ingested_at), record.ingest_run),
        ));
        rows.push((
            "identity",
            record
                .identity
                .map_or_else(|| "none".to_owned(), |identity| identity.to_string()),
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
            "ingested",
            format!(
                "{} by run {}",
                when(&document.ingested_at),
                document.ingest_run
            ),
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
            &[
                "item",
                "kind",
                "status",
                "type",
                "holds",
                "matched",
                "raised by",
            ],
            items
                .iter()
                .map(|item| {
                    vec![
                        item.id.to_string(),
                        word(&item.kind),
                        word(&item.status),
                        item.entity_type.to_string(),
                        count(item.records.len() as u64, "record"),
                        count(item.identities.len() as u64, "identity"),
                        format!("run {}", item.run),
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
        if self.json {
            print_json(&json!({ "item": answer, "records": records }));
            return Ok(());
        }
        let records: Vec<RecordEntry> = typed(&Value::Array(records))?;

        let mut rows = vec![
            ("review item", item.id.to_string()),
            ("kind", word(&item.kind)),
            ("status", word(&item.status)),
            ("type", item.entity_type.to_string()),
            ("raised by", format!("reconcile run {}", item.run)),
        ];
        if let Some(seq) = item.resolved_seq {
            rows.push(("resolved at", format!("log position {seq}")));
        }
        if let Some(run) = item.superseded_by {
            rows.push(("raised again by", format!("reconcile run {run}")));
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
        table(
            "records left without an identity",
            &["record", "source and key"],
            records
                .iter()
                .map(|record| {
                    vec![
                        record.id.to_string(),
                        named(&record.source, record.key.as_deref()),
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
        let entries: Vec<LogEntry> = typed(&answer["changesets"])?;
        table(
            "changesets",
            &["position", "actor", "operations", "from", "reason"],
            entries
                .iter()
                .map(|entry| {
                    let changeset = &entry.changeset;
                    vec![
                        entry.seq.to_string(),
                        changeset.actor.to_string(),
                        changeset.ops.len().to_string(),
                        changeset
                            .run
                            .map_or_else(|| "-".to_owned(), |run| format!("reconcile run {run}")),
                        changeset.reason.clone().unwrap_or_else(|| "-".to_owned()),
                    ]
                })
                .collect(),
        );
        if let (Some(last), Some(head)) = (entries.last(), answer["head"].as_u64())
            && last.seq.0 < head
        {
            println!("\nmore: add `--after {}`", last.seq);
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

fn named(source: &str, key: Option<&str>) -> String {
    match key {
        Some(key) => format!("{source}:{key}"),
        None => format!("{source} (no key)"),
    }
}

fn versioned(name: &str, version: Option<&str>) -> String {
    match version {
        Some(version) => format!("{name}, version {version}"),
        None => name.to_owned(),
    }
}

fn read_by(document: &DocumentEntry) -> String {
    match document.extractions.is_empty() {
        true => "no extractor".to_owned(),
        false => document
            .extractions
            .iter()
            .map(|extraction| {
                format!(
                    "{} in run {} ({})",
                    extraction.extractor,
                    extraction.run,
                    count(extraction.mentions, "mention")
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn count(number: u64, what: &str) -> String {
    let plural = match what.strip_suffix('y') {
        _ if number == 1 => what.to_owned(),
        Some(stem) => format!("{stem}ies"),
        None => format!("{what}s"),
    };
    format!("{number} {plural}")
}

/// Label and value, one pair per line, the values in one column.
fn fields(rows: &[(&str, String)]) {
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
