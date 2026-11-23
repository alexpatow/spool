//! What a reading takes back that no extractor finds any more.

use super::*;

/// A mention a changed extractor no longer finds is taken back: the
/// command says so, the record says why it has no identity, and a record
/// says which versions of which extractor found it.
#[test]
#[ignore = "runs uv"]
fn a_mention_no_extractor_finds_any_more_is_taken_back() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_with_stand_in(dir.path());
    let server = Server::dev(&project);
    assert_eq!(
        extracted(&server, &project, &["samples/notes-1.txt"]),
        FIRST
    );
    shown(&server, &project, &["match"]);

    // Now it finds what ends in `Ltd` only when it is not Brightwater.
    drop(server);
    write(
        &project.join("extractors/companies.py"),
        &BY_LEGAL_FORM.replacen(
            "        yield document.mention",
            "        if found.group().startswith(\"Brightwater\"):\n            continue\n        yield document.mention",
            1,
        ),
    );
    let server = Server::dev(&project);
    let again = extracted(&server, &project, &["--again"]);
    assert!(
        again.contains(
            "found      2 records in 1 document: 0 new, 2 already held\n\
             taken back 1 record: no extractor finds it any more\n"
        ),
        "{again}"
    );
    let records = found(&server);
    let taken = records
        .iter()
        .find(|record| record["payload"]["name"] == "Brightwater Foods Ltd")
        .unwrap();
    let shown_taken = shown(
        &server,
        &project,
        &["show", "records", taken["id"].as_str().unwrap()],
    );
    assert!(
        shown_taken.contains("identity  none: taken back in run "),
        "{shown_taken}"
    );
    assert!(
        shown_taken.contains(", as no extractor finds it any more\n"),
        "{shown_taken}"
    );
    let kept = records
        .iter()
        .find(|record| record["payload"]["name"] == "Acorn Tools Ltd")
        .unwrap();
    assert_eq!(kept["found_by"].as_array().unwrap().len(), 2, "{kept}");
    assert_ne!(
        kept["found_by"][0]["version"],
        kept["found_by"][1]["version"]
    );
    let shown_kept = shown(
        &server,
        &project,
        &["show", "records", kept["id"].as_str().unwrap()],
    );
    assert!(
        shown_kept.contains("found by  companies version "),
        "{shown_kept}"
    );
}

/// An extractor that finds nothing at all takes back what was found
/// before, as a broken one would, and the command says so out loud.
#[test]
#[ignore = "runs uv"]
fn a_reading_that_finds_nothing_at_all_says_what_it_took_back() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_with_stand_in(dir.path());
    let server = Server::dev(&project);
    assert_eq!(
        extracted(&server, &project, &["samples/notes-1.txt"]),
        FIRST
    );

    drop(server);
    write(
        &project.join("extractors/companies.py"),
        "from spool import extractor\n\n\n@extractor\ndef find(document):\n    return []\n",
    );
    let server = Server::dev(&project);
    let output = server
        .spool_in(&project, &["extract", "--again"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let run = runs(&server, "extract")
        .into_iter()
        .map(|run| run["run"].as_u64().unwrap())
        .max()
        .unwrap();
    assert_eq!(
        text(&output.stderr),
        format!(
            "warning: took back 3 records that run {run} found nowhere, though it found nothing \
             at all: if the extractor is wrong, mend it and run `spool extract --again`, and what \
             it finds again goes back to the identity it left\n"
        )
    );
    assert!(
        text(&output.stdout).contains("taken back 3 records: no extractor finds them any more"),
        "{}",
        text(&output.stdout)
    );
}
