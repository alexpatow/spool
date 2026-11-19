//! What the commands print in each format, and where: the result on
//! standard output, and hints beside it on standard error.

mod support;

use std::path::Path;

use serde_json::Value;
use support::*;

/// A server over the sample project, with its first rows loaded and
/// matched.
fn matched(dir: &Path) -> (Server, std::path::PathBuf) {
    let project = init_companies(dir);
    let server = Server::dev(&project);
    for arguments in [&["load", "samples/crm-1.csv"][..], &["match"]] {
        let output = server.spool_in(&project, arguments).output().unwrap();
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    (server, project)
}

/// What `spool <arguments>` printed on each stream. It has to succeed.
fn printed(server: &Server, dir: &Path, arguments: &[&str]) -> (String, String) {
    let output = server.spool_in(dir, arguments).output().unwrap();
    assert!(
        output.status.success(),
        "spool {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    (text(&output.stdout), text(&output.stderr))
}

#[test]
fn a_list_is_a_table_json_ndjson_or_csv() {
    let dir = tempfile::tempdir().unwrap();
    let (server, project) = matched(dir.path());
    let answer = server.read("/v1/identities");
    let identities = answer["identities"].as_array().unwrap();
    assert_eq!(identities.len(), 4);

    // A table for people, its columns named in capitals.
    let (table, _) = printed(&server, &project, &["show", "identities"]);
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines.len(), 5, "{table}");
    assert!(lines[0].starts_with("IDENTITY  "), "{table}");
    assert!(lines.iter().all(|line| line == &line.trim_end()), "{table}");

    // JSON is the server's answer, by either option.
    for option in [&["--json"][..], &["-o", "json"], &["--output", "json"]] {
        let (json, beside) = printed(
            &server,
            &project,
            &[&["show", "identities"][..], option].concat(),
        );
        assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), answer);
        assert_eq!(beside, "");
    }

    // NDJSON is the items, one to a line.
    let (ndjson, _) = printed(&server, &project, &["show", "identities", "-o", "ndjson"]);
    let items: Vec<Value> = ndjson
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(&items, identities);

    // CSV is the table's columns, with the type and the whole values.
    let (csv, _) = printed(&server, &project, &["show", "identities", "-o", "csv"]);
    let mut reader = csv::Reader::from_reader(csv.as_bytes());
    let header: Vec<String> = reader
        .headers()
        .unwrap()
        .iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        header,
        [
            "identity",
            "type",
            "name",
            "members",
            "names",
            "unlinked_pairs",
            "created_at"
        ]
    );
    let rows: Vec<csv::StringRecord> = reader.records().map(Result::unwrap).collect();
    assert_eq!(rows.len(), 4);
    for (row, identity) in rows.iter().zip(identities) {
        assert_eq!(&row[0], identity["id"].as_str().unwrap());
        assert_eq!(&row[1], "company");
        assert_eq!(&row[6], identity["created"].to_string());
    }

    // The records' IDs are in CSV whether or not the table shows them.
    let (csv, _) = printed(&server, &project, &["show", "records", "-o", "csv"]);
    assert!(
        csv.starts_with("record,type,source_and_key,name,identity\n"),
        "{csv}"
    );
    assert_eq!(csv.lines().count(), 7, "{csv}");

    // An empty list is a line for people, and a header in CSV.
    let (none, _) = printed(&server, &project, &["show", "drafts"]);
    assert_eq!(none, "no drafts\n");
    let (none, _) = printed(&server, &project, &["show", "drafts", "-o", "csv"]);
    assert_eq!(none, "draft,type,status,operations,from\n");
    let (none, _) = printed(&server, &project, &["show", "drafts", "-o", "ndjson"]);
    assert_eq!(none, "");
}

#[test]
fn a_report_is_json_on_one_line_as_ndjson_and_never_csv() {
    let dir = tempfile::tempdir().unwrap();
    let (server, project) = matched(dir.path());
    let runs = server.read("/v1/runs")["runs"].as_array().unwrap().len();

    let (json, _) = printed(&server, &project, &["show", "-o", "json"]);
    let (line, _) = printed(&server, &project, &["show", "-o", "ndjson"]);
    assert_eq!(line.lines().count(), 1, "{line}");
    assert_eq!(
        serde_json::from_str::<Value>(&json).unwrap(),
        serde_json::from_str::<Value>(&line).unwrap()
    );

    // A report is refused as CSV before anything is done.
    for arguments in [
        &["match", "-o", "csv"][..],
        &["load", "samples/crm-2.csv", "-o", "csv"],
        &["show", "-o", "csv"],
        &["show", "runs", "1", "-o", "csv"],
        &["explain", "crm:c-1", "-o", "csv"],
    ] {
        let output = server.spool_in(&project, arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let said = text(&output.stderr);
        assert!(
            said.contains("is a report, not a list, so it cannot be CSV"),
            "{said}"
        );
        assert_eq!(text(&output.stdout), "");
    }
    assert_eq!(
        server.read("/v1/runs")["runs"].as_array().unwrap().len(),
        runs
    );
    let output = run(&["info", path(&project), "-o", "csv"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("`spool info` is a report"));

    // `--json` and `-o` are one choice.
    let output = server
        .spool_in(&project, &["show", "runs", "--json", "-o", "csv"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("cannot be used with"));
}

#[test]
fn hints_go_to_standard_error_and_quiet_leaves_them_out() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);

    let (result, beside) = printed(&server, &project, &["load", "samples/crm-1.csv"]);
    assert_eq!(
        result,
        "loaded  6 rows into source `crm`, type `company`: 6 new\n"
    );
    assert_eq!(beside, "next: `spool match` places them in identities\n");
    let (result, beside) = printed(&server, &project, &["show", "records", "--limit", "2"]);
    assert_eq!(result.lines().count(), 3, "{result}");
    assert!(beside.starts_with("more: add `--after "), "{beside}");

    for quiet in ["-q", "--quiet"] {
        let (result, beside) = printed(
            &server,
            &project,
            &["show", "records", "--limit", "2", quiet],
        );
        assert_eq!(result.lines().count(), 3, "{result}");
        assert_eq!(beside, "");
    }
    let (_, beside) = printed(&server, &project, &["-q", "load", "samples/crm-2.csv"]);
    assert_eq!(beside, "");
}

#[test]
fn the_local_commands_print_json_too() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("companies");
    let created = succeed(&["init", path(&project), "--example", "companies", "--json"]);
    let created: Value = serde_json::from_str(&created).unwrap();
    assert_eq!(created["created"], path(&project));
    assert!(
        created["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "samples/crm-1.csv")
    );

    let listing: Value =
        serde_json::from_str(&succeed(&["info", path(&project), "--json"])).unwrap();
    assert_eq!(listing["entity_types"][0]["name"], "company");

    let built: Value =
        serde_json::from_str(&succeed(&["build", path(&project), "-o", "json"])).unwrap();
    assert_eq!(built["entity_types"], serde_json::json!(["company"]));
    assert_eq!(built["manifest_hash"], listing["manifest_hash"]);

    let store = dir.path().join("store.duckdb");
    let applied: Value = serde_json::from_str(&succeed(&[
        "apply",
        path(&project),
        "--store",
        path(&store),
        "--json",
    ]))
    .unwrap();
    assert_eq!(applied["declared"], serde_json::json!(["company"]));
    assert_eq!(applied["already_applied"], false);

    let added: Value = serde_json::from_str(&succeed(&[
        "token",
        "add",
        "ci",
        "--store",
        path(&store),
        "--json",
    ]))
    .unwrap();
    assert_eq!(added["token"]["name"], "ci");
    assert!(added["secret"].as_str().unwrap().starts_with("spool_"));
    let listed = succeed(&["token", "list", "--store", path(&store), "-o", "csv"]);
    assert!(
        listed.starts_with("token,made_at,made_by,last_used\nci,"),
        "{listed}"
    );
    let revoked: Value = serde_json::from_str(&succeed(&[
        "token",
        "revoke",
        "ci",
        "--store",
        path(&store),
        "-o",
        "ndjson",
    ]))
    .unwrap();
    assert_eq!(revoked["name"], "ci");

    let reset: Value =
        serde_json::from_str(&succeed(&["reset", path(&project), "--yes", "--json"])).unwrap();
    assert_eq!(reset["removed"], serde_json::json!([]));
}
