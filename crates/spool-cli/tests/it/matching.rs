//! `spool match`, run against a server over a socket, and what `spool show
//! records` says before and after it.

use std::net::TcpListener;
use std::path::Path;

use serde_json::{Value, json};

use crate::support::*;

/// What `spool <arguments>` printed in `dir`: its result, then what it said
/// beside on standard error. It has to succeed.
fn shown(server: &Server, dir: &Path, arguments: &[&str]) -> String {
    let output = server.spool_in(dir, arguments).output().unwrap();
    assert!(
        output.status.success(),
        "spool {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    format!("{}{}", text(&output.stdout), text(&output.stderr))
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
        .strip_prefix("error: ")
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
#[expect(clippy::too_many_lines)]
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
        "no records\n`spool load <file>` sends rows\n"
    );
    assert_eq!(match_runs(&server), Vec::<Value>::new());

    assert_eq!(
        shown(&["load", "samples/crm-1.csv"]),
        "\
loaded  6 rows into source `crm`, type `company`: 6 new
next: `spool match` places them in identities
"
    );
    // Loaded and not matched: the list says what is missing.
    assert_eq!(
        shown(&["show", "records"]),
        "\
SOURCE AND KEY  NAME                 IDENTITY
crm:c-1         Acorn Tools          -
crm:c-2         ACORN TOOLS          -
crm:c-3         Acorn Tools Ltd      -
crm:c-4         Larkfield Logistics  -
crm:c-5         Halden Group         -
crm:c-6         Halden Group         -
6 records have no identity: `spool match` places new records in identities, and `spool show review` lists the ones a match left out
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
SOURCE AND KEY  NAME                 IDENTITY
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
    assert_eq!(
        shown(&["match", "--type", "company", "--json"]),
        "{\n  \"run\": null\n}\n"
    );
    let answer: Value = serde_json::from_str(&shown(&["match", "--json"])).unwrap();
    assert_eq!(
        answer,
        json!({ "runs": [{ "entity_type": "company", "run": null }] })
    );
    assert_eq!(match_runs(&server).len(), 1);

    // More rows: one for a company that is new, and two that join
    // identities, which keep their IDs.
    assert_eq!(
        shown(&["load", "samples/crm-2.csv"]),
        "\
loaded  3 rows into source `crm`, type `company`: 3 new
next: `spool match` places them in identities
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
RECORD                                SOURCE AND KEY  NAME                 IDENTITY
ab8e9d59-0aaa-96b9-ddb7-db0bff7a9356  crm:c-1         Acorn Tools          {acorn}
"
        )),
        "{with_ids}"
    );
    let identities = shown(&["show", "identities"]);
    assert!(
        identities.starts_with(
            "IDENTITY                              NAME                 MEMBERS  NAMES  UNLINKED PAIRS  CREATED AT\n"
        ),
        "{identities}"
    );
    assert!(
        identities.contains(&format!(
            "\n{larkfield}  Larkfield Logistics  2        2      0%              log position 1\n"
        )),
        "{identities}"
    );
    assert!(
        identities.contains(&format!(
            "\n{kestrel}  Kestrel Analytics    1        1      -               log position 2\n"
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
next: `spool match` places it in an identity
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
        review.contains("\n1     possible_merge  open    company  run 6      1 record without an identity, which matched 2 identities\n"),
        "{review}"
    );
    let records = shown(&["show", "records"]);
    assert!(
        records.ends_with(
            "\
crm:c-7         Halden Group         -
1 record has no identity: `spool match` places new records in identities, and `spool show review` lists the ones a match left out
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

    let printed = shown(&[
        "match",
        "--type",
        "company",
        "--strategy",
        "rules",
        "--json",
    ]);
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

    // With no type named, each type is matched, and says why it had
    // nothing to match.
    assert_eq!(
        shown(&server, &project, &["match"]),
        "\
nothing to match: there are no `company` records yet
nothing to match: there are no `person` records yet
`spool load <file>` sends rows
"
    );
    assert_eq!(
        refused(&server, &project, &["--strategy", "ml"]),
        "a strategy judges one entity type: say which `ml` is for with `--type`. The project \
         has `company`, `person`."
    );
    assert_eq!(
        refused(&server, &project, &["--type", "place"]),
        "the project has no entity type `place`. It has `company`, `person`."
    );
    let output = server
        .spool_in(&project, &["match", "--type", "person"])
        .output()
        .unwrap();
    assert_eq!(text(&output.stderr), "`spool load <file>` sends rows\n");
    assert_eq!(
        text(&output.stdout),
        "\
nothing to match: there are no `person` records yet
"
    );

    // A file is `spool load`'s, and an option it does not have is named.
    for (arguments, complaint) in [
        (
            &["match", "samples/crm-1.csv"][..],
            "error: unexpected argument 'samples/crm-1.csv' found\n\nUsage: spool match [OPTIONS]\n",
        ),
        (
            &["match", "--all"],
            "error: unexpected argument '--all' found\n\nUsage: spool match [OPTIONS]\n",
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
        text(&output.stderr).starts_with(&format!(
            "error: the server at {url} asks for a token\nhint: pass one with `--token`"
        )),
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
        complaint.starts_with(&format!("error: the server at {url} cannot be reached: ")),
        "{complaint}"
    );
    assert!(
        complaint.ends_with(
            "\nhint: start one with `spool dev`, or name another with `--url` or SPOOL_URL\n"
        ),
        "{complaint}"
    );
}

#[test]
fn match_matches_every_type_and_reports_each() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_two_of_each(&project);
    let server = Server::serve(&project, dir.path(), 1);
    let shown = |arguments: &[&str]| shown(&server, &project, arguments);
    std::fs::write(
        project.join("registry.csv"),
        "id,name\nr-1,Acorn\nr-2,Acorn\nr-3,Kestrel\n",
    )
    .unwrap();
    std::fs::write(
        project.join("crm.csv"),
        "id,name\nc-1,Dana\nc-2,Dana\nc-3,Ravi\n",
    )
    .unwrap();
    shown(&["load", "registry.csv"]);
    shown(&["load", "crm.csv", "--type", "person"]);

    // One run for each type, one after the other, each with its report.
    let printed = shown(&["match"]);
    let reports: Vec<&str> = printed.split("\n\n").collect();
    assert_eq!(reports.len(), 2, "{printed}");
    assert!(reports[0].starts_with("run         "), "{printed}");
    assert!(
        reports[0]
            .lines()
            .next()
            .unwrap()
            .ends_with(", type `company`"),
        "{printed}"
    );
    assert!(
        reports[1]
            .lines()
            .next()
            .unwrap()
            .ends_with(", type `person`"),
        "{printed}"
    );
    for report in &reports {
        assert!(
            report.contains("placed      3 records, 2 identities created"),
            "{printed}"
        );
    }
    let runs = match_runs(&server);
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|run| run["status"] == "succeeded"));

    // A type with nothing new says so, and the others are still matched.
    std::fs::write(project.join("people.csv"), "id,name\nc-4,Mira\n").unwrap();
    shown(&["load", "people.csv", "--source", "crm", "--type", "person"]);
    let printed = shown(&["match"]);
    assert!(
        printed.starts_with(
            "nothing to match: no `company` record is new since the last match\n\nrun         "
        ),
        "{printed}"
    );
    assert!(printed.contains(", type `person`\n"), "{printed}");
    assert!(printed.contains("placed      1 record"), "{printed}");
    assert!(
        printed.ends_with(
            "`spool load <file>` sends more rows, and `spool match --rematch` examines them all \
             again\n"
        ),
        "{printed}"
    );

    // As JSON, each type with its run, or none.
    let answer: Value = serde_json::from_str(&shown(&["match", "--json"])).unwrap();
    assert_eq!(
        answer,
        json!({ "runs": [
            { "entity_type": "company", "run": null },
            { "entity_type": "person", "run": null },
        ] })
    );
    let answer: Value = serde_json::from_str(&shown(&["match", "--rematch", "--json"])).unwrap();
    let runs = answer["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2, "{answer}");
    assert_eq!(runs[0]["entity_type"], "company");
    assert_eq!(runs[0]["run"]["entity_type"], "company");
    assert_eq!(runs[1]["run"]["entity_type"], "person");
    assert_eq!(runs[1]["run"]["status"], "succeeded");
}

#[test]
fn a_type_whose_match_fails_does_not_stop_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_two_of_each(&project);
    // A rule that cannot be worked out for these names: the run fails as
    // soon as it compares two of them, well within the wait.
    std::fs::write(
        project.join("entities/company/rules/match/same_name.sql"),
        "CAST(l.name AS INTEGER) = CAST(r.name AS INTEGER)\n",
    )
    .unwrap();
    let server = Server::serve(&project, dir.path(), 1);
    std::fs::write(
        project.join("registry.csv"),
        "id,name\nr-1,Acorn\nr-2,Acorn\n",
    )
    .unwrap();
    std::fs::write(project.join("crm.csv"), "id,name\nc-1,Dana\nc-2,Dana\n").unwrap();
    shown(&server, &project, &["load", "registry.csv"]);
    shown(&server, &project, &["load", "crm.csv", "--type", "person"]);

    let output = server.spool_in(&project, &["match"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let printed = text(&output.stdout);
    assert!(
        printed.starts_with(
            "failed      type `company`: the match did not run to the end\n\nrun         "
        ),
        "{printed}"
    );
    assert!(printed.contains(", type `person`\n"), "{printed}");
    assert!(
        printed.contains("placed      2 records, 1 identity created"),
        "{printed}"
    );
    let complaint = text(&output.stderr);
    assert!(
        complaint.starts_with("error: type `company`: "),
        "{complaint}"
    );
    assert!(complaint.contains("INTEGER"), "{complaint}");

    // As JSON, the failed type with its error, and the others with their
    // runs.
    std::fs::write(project.join("people.csv"), "id,name\nc-3,Ravi\n").unwrap();
    shown(
        &server,
        &project,
        &["load", "people.csv", "--source", "crm", "--type", "person"],
    );
    let output = server
        .spool_in(&project, &["match", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let answer: Value = serde_json::from_str(&text(&output.stdout)).unwrap();
    let runs = answer["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2, "{answer}");
    assert_eq!(runs[0]["entity_type"], "company");
    assert_eq!(runs[0]["run"], Value::Null);
    assert!(
        runs[0]["error"].as_str().unwrap().contains("INTEGER"),
        "{answer}"
    );
    assert_eq!(runs[1]["entity_type"], "person");
    assert_eq!(runs[1]["run"]["status"], "succeeded");
}
