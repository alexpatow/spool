//! `spool recompute` and `POST /v1/recompute`, against a `spool dev` server
//! whose rules change under identities that exist.

use std::path::Path;

use serde_json::{Value, json};

use crate::support::*;

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

fn failed(server: &Server, dir: &Path, arguments: &[&str], code: i32) -> String {
    let output = server.spool_in(dir, arguments).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "spool {arguments:?}: {}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    text(&output.stderr)
}

fn record(server: &Server, key: &str) -> Value {
    server.read(&format!("/v1/records?source=crm&key={key}"))["records"][0].clone()
}

/// The companies example, matched in three loads, with a `cannot_link`
/// between the two Halden Groups in GB that the looser rule would join,
/// and then a looser match rule and a no_match rule for a legal suffix.
fn changed_rules(dir: &Path) -> (Server, std::path::PathBuf) {
    let project = init_companies(dir);
    let server = Server::dev(&project);
    let shown = |arguments: &[&str]| shown(&server, &project, arguments);
    shown(&["load", "samples/crm-1.csv"]);
    shown(&["match"]);
    shown(&["load", "samples/crm-2.csv"]);
    shown(&["match"]);
    std::fs::write(
        project.join("halden.csv"),
        "id,name,country\nc-10,Halden,GB\n",
    )
    .unwrap();
    shown(&["load", "halden.csv", "--source", "crm"]);
    shown(&["match"]);

    assert_eq!(
        shown(&["recompute"]).lines().last(),
        Some("nothing would change: the identities are what the rules say they are")
    );

    let a = record(&server, "c-5")["id"].clone();
    let b = record(&server, "c-10")["id"].clone();
    server.call(
        "/v1/changesets",
        json!({
            "reason": "the Leeds Halden is not the Halden Group",
            "ops": [{"op": "constrain", "kind": "cannot_link", "a": a, "b": b}],
        }),
    );
    std::fs::write(
        project.join("entities/company/rules/match/similar_name.sql"),
        "jaro_winkler_similarity(l.name_key, r.name_key) >= 0.85\n",
    )
    .unwrap();
    server.printed("applied");
    std::fs::write(
        project.join("entities/company/rules/no_match/suffix.sql"),
        "(l.name_key LIKE '% ltd') <> (r.name_key LIKE '% ltd')\n",
    )
    .unwrap();
    server.printed("applied");
    (server, project)
}

#[test]
fn a_rule_change_is_shown_then_applied() {
    let dir = tempfile::tempdir().unwrap();
    let (server, project) = changed_rules(dir.path());
    let shown = |arguments: &[&str]| shown(&server, &project, arguments);
    let acorn = record(&server, "c-1")["identity"]
        .as_str()
        .unwrap()
        .to_owned();

    let before = server.read("/v1/changesets");
    let printed = shown(&["recompute"]);
    let plan = printed
        .lines()
        .last()
        .unwrap()
        .strip_prefix("spool recompute --apply ")
        .unwrap()
        .to_owned();
    assert_eq!(plan.len(), 64);
    let expected = format!(
        "\
type `company`: 10 records, in 6 identities now and 7 after
unchanged  5 identities would keep exactly their records
split      the records of 1 identity would go to more than one identity
lose       1 identity would lose records
new        1 identity would be made
moved      1 record would change identity
pairs      7 pairs share an identity now, 4 would after, 4 of them both

decisions
  constraints  0 must_links and 1 cannot_link in force, each holding whatever the rules say

1 change, the largest first
#  KIND   RECORDS  IDENTITY                              NOW                  AFTER
1  split  4        {acorn}  Acorn Tools Ltd (4)  ACORN TOOLS (3, keeps its ID), Acorn Tools Ltd (1, new)
`--identity <id>` shows a change record by record
nothing was changed. This applies it:
spool recompute --apply {plan}
"
    );
    assert_eq!(printed, expected);
    assert_eq!(
        server.read("/v1/changesets"),
        before,
        "a preview changes nothing"
    );

    // The change record by record.
    let detail = shown(&["recompute", "--identity", &acorn]);
    assert!(detail.contains("kind     split\n"), "{detail}");
    assert!(
        detail.contains("crm:c-3         Acorn Tools Ltd"),
        "{detail}"
    );
    let kept = detail
        .lines()
        .find(|line| line.starts_with(&format!("  1  {acorn}, keeps its ID")))
        .unwrap_or_else(|| panic!("{detail}"));
    assert!(
        kept.contains("ACORN TOOLS") && kept.ends_with("3 records"),
        "{kept}"
    );
    assert!(detail.contains("  2  a new identity"), "{detail}");

    // The same facts as JSON.
    let report: Value = serde_json::from_str(&shown(&["recompute", "--json"])).unwrap();
    assert_eq!(report["plan"], plan.as_str());
    assert_eq!(report["types"][0]["changes"][0]["kind"], "split");
    assert_eq!(report["types"][0]["summary"]["records_moved"], 1);

    // Applying refuses a plan that is not what it finds.
    let refused = failed(
        &server,
        &project,
        &[
            "recompute",
            "--apply",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ],
        1,
    );
    assert!(
        refused.contains("is not what recomputing finds now"),
        "{refused}"
    );
    assert_eq!(server.read("/v1/changesets"), before);

    let applied = shown(&["recompute", "--apply", &plan, "--reason", "suffixes"]);
    assert!(applied.contains("changeset   at log position"), "{applied}");
    assert!(
        applied.ends_with(&format!("applied plan {plan}\n")),
        "{applied}"
    );
    let run = server.read("/v1/runs?kind=match")["runs"][0].clone();
    assert_eq!(run["recompute"]["summary"]["records_moved"], 1);
    let run = shown(&["show", "runs", &run["run"].to_string()]);
    assert!(
        run.contains("recompute      every record judged again and applied"),
        "{run}"
    );
    assert_eq!(record(&server, "c-1")["identity"], acorn.as_str());
    assert_ne!(record(&server, "c-3")["identity"], acorn.as_str());
    // The `cannot_link` held: the Haldens stay apart.
    assert_ne!(
        record(&server, "c-5")["identity"],
        record(&server, "c-10")["identity"]
    );
    assert_eq!(
        shown(&["recompute"]).lines().last(),
        Some("nothing would change: the identities are what the rules say they are")
    );
    let log = server.read("/v1/changesets?limit=1000");
    let last = log["changesets"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["changeset"]["actor"], "engine");
    assert!(
        last["changeset"]["reason"]
            .as_str()
            .unwrap()
            .contains("suffixes")
    );
    // The engine decided it, and the person who applied it started the run.
    assert!(last["started_by"].as_str().unwrap().starts_with("user:"));
}

#[test]
fn the_api_pages_the_changes_and_refuses_what_it_cannot_do() {
    let dir = tempfile::tempdir().unwrap();
    let (server, project) = changed_rules(dir.path());

    let report = server.call("/v1/recompute", json!({ "limit": 1, "offset": 1 }));
    assert_eq!(report["types"][0]["changes_total"], 1);
    assert_eq!(report["types"][0]["changes"], json!([]));

    let unknown = server.post("/v1/recompute", json!({ "entity_type": "widget" }));
    assert_eq!(unknown.status, 400);
    let stale = server.post("/v1/recompute/apply", json!({ "plan": "0" }));
    assert_eq!(stale.status, 409);
    assert_eq!(stale.json()["error"]["code"], "conflict");
    let wrong = server.post("/v1/recompute", json!({ "plan": "0" }));
    assert_eq!(wrong.status, 400);

    let usage = failed(&server, &project, &["recompute", "--reason", "why"], 2);
    assert!(usage.contains("--apply <PLAN>"), "{usage}");
    let usage = failed(&server, &project, &["recompute", "--identity", "acorn"], 2);
    assert!(
        usage.contains("invalid value 'acorn' for '--identity <ID>'"),
        "{usage}"
    );
}

#[test]
fn the_help_says_what_it_does() {
    let help = succeed(&["recompute", "--help"]);
    assert!(help.contains("\nUsage: spool recompute [OPTIONS]\n"));
    assert!(help.contains("--apply <PLAN>"));
    assert!(succeed(&["--help"]).contains("\n  recompute  "));
}
