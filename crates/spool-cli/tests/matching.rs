//! `spool match`, run against a server over a socket, and what `spool show
//! records` says before and after it.

mod support;

use std::net::TcpListener;
use std::path::Path;

use serde_json::{Value, json};
use support::*;

/// What `spool <arguments>` printed in `dir`. It has to succeed and say
/// nothing beside.
fn shown(server: &Server, dir: &Path, arguments: &[&str]) -> String {
    let output = server.spool_in(dir, arguments).output().unwrap();
    assert!(
        output.status.success(),
        "spool {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert_eq!(text(&output.stderr), "", "spool {arguments:?}");
    text(&output.stdout)
}

/// What `spool match <arguments>` complained of. It has to fail with status
/// 1 and print nothing.
fn refused(server: &Server, dir: &Path, arguments: &[&str]) -> String {
    let output = server
        .spool_in(dir, &[&["match"], arguments].concat())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "spool match {arguments:?}: {}",
        text(&output.stderr)
    );
    assert_eq!(text(&output.stdout), "", "nothing is printed on a failure");
    let complaint = text(&output.stderr);
    complaint
        .strip_prefix("spool: ")
        .unwrap_or_else(|| panic!("{complaint}"))
        .trim_end()
        .to_owned()
}

/// The identity of the `crm` record with a key.
fn identity_of(server: &Server, key: &str) -> String {
    server.read(&format!("/v1/records?source=crm&key={key}"))["records"][0]["identity"]
        .as_str()
        .unwrap_or_else(|| panic!("crm:{key} has no identity"))
        .to_owned()
}

fn match_runs(server: &Server) -> Vec<Value> {
    server.read("/v1/runs?kind=match")["runs"]
        .as_array()
        .unwrap()
        .clone()
}

/// The first steps of a new project, with what each prints: the text the
/// README and the site show.
#[test]
fn the_first_rows_of_a_new_project_are_loaded_matched_and_shown() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);
    let shown = |arguments: &[&str]| shown(&server, &project, arguments);

    // Before any rows, each command says which one comes first.
    assert_eq!(
        shown(&["match"]),
        "\
nothing to match: there are no `company` records yet
`spool load <file>` sends rows
"
    );
    assert_eq!(
        shown(&["show", "records"]),
        "no records: `spool load <file>` sends rows\n"
    );
    assert_eq!(match_runs(&server), Vec::<Value>::new());

    assert_eq!(
        shown(&["load", "samples/crm-1.csv"]),
        "\
loaded  6 rows into source `crm`, type `company`: 6 new
next    `spool match` places them in identities
"
    );
    // Loaded and not matched: the list says what is missing.
    assert_eq!(
        shown(&["show", "records"]),
        "\
source and key  name                 identity
crm:c-1         Acorn Tools          -
crm:c-2         ACORN TOOLS          -
crm:c-3         Acorn Tools Ltd      -
crm:c-4         Larkfield Logistics  -
crm:c-5         Halden Group         -
crm:c-6         Halden Group         -

6 records have no identity. `spool match` places new records in identities,
and `spool show review` lists the ones a match left out.
"
    );

    assert_eq!(
        shown(&["match"]),
        "\
run         2, type `company`
examined    6 records, 4 candidate pairs
judgements  3 match, 1 no_match
placed      6 records, 4 identities created
"
    );
    let acorn = identity_of(&server, "c-1");
    let larkfield = identity_of(&server, "c-4");
    let halden = identity_of(&server, "c-5");
    let other_halden = identity_of(&server, "c-6");
    let first = format!(
        "\
source and key  name                 identity
crm:c-1         Acorn Tools          {acorn}
crm:c-2         ACORN TOOLS          {acorn}
crm:c-3         Acorn Tools Ltd      {acorn}
crm:c-4         Larkfield Logistics  {larkfield}
crm:c-5         Halden Group         {halden}
crm:c-6         Halden Group         {other_halden}
"
    );
    assert_eq!(shown(&["show", "records"]), first);
    assert_ne!(halden, other_halden, "two countries, two companies");
    assert!(
        first.lines().all(|line| line.chars().count() <= 100),
        "the list fits a page that is 100 characters wide"
    );

    // A second match has nothing new to look at, which is no failure.
    assert_eq!(
        shown(&["match"]),
        "\
nothing to match: no `company` record is new since the last match
`spool load <file>` sends more rows, and `spool match --rematch` examines them all again
"
    );
    assert_eq!(shown(&["match", "--json"]), "{\n  \"run\": null\n}\n");
    assert_eq!(match_runs(&server).len(), 1);

    // More rows: one for a company that is new, and two that join
    // identities, which keep their IDs.
    assert_eq!(
        shown(&["load", "samples/crm-2.csv"]),
        "\
loaded  3 rows into source `crm`, type `company`: 3 new
next    `spool match` places them in identities
"
    );
    assert_eq!(
        shown(&["match"]),
        "\
run         5, type `company`
examined    3 records, 4 candidate pairs
judgements  4 match, 0 no_match
placed      3 records, 1 identity created
"
    );
    let kestrel = identity_of(&server, "c-8");
    assert_eq!(
        shown(&["show", "records"]),
        format!(
            "\
{first}\
crm:c-7         Larkfeild Logistics  {larkfield}
crm:c-8         Kestrel Analytics    {kestrel}
crm:c-9         Acorn Tool           {acorn}
"
        )
    );

    // The record IDs are one option away, and the identities have the name
    // of one of their records.
    let with_ids = shown(&["show", "records", "--ids"]);
    assert!(
        with_ids.starts_with(&format!(
            "\
record                                source and key  name                 identity
ab8e9d59-0aaa-96b9-ddb7-db0bff7a9356  crm:c-1         Acorn Tools          {acorn}
"
        )),
        "{with_ids}"
    );
    let identities = shown(&["show", "identities"]);
    assert!(
        identities.starts_with(
            "identity                              name                 members  created at\n"
        ),
        "{identities}"
    );
    assert!(
        identities.contains(&format!(
            "\n{larkfield}  Larkfield Logistics  2        log position 1\n"
        )),
        "{identities}"
    );
    assert!(
        identities.contains(&format!(
            "\n{kestrel}  Kestrel Analytics    1        log position 2\n"
        )),
        "{identities}"
    );

    // Every record again: the rules still say what the identities say.
    assert_eq!(
        shown(&["match", "--rematch"]),
        "\
run         7, type `company`
examined    0 records without an identity and every record placed before, 8 candidate pairs
judgements  7 match, 1 no_match
placed      0 records, 0 identities created
suggested   nothing: the rules agree with the identities
"
    );
    assert_eq!(identity_of(&server, "c-9"), acorn);
}

#[test]
fn match_says_what_it_left_out_and_where_to_look() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);
    let shown = |arguments: &[&str]| shown(&server, &project, arguments);
    shown(&["load", "samples/crm-1.csv"]);
    shown(&["match"]);

    // A Halden Group with no country fits the one in each country, and the
    // rules keep those two apart: it is not for a match to choose.
    std::fs::write(
        project.join("unclear.csv"),
        "id,name,country\nc-7,Halden Group,\n",
    )
    .unwrap();
    assert_eq!(
        shown(&["load", "unclear.csv"]),
        "\
loaded  1 row into source `crm`, type `company`: 1 new
next    `spool match` places it in an identity
"
    );
    assert_eq!(
        shown(&["match"]),
        "\
run         5, type `company`
examined    1 record, 2 candidate pairs
judgements  2 match, 0 no_match
placed      0 records, 0 identities created
left out    1 record in 1 review item: `spool show review` says why
"
    );
    let review = shown(&["show", "review"]);
    assert!(
        review.contains("\n1     possible_merge  open    company  1 record  2 identities  run "),
        "{review}"
    );
    let records = shown(&["show", "records"]);
    assert!(
        records.ends_with(
            "\
crm:c-7         Halden Group         -

1 record has no identity. `spool match` places new records in identities,
and `spool show review` lists the ones a match left out.
"
        ),
        "{records}"
    );

    // Each match looks at it again, until a reviewer decides.
    assert_eq!(
        shown(&["match"]),
        "\
run         7, type `company`
examined    1 record (1 of them left for review before), 2 candidate pairs
judgements  2 match, 0 no_match
placed      0 records, 0 identities created
left out    1 record in 1 review item: `spool show review` says why
"
    );
    assert_eq!(
        shown(&["match", "--rematch"]),
        "\
run         9, type `company`
examined    1 record without an identity (1 of them left for review before) and every record \
         placed before, 6 candidate pairs
judgements  5 match, 1 no_match
placed      0 records, 0 identities created
left out    1 record in 1 review item: `spool show review` says why
suggested   nothing: the rules agree with the identities
"
    );
}

#[test]
fn match_prints_the_run_as_json_and_takes_a_strategy() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);
    let shown = |arguments: &[&str]| shown(&server, &project, arguments);
    shown(&["load", "samples/crm-1.csv"]);

    // A strategy the type does not have is refused before a run starts:
    // the server would take the name for a caller that judges by itself.
    assert_eq!(
        refused(&server, &project, &["--strategy", "ml"]),
        "`company` has no strategy `ml`. It has `rules`."
    );
    assert_eq!(match_runs(&server), Vec::<Value>::new());

    let printed = shown(&["match", "--strategy", "rules", "--json"]);
    let answer: Value = serde_json::from_str(&printed).unwrap();
    let run = &answer["run"];
    assert_eq!(run["kind"], "match");
    assert_eq!(run["status"], "succeeded");
    assert_eq!(run["strategy"]["name"], "rules");
    assert_eq!(
        run["resolution"]["judgements"],
        json!({ "match": 3, "no_match": 1, "unsure": 0 })
    );
    assert_eq!(
        *run,
        server.read(&format!("/v1/runs/{}", run["run"]))["run"]
    );

    // The type by name is the type it would have taken.
    shown(&["load", "samples/crm-2.csv"]);
    let printed = shown(&["match", "--type", "company"]);
    assert!(
        printed.starts_with("run         5, type `company`\n"),
        "{printed}"
    );
}

#[test]
fn match_says_what_is_wrong_with_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_two_of_each(&project);
    let server = Server::serve(&project, dir.path(), 1);

    assert_eq!(
        refused(&server, &project, &[]),
        "say which entity type to match with `--type`: the project has `company`, `person`"
    );
    assert_eq!(
        refused(&server, &project, &["--type", "place"]),
        "the project has no entity type `place`. It has `company`, `person`."
    );
    assert_eq!(
        text(
            &server
                .spool_in(&project, &["match", "--type", "person"])
                .output()
                .unwrap()
                .stdout
        ),
        "\
nothing to match: there are no `person` records yet
`spool load <file>` sends rows
"
    );

    // A file is `spool load`'s, and an option it does not have is named.
    for (arguments, complaint) in [
        (
            &["match", "samples/crm-1.csv"][..],
            "spool: unexpected argument `samples/crm-1.csv`\n\nUsage: spool match [options]\n",
        ),
        (
            &["match", "--all"],
            "spool: unknown option `--all`\n\nUsage: spool match [options]\n",
        ),
    ] {
        let output = server.spool_in(&project, arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(
            text(&output.stderr).starts_with(complaint),
            "{}",
            text(&output.stderr)
        );
    }

    // Without the token the server asks for.
    let url = format!("http://{}", server.address);
    let output = spool()
        .args(["match", "--type", "person", "--url", &url])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).starts_with(&format!("spool: the server at {url} asks for a token: ")),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn match_says_when_no_server_answers() {
    // A port nothing listens on: the one a listener had a moment ago.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let output = spool().args(["match", "--url", &url]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), "");
    let complaint = text(&output.stderr);
    assert!(
        complaint.starts_with(&format!("spool: the server at {url} cannot be reached: ")),
        "{complaint}"
    );
    assert!(
        complaint.ends_with(
            "\nStart one with `spool dev`, or name another with `--url` or SPOOL_URL.\n"
        ),
        "{complaint}"
    );
}
