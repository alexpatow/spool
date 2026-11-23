//! `spool show` and `spool explain`, run against a server over a socket.
//!
//! The test that starts `uv` is ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool-cli --test it -- --include-ignored show::
//! ```

use std::path::Path;
use std::process::Output;

use serde_json::{Value, json};
use spool::Rows;

use crate::support::*;

mod attributes;
mod explain;
mod mentions;

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

/// What a command that has to succeed printed: its result, then what it
/// said beside on standard error.
fn shown(server: &Server, arguments: &[&str]) -> String {
    let output = ask(server, arguments);
    assert!(
        output.status.success(),
        "spool {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    format!("{}{}", text(&output.stdout), text(&output.stderr))
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

fn load(server: &Server, source: &str, entity_type: &str, rows: Rows) {
    let response = server.post(
        "/v1/load",
        json!({
            "source": source,
            "entity_type": entity_type,
            "rows": json_rows(rows),
        }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
}

fn resolve(server: &Server, entity_type: &str) -> Value {
    server.call("/v1/match", json!({ "entity_type": entity_type }))["run"].clone()
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
    load(
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

    show_runs(&server);

    let (one, two) = (id_of(&server, "shop", "1"), id_of(&server, "shop", "2"));
    let anvils = server.read(&format!("/v1/records/{one}"))["identity"]
        .as_str()
        .unwrap()
        .to_owned();
    show_identities(&server, &one, &two, &anvils);

    show_records(&server, &one, &two, &anvils);

    let log = shown(&server, &["show", "log"]);
    has(
        &log,
        "POSITION  ACTOR   OPERATIONS  FROM             STARTED BY  REASON",
    );
    has(
        &log,
        "1         engine  7           reconcile run 3  agent:api   reconcile new records",
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
        load(&server, source, "company", rows);
    }

    // The first run's draft is left for a reviewer.
    let run = server.call(
        "/v1/match",
        json!({ "entity_type": "company", "commit": false }),
    )["run"]
        .clone();
    let draft = run["resolution"]["draft"].as_str().unwrap();
    show_an_open_draft(&server, &run, draft);
    server.call(&format!("/v1/drafts/{draft}/approve"), json!({}));

    // The second batch has a record that fits two companies.
    for (file, source) in [
        ("batch-2-crm.csv", "crm"),
        ("batch-2-registry.csv", "registry"),
    ] {
        let rows = Rows::from_csv(companies.join(file)).unwrap();
        load(&server, source, "company", rows);
    }
    resolve(&server, "company");
    let stuck = id_of(&server, "crm", "c-117");

    let review = shown(&server, &["show", "review", "--status", "open"]);
    has(&review, "ITEM  KIND  STATUS  TYPE  RAISED BY  ABOUT");
    assert!(
        review.contains("\n1     possible_merge  open    company  run "),
        "{review}"
    );
    assert!(
        review.ends_with("  1 record without an identity, which matched 2 identities\n"),
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
    let record = shown(&server, &["show", "records", "crm:c-117"]);
    has(&record, "identity     none");
    has(&record, "review       item 1 (possible_merge, open)");
    let unassigned = shown(&server, &["show", "records", "--unassigned"]);
    assert_eq!(
        unassigned,
        "\
SOURCE AND KEY  NAME              IDENTITY
crm:c-117       Pioneer Supplies  -
1 record has no identity: `spool match` places new records in identities, and `spool show review` lists the ones a match left out
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
    assert_eq!(complaint, "error: there is no review item 9\n");

    let raised = review_item_of_a_row_taken_back(&server, &stuck, &matched);
    review_items_in_pages(&server, &raised);
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
    let bundle = answered(&server, &["show", "identities", &anvils]);
    assert_eq!(bundle["identity"]["current"], hammers.as_str());
    assert_eq!(bundle["identity"]["members"].as_array().unwrap().len(), 3);
    assert_eq!(bundle["constraints"], expected);
    let merged = shown(&server, &["show", "identities", &anvils]);
    has(&merged, &format!("merged into  {hammers}"));
    assert!(merged.contains("\nmust_link   "), "{merged}");
    let current = shown(&server, &["show", "identities", &hammers]);
    assert!(current.contains("\nmust_link   "), "{current}");
}

#[test]
fn a_mistake_or_a_server_that_does_not_answer_is_said_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());

    refuse_mistakes(&server);

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
            "error: the server at http://{address} cannot be reached: "
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

/// `spool show runs`: the list, and a match run and a load run.
fn show_runs(server: &Server) {
    let runs = shown(server, &["show", "runs"]);
    assert!(
        runs.starts_with("RUN  KIND       STATUS     ABOUT         STARTED BY  STARTED\n"),
        "{runs}"
    );
    assert!(
        runs.contains("\n2    match      succeeded  thing, rules  agent:api   "),
        "{runs}"
    );
    assert!(
        runs.contains("\n1    load       succeeded  thing         agent:api   "),
        "{runs}"
    );
    let matches = shown(server, &["show", "runs", "--kind", "match"]);
    assert_eq!(matches.lines().count(), 2, "{matches}");
    assert_eq!(
        answered(server, &["show", "runs", "--kind", "match", "--limit", "1"]),
        server.read("/v1/runs?kind=match&limit=1")
    );

    let run = shown(server, &["show", "runs", "2"]);
    has(&run, "kind           match");
    has(&run, "started by     agent:api");
    has(
        &run,
        &format!(
            "strategy       rules, version {}",
            env!("CARGO_PKG_VERSION")
        ),
    );
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
    let loaded = shown(server, &["show", "runs", "1"]);
    has(
        &loaded,
        "rows      4 received: 4 new, 0 known, 0 new versions of a known row",
    );
}

/// `spool show identities`: the list in pages, and one identity with its
/// members and history.
fn show_identities(server: &Server, one: &str, two: &str, anvils: &str) {
    let identities = shown(server, &["show", "identities"]);
    assert_eq!(identities.lines().count(), 4, "{identities}");
    assert!(
        identities
            .starts_with("IDENTITY                              NAME    MEMBERS  NAMES  UNLINKED PAIRS  CREATED AT\n"),
        "{identities}"
    );
    let page = shown(
        server,
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
    let rest = shown(server, &["show", "identities", "--after", first]);
    assert_eq!(rest.lines().count(), 3, "{rest}");
    assert!(!rest.contains(first));
    let identity = shown(server, &["show", "identities", anvils]);
    has(&identity, &format!("identity    {anvils}"));
    has(&identity, "created at  log position 1");
    has(
        &identity,
        "RECORD                                SOURCE AND KEY  SINCE",
    );
    for (id, name) in [(one, "shop:1"), (two, "shop:2")] {
        has(&identity, &format!("{id}  {name}          log position 1"));
        has(&identity, &format!("      place record {id} in {anvils}"));
    }
    has(&identity, &format!("      create thing identity {anvils}"));
    assert!(
        identity.contains("  engine  reconcile run 3  \"reconcile new records\"\n"),
        "{identity}"
    );
    let bundle = answered(server, &["show", "identities", anvils]);
    assert_eq!(
        bundle["identity"],
        server.read(&format!("/v1/identities/{anvils}"))
    );
    assert_eq!(
        bundle["history"],
        server.read(&format!("/v1/identities/{anvils}/history"))["history"]
    );
    assert_eq!(bundle["constraints"], json!([]));
}

/// `spool show records`: the list, with IDs and in pages, and one record by
/// its ID or its source and key.
fn show_records(server: &Server, one: &str, two: &str, anvils: &str) {
    // Records are listed by source and key, with the name their type's
    // label gives them. Their IDs are one option away.
    let records = shown(server, &["show", "records", "--source", "shop"]);
    let listed: Vec<&str> = records.lines().collect();
    assert_eq!(listed.len(), 5, "{records}");
    assert_eq!(listed[0], "SOURCE AND KEY  NAME    IDENTITY");
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
    let with_ids = shown(server, &["show", "records", "--source", "shop", "--ids"]);
    assert_eq!(with_ids.lines().count(), 5, "{with_ids}");
    has(
        &with_ids,
        "RECORD                                SOURCE AND KEY  NAME    IDENTITY",
    );
    has(&with_ids, &format!("{one}  shop:1  anvil  {anvils}"));
    let page = shown(server, &["show", "records", "--limit", "2"]);
    has(&page, &format!("more: add `--after {two}`"));
    let rest = shown(server, &["show", "records", "--after", two]);
    assert_eq!(rest.lines().count(), 3, "{rest}");
    assert!(rest.contains("\nshop:3  "), "{rest}");
    let none = shown(server, &["show", "records", "--unassigned"]);
    assert_eq!(none, "no records\n");
    // `--ids` is an option of the list.
    let (code, complaint) = refused(server, &["show", "records", "shop:1", "--ids"]);
    assert_eq!(code, Some(2));
    assert!(
        complaint.starts_with("error: the argument '[RECORD]' cannot be used with '--ids'\n"),
        "{complaint}"
    );

    // A record is named by its ID or by its source and key.
    let record = shown(server, &["show", "records", "shop:1"]);
    assert_eq!(record, shown(server, &["show", "records", one]));
    has(&record, &format!("record  {one}"));
    has(&record, "source       shop");
    has(&record, "key          1");
    has(&record, &format!("identity     {anvils}"));
    has(&record, "payload");
    has(&record, "  code    anvil");
    has(&record, "  colour  black");
    assert!(record.contains(" by run 1\n"), "{record}");
    let bundle = answered(server, &["show", "records", "shop:1"]);
    assert_eq!(bundle["record"], server.read(&format!("/v1/records/{one}")));
    assert_eq!(bundle["mention"], Value::Null);
    assert_eq!(bundle["review_items"], json!([]));
}

/// A draft left for review, in the list, the overview, its run and itself.
fn show_an_open_draft(server: &Server, run: &Value, draft: &str) {
    let drafts = shown(server, &["show", "drafts"]);
    has(
        &drafts,
        "DRAFT                                 TYPE     STATUS  OPERATIONS  FROM",
    );
    assert!(
        drafts.contains(&format!("\n{draft}  company  open    ")),
        "{drafts}"
    );
    assert_eq!(
        shown(server, &["show", "drafts", "--status", "rejected"]),
        "no drafts\n"
    );
    let overview = shown(server, &["show"]);
    has(
        &overview,
        &format!("  draft       {draft} waits for a decision"),
    );
    let waiting = shown(server, &["show", "runs", &run["run"].to_string()]);
    has(&waiting, "status         drafted");
    has(&waiting, &format!("draft          {draft}"));

    let one = shown(server, &["show", "drafts", draft]);
    has(&one, &format!("draft            {draft}"));
    has(&one, "status           open");
    has(&one, "written against  log position 0");
    has(&one, "operations");
    assert!(one.contains("\n  create company identity "), "{one}");
    assert!(one.contains("\n  place record "), "{one}");
    assert_eq!(
        answered(server, &["show", "drafts", draft]),
        server.read(&format!("/v1/drafts/{draft}"))
    );
}

/// A mistake in the arguments exits with 2, and what the server refuses
/// with 1, in its words.
fn refuse_mistakes(server: &Server) {
    for (arguments, expected) in [
        (
            &["show", "weather"][..],
            "unrecognized subcommand 'weather'",
        ),
        (
            &["show", "runs", "first"],
            "invalid value 'first' for '[RUN]'",
        ),
        (
            &["show", "identities", "anvil"],
            "invalid value 'anvil' for '[IDENTITY]'",
        ),
        (
            &["show", "drafts", "anvil"],
            "invalid value 'anvil' for '[DRAFT]'",
        ),
        (
            &["show", "documents", "anvil"],
            "invalid value 'anvil' for '[DOCUMENT]'",
        ),
        (
            &["show", "review", "first"],
            "invalid value 'first' for '[ITEM]'",
        ),
        (
            &["show", "records", "anvil"],
            "`anvil` does not name a record",
        ),
        (
            &["show", "runs", "--source", "shop"],
            "unexpected argument '--source'",
        ),
        (
            &["show", "runs", "1", "--kind", "match"],
            "the argument '[RUN]' cannot be used with '--kind <KIND>'",
        ),
        (&["show", "--type", "thing"], "unexpected argument '--type'"),
        (&["show", "log", "1"], "unexpected argument '1'"),
        (&["show", "runs", "1", "2"], "unexpected argument '2'"),
        (
            &["show", "runs", "--limit", "few"],
            "invalid value 'few' for '--limit <LIMIT>'",
        ),
        (&["show", "runs", "--all"], "unexpected argument '--all'"),
    ] {
        let (code, complaint) = refused(server, arguments);
        assert_eq!(code, Some(2), "{arguments:?}: {complaint}");
        assert!(complaint.contains(expected), "{arguments:?}: {complaint}");
        assert!(complaint.starts_with("error: "), "{complaint}");
    }

    // What the server refuses is said in the server's words.
    for (arguments, expected) in [
        (
            &["show", "runs", "99"][..],
            "error: there is no run 99\n".to_owned(),
        ),
        (
            &["show", "identities", "00000000-0000-7000-8000-000000000000"],
            "error: there is no identity 00000000-0000-7000-8000-000000000000\n".to_owned(),
        ),
        (
            &["show", "records", "shop:9"],
            "error: there is no record with key `9` from source `shop`\n".to_owned(),
        ),
        (
            &["show", "documents", "00000000-0000-7000-8000-000000000000"],
            "error: there is no document 00000000-0000-7000-8000-000000000000\n".to_owned(),
        ),
        (
            &["show", "drafts", "00000000-0000-7000-8000-000000000000"],
            "error: there is no draft 00000000-0000-7000-8000-000000000000\n".to_owned(),
        ),
    ] {
        let (code, complaint) = refused(server, arguments);
        assert_eq!(code, Some(1), "{arguments:?}");
        assert_eq!(complaint, expected, "{arguments:?}");
    }
    let (code, complaint) = refused(server, &["show", "runs", "--status", "asleep"]);
    assert_eq!(code, Some(2));
    assert!(
        complaint.contains("invalid value 'asleep' for '--status <STATUS>'"),
        "{complaint}"
    );
}

/// Review items listed in pages, as the other lists are. Without a limit
/// the API gives every item.
fn review_items_in_pages(server: &Server, raised: &str) {
    let every = server.read("/v1/review");
    assert!(every["items"].as_array().unwrap().len() >= 2, "{every}");
    assert_eq!(every["next"], Value::Null);
    let page = shown(server, &["show", "review", "--limit", "1"]);
    assert_eq!(page.lines().count(), 3, "{page}");
    assert!(page.contains("\n1     possible_merge  "), "{page}");
    assert!(page.ends_with("\nmore: add `--after 1`\n"), "{page}");
    let rest = shown(server, &["show", "review", "--after", "1"]);
    assert!(!rest.contains("\n1     "), "{rest}");
    assert!(rest.contains(&format!("\n{raised} ")), "{rest}");
    let answer = answered(server, &["show", "review", "--limit", "1"]);
    assert_eq!(answer, server.read("/v1/review?limit=1"));
    assert_eq!(answer["next"], 1);
}

/// The same company under a second key, held back with the first: when the
/// CRM takes that row back, the item says which of the two still waits,
/// and which identity a record went to once it is placed.
fn review_item_of_a_row_taken_back(server: &Server, stuck: &str, matched: &str) -> String {
    // The same company under a second key is held back with the first. When
    // the CRM takes that row back, the item says which of the two still
    // waits, and which identity a record went to once it is placed.
    let mut again = server.read(&format!("/v1/records/{stuck}"))["payload"].clone();
    again["id"] = json!("c-217");
    let response = server.post(
        "/v1/load",
        json!({ "source": "crm", "entity_type": "company", "rows": [again] }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
    resolve(server, "company");
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
    let item = shown(server, &["show", "review", &raised]);
    has(&item, "status       open");
    has(&item, &format!("{stuck}  crm:c-117  waits for an identity"));
    has(
        &item,
        &format!(
            "{}  crm:c-217  taken back by its source in run {taken}, so it does not wait",
            id_of(server, "crm", "c-217")
        ),
    );
    server.call(
        "/v1/changesets",
        json!({ "ops": [{ "op": "assign", "record": stuck, "from": null, "to": matched }] }),
    );
    let item = shown(server, &["show", "review", &raised]);
    has(&item, "status       resolved");
    has(&item, &format!("{stuck}  crm:c-117  in identity {matched}"));
    raised
}
