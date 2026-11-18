//! `spool show` and `spool explain`, run against a server over a socket.
//!
//! The test that starts `uv` is ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool-cli --test show -- --include-ignored
//! ```

mod support;

use std::path::Path;
use std::process::Output;

use serde_json::{Value, json};
use spool::Rows;
use support::*;

/// `spool <arguments>` against `server`, with its token in the environment
/// if it asks for one.
fn ask(server: &Server, arguments: &[&str]) -> Output {
    let mut command = spool();
    command
        .args(arguments)
        .args(["--url", &format!("http://{}", server.address)]);
    if let Some(token) = &server.token {
        command.env("SPOOL_TOKEN", token);
    }
    command.output().unwrap()
}

/// What a command that has to succeed printed.
fn shown(server: &Server, arguments: &[&str]) -> String {
    let output = ask(server, arguments);
    assert!(
        output.status.success(),
        "spool {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    text(&output.stdout)
}

/// What a command printed with `--json`.
fn answered(server: &Server, arguments: &[&str]) -> Value {
    let mut arguments = arguments.to_vec();
    arguments.push("--json");
    let printed = shown(server, &arguments);
    serde_json::from_str(&printed).unwrap_or_else(|error| panic!("{error}: {printed}"))
}

/// The exit code and the complaint of a command that has to fail.
fn refused(server: &Server, arguments: &[&str]) -> (Option<i32>, String) {
    let output = ask(server, arguments);
    assert!(!output.status.success(), "spool {arguments:?} succeeded");
    assert_eq!(text(&output.stdout), "", "nothing is shown on a failure");
    (output.status.code(), text(&output.stderr))
}

/// A line of what was printed, whatever the widths of its columns.
fn has(printed: &str, line: &str) {
    assert!(
        printed
            .lines()
            .any(|printed| printed.split_whitespace().eq(line.split_whitespace())),
        "no line `{line}` in:\n{printed}"
    );
}

fn ingest(server: &Server, source: &str, entity_type: &str, rows: Rows) {
    let response = server.post(
        "/v1/ingest",
        json!({
            "source": source,
            "entity_type": entity_type,
            "rows": json_rows(rows),
        }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
}

fn resolve(server: &Server, entity_type: &str) -> Value {
    server.call("/v1/runs", json!({ "entity_type": entity_type }))["run"].clone()
}

fn id_of(server: &Server, source: &str, key: &str) -> String {
    server.read(&format!("/v1/records?source={source}&key={key}"))["records"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Two black anvils, which are one thing; a red one, which the rules keep
/// apart from them; and a hammer.
fn workshop(dir: &Path) -> Server {
    let project = dir.join("project");
    write_things(&project);
    let server = Server::serve(&project, dir, 1);
    ingest(
        &server,
        "shop",
        "thing",
        rows(&[
            &["id", "code", "colour"],
            &["1", "anvil", "black"],
            &["2", "anvil", "black"],
            &["3", "anvil", "red"],
            &["4", "hammer", ""],
        ]),
    );
    assert_eq!(resolve(&server, "thing")["status"], "succeeded");
    server
}

#[test]
fn show_lists_what_a_server_holds() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());

    let overview = shown(&server, &["show"]);
    has(&overview, &format!("server     http://{}", server.address));
    has(&overview, "head       log position 1");
    has(&overview, "entity type `thing`");
    has(&overview, "  records     4, 0 without an identity");
    has(&overview, "  identities  3");
    has(&overview, "  review      0 items open");
    assert_eq!(answered(&server, &["show"]), server.read("/v1/info"));

    let runs = shown(&server, &["show", "runs"]);
    assert!(
        runs.starts_with("run  kind       status     about         started\n"),
        "{runs}"
    );
    assert!(
        runs.contains("\n2    match      succeeded  thing, rules  "),
        "{runs}"
    );
    assert!(
        runs.contains("\n1    ingest     succeeded  thing         "),
        "{runs}"
    );
    let matches = shown(&server, &["show", "runs", "--kind", "match"]);
    assert_eq!(matches.lines().count(), 2, "{matches}");
    assert_eq!(
        answered(
            &server,
            &["show", "runs", "--kind", "match", "--limit", "1"]
        ),
        server.read("/v1/runs?kind=match&limit=1")
    );

    let run = shown(&server, &["show", "run", "2"]);
    has(&run, "kind           match");
    has(&run, "strategy       rules, version 0.1.0");
    has(
        &run,
        "examined       4 records, 0 of them left for review before",
    );
    has(&run, "candidates     3 pairs");
    has(&run, "judgements     1 match, 2 no_match, 0 unsure");
    has(&run, "placed         4 records, 3 identities created");
    has(&run, "left out       0 records in 0 review items");
    assert!(
        run.contains("\ncommitted      log position 1, changeset "),
        "{run}"
    );
    let ingested = shown(&server, &["show", "runs", "1"]);
    has(
        &ingested,
        "rows      4 received: 4 new, 0 known, 0 new versions of a known row",
    );

    let identities = shown(&server, &["show", "identities"]);
    assert_eq!(identities.lines().count(), 4, "{identities}");
    assert!(
        identities
            .starts_with("identity                              name    members  created at\n"),
        "{identities}"
    );
    let page = shown(
        &server,
        &["show", "identities", "--limit", "1", "--type", "thing"],
    );
    let first = page
        .lines()
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    has(&page, &format!("more: add `--after {first}`"));
    let rest = shown(&server, &["show", "identities", "--after", first]);
    assert_eq!(rest.lines().count(), 3, "{rest}");
    assert!(!rest.contains(first));

    let (one, two) = (id_of(&server, "shop", "1"), id_of(&server, "shop", "2"));
    let anvils = server.read(&format!("/v1/records/{one}"))["identity"]
        .as_str()
        .unwrap()
        .to_owned();
    let identity = shown(&server, &["show", "identity", &anvils]);
    has(&identity, &format!("identity    {anvils}"));
    has(&identity, "created at  log position 1");
    has(
        &identity,
        "record                                source and key  since",
    );
    for (id, name) in [(&one, "shop:1"), (&two, "shop:2")] {
        has(&identity, &format!("{id}  {name}          log position 1"));
        has(&identity, &format!("      place record {id} in {anvils}"));
    }
    has(&identity, &format!("      create thing identity {anvils}"));
    assert!(
        identity.contains("  engine  reconcile run 3  \"reconcile new records\"\n"),
        "{identity}"
    );
    let bundle = answered(&server, &["show", "identities", &anvils]);
    assert_eq!(
        bundle["identity"],
        server.read(&format!("/v1/identities/{anvils}"))
    );
    assert_eq!(
        bundle["history"],
        server.read(&format!("/v1/identities/{anvils}/history"))["history"]
    );
    assert_eq!(bundle["constraints"], json!([]));

    // Records are listed by source and key, with the name their type's
    // label gives them. Their IDs are one option away.
    let records = shown(&server, &["show", "records", "--source", "shop"]);
    let listed: Vec<&str> = records.lines().collect();
    assert_eq!(listed.len(), 5, "{records}");
    assert_eq!(listed[0], "source and key  name    identity");
    assert_eq!(listed[1], format!("shop:1          anvil   {anvils}"));
    assert_eq!(listed[2], format!("shop:2          anvil   {anvils}"));
    assert!(
        listed[3].starts_with("shop:3          anvil   "),
        "{records}"
    );
    assert!(
        listed[4].starts_with("shop:4          hammer  "),
        "{records}"
    );
    let with_ids = shown(&server, &["show", "records", "--source", "shop", "--ids"]);
    assert_eq!(with_ids.lines().count(), 5, "{with_ids}");
    has(
        &with_ids,
        "record                                source and key  name    identity",
    );
    has(&with_ids, &format!("{one}  shop:1  anvil  {anvils}"));
    let page = shown(&server, &["show", "records", "--limit", "2"]);
    has(&page, &format!("more: add `--after {two}`"));
    let rest = shown(&server, &["show", "records", "--after", &two]);
    assert_eq!(rest.lines().count(), 3, "{rest}");
    assert!(rest.contains("\nshop:3  "), "{rest}");
    let none = shown(&server, &["show", "records", "--unassigned"]);
    assert_eq!(none, "no records\n");
    // `--ids` is an option of the list.
    let (code, complaint) = refused(&server, &["show", "record", "shop:1", "--ids"]);
    assert_eq!(code, Some(2));
    assert!(
        complaint.starts_with("spool: `--ids` goes with the list of records\n"),
        "{complaint}"
    );

    // A record is named by its ID or by its source and key.
    let record = shown(&server, &["show", "record", "shop:1"]);
    assert_eq!(record, shown(&server, &["show", "records", &one]));
    has(&record, &format!("record  {one}"));
    has(&record, "source       shop");
    has(&record, "key          1");
    has(&record, &format!("identity     {anvils}"));
    has(&record, "payload");
    has(&record, "  code    anvil");
    has(&record, "  colour  black");
    assert!(record.contains(" by run 1\n"), "{record}");
    let bundle = answered(&server, &["show", "record", "shop:1"]);
    assert_eq!(bundle["record"], server.read(&format!("/v1/records/{one}")));
    assert_eq!(bundle["mention"], Value::Null);
    assert_eq!(bundle["review_items"], json!([]));

    let log = shown(&server, &["show", "log"]);
    has(
        &log,
        "position  actor   operations  from             reason",
    );
    has(
        &log,
        "1         engine  7           reconcile run 3  reconcile new records",
    );
    assert_eq!(
        shown(&server, &["show", "log", "--after", "1"]),
        "no changesets\n"
    );
    assert_eq!(shown(&server, &["show", "drafts"]), "no drafts\n");
    assert_eq!(shown(&server, &["show", "review"]), "no review items\n");
    assert_eq!(shown(&server, &["show", "documents"]), "no documents\n");
}

#[test]
fn explain_says_why_two_records_are_together_or_apart() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());
    let (one, two, red) = (
        id_of(&server, "shop", "1"),
        id_of(&server, "shop", "2"),
        id_of(&server, "shop", "3"),
    );

    let together = shown(&server, &["explain", "shop:1", "shop:2"]);
    let mut lines = together.lines();
    let first = lines.next().unwrap();
    assert!(
        first.starts_with("`shop:") && first.contains("belong to the same identity, "),
        "{together}"
    );
    has(
        &together,
        "Run 2 (strategy `rules`) judged them `match` (match rule `same_code` held).",
    );
    has(&together, "features");
    has(&together, "  code    anvil  anvil");
    has(&together, "  colour  black  black");
    has(&together, "rules as they are now");
    has(&together, "  block     same_code     holds");
    has(&together, "  no_match  other_colour  does not hold");
    has(&together, "  match     same_code     holds");
    has(&together, "runs that compared them");
    has(
        &together,
        "  run 2  rules  succeeded  block: same_code  match",
    );
    // Either way of naming a record will do, in either order.
    assert_eq!(shown(&server, &["explain", &two, "shop:1"]), together);

    let apart = shown(&server, &["explain", "shop:1", &red]);
    has(
        &apart,
        "Run 2 (strategy `rules`) judged them `no_match` (no_match rule `other_colour` held).",
    );
    has(&apart, "  no_match  other_colour  holds");
    has(
        &apart,
        "  run 2  rules  succeeded  block: same_code  no_match",
    );

    let never = shown(&server, &["explain", "shop:1", "shop:4"]);
    has(&never, "No run has compared them.");
    has(&never, "  block     same_code     does not hold");
    has(
        &never,
        "  no_match  other_colour  cannot be decided: a feature it reads is null",
    );
    assert!(!never.contains("runs that compared them"), "{never}");

    let answer = answered(&server, &["explain", "shop:1", "shop:2"]);
    assert_eq!(answer, server.read(&format!("/v1/explain?a={one}&b={two}")));
    assert_eq!(answer["outcome"], "same_identity");

    // A reviewer's decisions are part of the explanation.
    let identity = answer["left"]["record"]["identity"].as_str().unwrap();
    server.call(
        &format!("/v1/identities/{identity}/split"),
        json!({ "records": [two], "reason": "a replica" }),
    );
    let split = shown(&server, &["explain", "shop:1", "shop:2"]);
    has(
        &split,
        "A `cannot_link` constraint committed at log position 2 keeps them apart, whatever a strategy judges.",
    );
    has(&split, "constraints between them");
    has(&split, "  cannot_link from log position 2");
    has(
        &split,
        "`shop:2` was placed at log position 2 by `agent:api` (\"a replica\").",
    );
    let identity = shown(&server, &["show", "identity", identity]);
    has(
        &identity,
        "constraint   between                               and                                   since",
    );
    assert!(identity.contains("\ncannot_link  "), "{identity}");
    assert!(
        identity.contains("  agent:api  \"a replica\"\n"),
        "{identity}"
    );
    assert!(
        identity.contains(&format!("      move record {two} from ")),
        "{identity}"
    );

    let (code, complaint) = refused(&server, &["explain", "shop:1"]);
    assert_eq!(code, Some(2));
    assert!(
        complaint.contains("`explain` needs two records"),
        "{complaint}"
    );
    let (code, complaint) = refused(&server, &["explain", "shop:1", "anvil"]);
    assert_eq!(code, Some(2));
    assert!(
        complaint.contains("`anvil` does not name a record"),
        "{complaint}"
    );
    let (code, complaint) = refused(&server, &["explain", "shop:1", "shop:9"]);
    assert_eq!(code, Some(1));
    assert_eq!(
        complaint,
        "spool: there is no record with key `9` from source `shop`\n"
    );
    let (code, complaint) = refused(&server, &["explain", "shop:1", "shop:1"]);
    assert_eq!(code, Some(1));
    assert!(complaint.contains("named twice"), "{complaint}");
    let unknown = "00000000-0000-7000-8000-000000000000";
    let (code, complaint) = refused(&server, &["explain", "shop:1", unknown]);
    assert_eq!(code, Some(1));
    assert_eq!(complaint, format!("spool: there is no record {unknown}\n"));
}

#[test]
fn drafts_and_review_items_are_shown_with_what_they_hold() {
    let dir = tempfile::tempdir().unwrap();
    let companies = benchmarks().join("identity/companies-small");
    let project = dir.path().join("project");
    copy_project(&companies, &project);
    let server = Server::serve(&project, dir.path(), 1);
    for (file, source) in [
        ("batch-1-crm.csv", "crm"),
        ("batch-1-registry.csv", "registry"),
    ] {
        let rows = Rows::from_csv(companies.join(file)).unwrap();
        ingest(&server, source, "company", rows);
    }

    // The first run's draft is left for a reviewer.
    let run = server.call(
        "/v1/runs",
        json!({ "entity_type": "company", "commit": false }),
    )["run"]
        .clone();
    let draft = run["resolution"]["draft"].as_str().unwrap();
    let drafts = shown(&server, &["show", "drafts"]);
    has(
        &drafts,
        "draft                                 type     status  operations  from",
    );
    assert!(
        drafts.contains(&format!("\n{draft}  company  open    ")),
        "{drafts}"
    );
    assert_eq!(
        shown(&server, &["show", "drafts", "--status", "rejected"]),
        "no drafts\n"
    );
    let overview = shown(&server, &["show"]);
    has(
        &overview,
        &format!("  draft       {draft} waits for a decision"),
    );
    let waiting = shown(&server, &["show", "run", &run["run"].to_string()]);
    has(&waiting, "status         drafted");
    has(&waiting, &format!("draft          {draft}"));

    let one = shown(&server, &["show", "draft", draft]);
    has(&one, &format!("draft            {draft}"));
    has(&one, "status           open");
    has(&one, "written against  log position 0");
    has(&one, "operations");
    assert!(one.contains("\n  create company identity "), "{one}");
    assert!(one.contains("\n  place record "), "{one}");
    assert_eq!(
        answered(&server, &["show", "draft", draft]),
        server.read(&format!("/v1/drafts/{draft}"))
    );
    server.call(&format!("/v1/drafts/{draft}/approve"), json!({}));

    // The second batch has a record that fits two companies.
    for (file, source) in [
        ("batch-2-crm.csv", "crm"),
        ("batch-2-registry.csv", "registry"),
    ] {
        let rows = Rows::from_csv(companies.join(file)).unwrap();
        ingest(&server, source, "company", rows);
    }
    resolve(&server, "company");
    let stuck = id_of(&server, "crm", "c-117");

    let review = shown(&server, &["show", "review", "--status", "open"]);
    has(
        &review,
        "item  kind            status  type     holds     matched       raised by",
    );
    assert!(
        review.contains("\n1     possible_merge  open    company  1 record  2 identities  run "),
        "{review}"
    );
    assert_eq!(
        shown(&server, &["show", "review", "--status", "dismissed"]),
        "no review items\n"
    );
    let item = shown(&server, &["show", "review", "1"]);
    has(&item, "review item  1");
    has(&item, "kind         possible_merge");
    has(&item, "status       open");
    has(&item, &format!("{stuck}  crm:c-117  waits for an identity"));
    has(&item, "identities they matched");
    let bundle = answered(&server, &["show", "review", "1"]);
    assert_eq!(bundle["item"], server.read("/v1/review/1"));
    assert_eq!(bundle["records"][0]["id"], stuck.as_str());

    // The record says it is under review, and an explanation says why.
    let record = shown(&server, &["show", "record", "crm:c-117"]);
    has(&record, "identity     none");
    has(&record, "review       item 1 (possible_merge, open)");
    let unassigned = shown(&server, &["show", "records", "--unassigned"]);
    assert_eq!(
        unassigned,
        "\
source and key  name              identity
crm:c-117       Pioneer Supplies  -

1 record has no identity. `spool match` places new records in identities,
and `spool show review` lists the ones a match left out.
"
    );
    let matched = server.read("/v1/review/1")["identities"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let member = server.read(&format!("/v1/identities/{matched}"))["members"][0]["record"]
        .as_str()
        .unwrap()
        .to_owned();
    let explanation = shown(&server, &["explain", "crm:c-117", &member]);
    assert!(
        explanation.contains(
            "\nReview item 1 (`possible_merge`) is open and names `crm:c-117`: what it matched belongs to more than one identity, and merging identities is left to a reviewer.\n"
        ),
        "{explanation}"
    );
    has(&explanation, "review items that name them");
    has(&explanation, "  item 1 (possible_merge, open)");

    let (code, complaint) = refused(&server, &["show", "review", "9"]);
    assert_eq!(code, Some(1));
    assert_eq!(complaint, "spool: there is no review item 9\n");

    // The same company under a second key is held back with the first. When
    // the CRM takes that row back, the item says which of the two still
    // waits, and which identity a record went to once it is placed.
    let mut again = server.read(&format!("/v1/records/{stuck}"))["payload"].clone();
    again["id"] = json!("c-217");
    let response = server.post(
        "/v1/ingest",
        json!({ "source": "crm", "entity_type": "company", "rows": [again] }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
    resolve(&server, "company");
    let open = server.read("/v1/review?status=open");
    let [raised] = &open["items"].as_array().unwrap()[..] else {
        panic!("one open review item: {open}");
    };
    assert_eq!(raised["records"].as_array().unwrap().len(), 2);
    let raised = raised["id"].to_string();
    let taken = server.call(
        "/v1/retract",
        json!({ "source": "crm", "entity_type": "company", "keys": ["c-217"] }),
    )["run"]["run"]
        .clone();
    let item = shown(&server, &["show", "review", &raised]);
    has(&item, "status       open");
    has(&item, &format!("{stuck}  crm:c-117  waits for an identity"));
    has(
        &item,
        &format!(
            "{}  crm:c-217  taken back by its source in run {taken}, so it does not wait",
            id_of(&server, "crm", "c-217")
        ),
    );
    server.call(
        "/v1/changesets",
        json!({ "ops": [{ "op": "assign", "record": stuck, "from": null, "to": matched }] }),
    );
    let item = shown(&server, &["show", "review", &raised]);
    has(&item, "status       resolved");
    has(&item, &format!("{stuck}  crm:c-117  in identity {matched}"));
}

#[test]
fn a_mention_is_shown_in_its_document() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    let filler = "x".repeat(100);
    let note = format!("{filler} The Anvil is\nblack, as anvils are. {filler}");
    server.call(
        "/v1/documents",
        json!({ "source": "notes", "documents": [
            { "key": "a.txt", "uri": "file:///notes/a.txt", "text": note },
            { "key": "b.txt", "text": "A hammer." },
        ] }),
    );
    let documents = server.read("/v1/documents?source=notes&key=a.txt");
    let document = documents["documents"][0]["id"].as_str().unwrap();

    // Evidence that names a place in a stored document is what an extractor
    // writes. A caller that finds mentions itself can write the same.
    let evidence = json!({ "document": document, "key": "a.txt", "start": 105, "end": 110 });
    let response = server.post(
        "/v1/ingest",
        json!({
            "source": "notes",
            "entity_type": "thing",
            "rows": [{ "id": "m1", "code": "anvil", "where": evidence.to_string() }],
        }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");

    let listed = shown(&server, &["show", "documents", "--source", "notes"]);
    has(
        &listed,
        "document                              source and key  characters  read by",
    );
    has(
        &listed,
        &format!(
            "{document}  notes:a.txt     {:<10}  no extractor",
            note.chars().count()
        ),
    );
    assert_eq!(listed.lines().count(), 3, "{listed}");
    let one = shown(&server, &["show", "document", document]);
    has(&one, &format!("document    {document}"));
    has(&one, "uri         file:///notes/a.txt");
    assert!(one.ends_with(&format!("\n\n{note}\n")), "{one}");

    let record = shown(&server, &["show", "record", "notes:m1"]);
    has(&record, "evidence");
    has(&record, &format!("  document  {document}"));
    has(&record, "in document notes:a.txt, characters 105 to 110");
    // Eighty characters either side, on one line, with the mention marked.
    has(
        &record,
        &format!(
            "  …{} The [Anvil] is black, as anvils are. {}…",
            "x".repeat(75),
            "x".repeat(54)
        ),
    );
    let bundle = answered(&server, &["show", "record", "notes:m1"]);
    assert_eq!(bundle["mention"]["text"], "Anvil");
    assert_eq!(bundle["mention"]["document"], document);
    assert_eq!(bundle["mention"]["cut_before"], true);
    // The list names a mention by its document and its place there, as
    // an explanation does, and the name finds the record.
    let found = shown(&server, &["show", "records", "--document", document]);
    has(&found, "notes:a.txt[105-110]  anvil  -");
    assert_eq!(
        shown(&server, &["show", "record", "notes:a.txt[105-110]"]),
        record
    );
    let (code, complaint) = refused(&server, &["show", "record", "notes:a.txt[1-2]"]);
    assert_eq!(code, Some(1));
    assert_eq!(
        complaint,
        "spool: there is no record with key `a.txt[1-2]` from source `notes`\n"
    );

    // Evidence that points outside the document is shown as it is.
    let evidence = json!({ "document": document, "start": 5, "end": 5000 });
    server.call(
        "/v1/ingest",
        json!({
            "source": "notes",
            "entity_type": "thing",
            "rows": [{ "id": "m2", "code": "tongs", "where": evidence.to_string() }],
        }),
    );
    let record = shown(&server, &["show", "record", "notes:m2"]);
    has(&record, "  end       5000");
    assert!(!record.contains("in document"), "{record}");

    // A document's mentions are listed in the order of its text. The key an
    // extractor gives one begins with the ID of the document, and by those
    // keys `[105-110]` would come before `[12-17]`.
    let evidence = json!({ "document": document, "key": "a.txt", "start": 12, "end": 17 });
    server.call(
        "/v1/ingest",
        json!({
            "source": "notes",
            "entity_type": "thing",
            "rows": [{ "id": "a-first", "code": "xs", "where": evidence.to_string() }],
        }),
    );
    let listed = shown(&server, &["show", "records", "--source", "notes"]);
    let names: Vec<&str> = listed
        .lines()
        .skip(1)
        .take(3)
        .map(|line| line.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(
        names,
        ["notes:a.txt[12-17]", "notes:a.txt[105-110]", "notes:m2"],
        "{listed}"
    );

    // A mention is named by its document and its place in it, when its
    // evidence says both.
    let explanation = shown(&server, &["explain", "notes:m1", "notes:m2"]);
    let summary = explanation.lines().next().unwrap();
    assert!(summary.contains("`notes:a.txt[105-110]`"), "{explanation}");
    assert!(summary.contains("`notes:m2`"), "{explanation}");
}

#[test]
fn a_source_and_key_name_the_latest_version_however_many_there_were() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let server = Server::serve(&project, dir.path(), 1);

    // More versions of one row than a page of records holds. They are
    // listed by ID, which says nothing of their order in time.
    let versions = 250;
    for version in 1..=versions {
        let code = format!("anvil-{version}");
        ingest(
            &server,
            "shop",
            "thing",
            rows(&[&["id", "code"], &["1", &code]]),
        );
    }
    ingest(
        &server,
        "shop",
        "thing",
        rows(&[&["id", "code"], &["2", "anvil-250"]]),
    );
    let first = server.read("/v1/records?source=shop&key=1");
    assert_eq!(first["records"].as_array().unwrap().len(), 100);
    assert!(
        first["records"]
            .as_array()
            .unwrap()
            .iter()
            .all(|version| !version["superseded_by"].is_null()),
        "the latest version is not on the first page, or this tests nothing"
    );

    let latest = answered(&server, &["show", "record", "shop:1"]);
    assert_eq!(latest["record"]["payload"]["code"], "anvil-250");
    assert_eq!(latest["record"]["superseded_by"], Value::Null);

    resolve(&server, "thing");
    let explanation = answered(&server, &["explain", "shop:1", "shop:2"]);
    assert_eq!(explanation["outcome"], "same_identity");
    let sides = [&explanation["left"], &explanation["right"]];
    assert!(
        sides
            .iter()
            .all(|side| side["record"]["payload"]["code"] == "anvil-250"),
        "{explanation}"
    );
}

#[test]
fn a_merged_identity_is_shown_with_the_constraints_of_the_one_it_became() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());
    let (one, two, hammer) = (
        id_of(&server, "shop", "1"),
        id_of(&server, "shop", "2"),
        id_of(&server, "shop", "4"),
    );
    let identity_of = |record: &str| {
        server.read(&format!("/v1/records/{record}"))["identity"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (anvils, hammers) = (identity_of(&one), identity_of(&hammer));
    server.call(
        "/v1/changesets",
        json!({
            "ops": [{ "op": "constrain", "kind": "must_link", "a": one, "b": two }],
        }),
    );
    server.call(
        "/v1/changesets",
        json!({
            "ops": [{ "op": "merge", "from": anvils, "into": hammers }],
        }),
    );

    // The merged ID answers with the members of the identity that stands
    // for it, and so with the constraints on those members.
    let expected =
        server.read(&format!("/v1/constraints?identity={hammers}"))["constraints"].clone();
    assert_eq!(expected.as_array().unwrap().len(), 1);
    let bundle = answered(&server, &["show", "identity", &anvils]);
    assert_eq!(bundle["identity"]["current"], hammers.as_str());
    assert_eq!(bundle["identity"]["members"].as_array().unwrap().len(), 3);
    assert_eq!(bundle["constraints"], expected);
    let merged = shown(&server, &["show", "identity", &anvils]);
    has(&merged, &format!("merged into  {hammers}"));
    assert!(merged.contains("\nmust_link   "), "{merged}");
    let current = shown(&server, &["show", "identity", &hammers]);
    assert!(current.contains("\nmust_link   "), "{current}");
}

#[test]
fn a_type_with_no_rules_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    std::fs::remove_dir_all(project.join("entities/thing/rules")).unwrap();
    let server = Server::serve(&project, dir.path(), 1);
    ingest(
        &server,
        "shop",
        "thing",
        rows(&[&["id", "code"], &["1", "anvil"], &["2", "anvil"]]),
    );

    let explanation = shown(&server, &["explain", "shop:1", "shop:2"]);
    has(
        &explanation,
        "The entity type has no block rule, so a run of the rules would not compare them.",
    );
    has(&explanation, "rules as they are now");
    has(&explanation, "  the entity type has none");
}

#[test]
fn a_rule_that_sql_refuses_on_a_pair_is_shown_as_one_that_cannot_be_applied() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    // The match rule is safe on what the block rule lets through.
    for (file, sql) in [
        (
            "rules/block/same_code.sql",
            "l.code = r.code AND TRY_CAST(l.code AS INTEGER) IS NOT NULL\n",
        ),
        (
            "rules/match/same_code.sql",
            "CAST(l.code AS INTEGER) = CAST(r.code AS INTEGER)\n",
        ),
    ] {
        std::fs::write(project.join("entities/thing").join(file), sql).unwrap();
    }
    let server = Server::serve(&project, dir.path(), 1);
    ingest(
        &server,
        "shop",
        "thing",
        rows(&[&["id", "code"], &["1", "anvil"], &["2", "anvil"]]),
    );
    assert_eq!(resolve(&server, "thing")["status"], "succeeded");

    let explanation = shown(&server, &["explain", "shop:1", "shop:2"]);
    has(&explanation, "  block     same_code     does not hold");
    has(
        &explanation,
        "  match     same_code     cannot be applied to these values",
    );
    has(
        &explanation,
        "  applying the `no_match` and `match` rules failed: Conversion Error: Could not convert string 'anvil' to INT32 when casting from source column code",
    );
    // The server answers the next call.
    has(&shown(&server, &["show"]), "  identities  2");
}

#[test]
fn a_mistake_or_a_server_that_does_not_answer_is_said_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());

    for (arguments, expected) in [
        (
            &["show", "weather"][..],
            "`weather` is not something `show` shows",
        ),
        (&["show", "runs", "first"], "`first` is not a run ID"),
        (
            &["show", "identity", "anvil"],
            "`anvil` is not an identity ID",
        ),
        (&["show", "draft", "anvil"], "`anvil` is not a draft ID"),
        (
            &["show", "document", "anvil"],
            "`anvil` is not a document ID",
        ),
        (
            &["show", "review", "first"],
            "`first` is not a review item ID",
        ),
        (
            &["show", "record", "anvil"],
            "`anvil` does not name a record",
        ),
        (
            &["show", "runs", "--source", "shop"],
            "`--source` does not go with `runs`",
        ),
        (
            &["show", "run", "1", "--kind", "match"],
            "`--kind` does not go with `a run`",
        ),
        (
            &["show", "--type", "thing"],
            "`--type` does not go with `the overview`",
        ),
        (&["show", "log", "1"], "unexpected argument `1`"),
        (&["show", "runs", "1", "2"], "unexpected argument `2`"),
        (
            &["show", "runs", "--limit", "few"],
            "`--limit` needs a whole number",
        ),
        (&["show", "runs", "--all"], "unknown option `--all`"),
    ] {
        let (code, complaint) = refused(&server, arguments);
        assert_eq!(code, Some(2), "{arguments:?}: {complaint}");
        assert!(complaint.contains(expected), "{arguments:?}: {complaint}");
        assert!(complaint.contains("Usage: spool show"), "{complaint}");
    }

    // What the server refuses is said in the server's words.
    for (arguments, expected) in [
        (
            &["show", "runs", "99"][..],
            "spool: there is no run 99\n".to_owned(),
        ),
        (
            &["show", "identity", "00000000-0000-7000-8000-000000000000"],
            "spool: there is no identity 00000000-0000-7000-8000-000000000000\n".to_owned(),
        ),
        (
            &["show", "record", "shop:9"],
            "spool: there is no record with key `9` from source `shop`\n".to_owned(),
        ),
        (
            &["show", "document", "00000000-0000-7000-8000-000000000000"],
            "spool: there is no document 00000000-0000-7000-8000-000000000000\n".to_owned(),
        ),
        (
            &["show", "draft", "00000000-0000-7000-8000-000000000000"],
            "spool: there is no draft 00000000-0000-7000-8000-000000000000\n".to_owned(),
        ),
    ] {
        let (code, complaint) = refused(&server, arguments);
        assert_eq!(code, Some(1), "{arguments:?}");
        assert_eq!(complaint, expected, "{arguments:?}");
    }
    let (code, complaint) = refused(&server, &["show", "runs", "--status", "asleep"]);
    assert_eq!(code, Some(1));
    assert!(complaint.contains("asleep"), "{complaint}");

    // The token is taken from the option before the environment.
    let wrong = spool()
        .args(["show", "--url", &format!("http://{}", server.address)])
        .env("SPOOL_TOKEN", "not-the-token")
        .output()
        .unwrap();
    assert_eq!(wrong.status.code(), Some(1));
    assert!(
        text(&wrong.stderr).contains("did not accept the token"),
        "{}",
        text(&wrong.stderr)
    );
    let right = spool()
        .args(["show", "--url", &format!("http://{}/", server.address)])
        .args(["--token", server.token()])
        .env("SPOOL_TOKEN", "not-the-token")
        .output()
        .unwrap();
    assert!(right.status.success(), "{}", text(&right.stderr));

    // The server can be named in the environment.
    let named = spool()
        .args(["show", "runs", "--limit", "1"])
        .env("SPOOL_URL", format!("http://{}", server.address))
        // A token read from a file often ends with a newline, which is not
        // part of it.
        .env("SPOOL_TOKEN", format!("{}\n", server.token()))
        .output()
        .unwrap();
    assert!(named.status.success(), "{}", text(&named.stderr));
    assert_eq!(text(&named.stdout).lines().count(), 2);

    // Nothing listens where the server was.
    let address = server.address.clone();
    server.stop();
    let gone = spool()
        .args(["show", "--url", &format!("http://{address}")])
        .output()
        .unwrap();
    assert_eq!(gone.status.code(), Some(1));
    let complaint = text(&gone.stderr);
    assert!(
        complaint.starts_with(&format!(
            "spool: the server at http://{address} cannot be reached: "
        )),
        "{complaint}"
    );
    assert!(complaint.contains("spool dev"), "{complaint}");
}

#[test]
fn the_defaults_are_those_of_spool_dev() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let server = Server::start_with(&[
        "dev",
        path(dir.path()),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ])
    .unwrap();

    // No token is named, and the development server asks for none.
    let overview = spool()
        .args(["show", "--url", &format!("http://{}", server.address)])
        .output()
        .unwrap();
    assert!(overview.status.success(), "{}", text(&overview.stderr));
    has(&text(&overview.stdout), "entity type `thing`");
    has(
        &text(&overview.stdout),
        "  records     0, 0 without an identity",
    );
}

/// Takes a capitalised word to be a thing, with the word in lower case as
/// its code.
const WORDS: &str = r#"
import re

from spool import extractor


@extractor
def find(document):
    for found in re.finditer(r"[A-Z][a-z]+", document.text):
        yield document.mention("thing", found.start(), found.end(), code=found.group().lower())
"#;

#[test]
#[ignore = "starts uv, which downloads packages the first time"]
fn what_an_extractor_found_is_shown_and_explained() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let extractor = project.join("extractors/words.py");
    std::fs::create_dir_all(extractor.parent().unwrap()).unwrap();
    std::fs::write(extractor, WORDS).unwrap();
    let manifest = build(&project);
    let server = Server::start(&[
        "serve",
        "--manifest",
        path(&manifest),
        "--project",
        path(&project),
        "--tasks",
        path(&dir.path().join("tasks")),
        "--store",
        path(&dir.path().join("store.duckdb")),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ]);
    server.call(
        "/v1/documents",
        json!({ "source": "notes", "documents": [
            { "key": "a.txt", "text": "the Anvil rang" },
            { "key": "b.txt", "text": "an Anvil, again" },
        ] }),
    );
    let run = server.call("/v1/extractions", json!({ "extractor": "words" }))["run"].clone();
    assert_eq!(run["status"], "succeeded", "{run}");
    resolve(&server, "thing");

    let overview = shown(&server, &["show"]);
    has(&overview, "documents  2");
    has(&overview, "extractor `words` (python, extractors/words.py)");
    let extraction = shown(&server, &["show", "run", &run["run"].to_string()]);
    has(&extraction, "kind       extract");
    has(&extraction, "documents  2 read");
    has(
        &extraction,
        "mentions   2 found: 2 new, 0 known, 0 new versions of a known mention",
    );
    let documents = shown(&server, &["show", "documents"]);
    assert!(
        documents.contains(&format!("  words in run {} (1 mention)\n", run["run"])),
        "{documents}"
    );

    let mentions = server.read("/v1/records?source=notes");
    let [first, second] = &mentions["records"].as_array().unwrap()[..] else {
        panic!("two mentions: {mentions}");
    };
    let record = shown(&server, &["show", "record", first["id"].as_str().unwrap()]);
    has(&record, "  extractor  words");
    assert!(
        record.contains("\n  the [Anvil] rang\n") || record.contains("\n  an [Anvil], again\n"),
        "{record}"
    );

    // A mention is named by its document and its place in it.
    let explanation = shown(
        &server,
        &[
            "explain",
            first["id"].as_str().unwrap(),
            second["id"].as_str().unwrap(),
        ],
    );
    let first_line = explanation.lines().next().unwrap();
    assert!(
        first_line.contains("`notes:a.txt[4-9]`") && first_line.contains("`notes:b.txt[3-8]`"),
        "{explanation}"
    );
    assert!(
        first_line.contains("belong to the same identity"),
        "{explanation}"
    );
}
