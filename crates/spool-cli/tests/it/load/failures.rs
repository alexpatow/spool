//! What `spool load` says is wrong: with the file, and with the server.

use super::*;

#[test]
fn load_says_what_is_wrong_with_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);
    let refused = |arguments: &[&str]| refused(&server, &project, arguments);

    assert_eq!(
        refused(&["samples/crm-3.csv"]),
        "there is no file `samples/crm-3.csv`"
    );
    assert_eq!(
        refused(&["samples"]),
        "`samples` is a directory: name a file of rows"
    );

    write(&project.join("empty.csv"), "");
    assert_eq!(
        refused(&["empty.csv"]),
        "empty.csv cannot be read as CSV: it is empty: the first line has to name the columns"
    );
    write(
        &project.join("ragged.csv"),
        "id,name,country\nc-1,Acorn Tools,GB\nc-2,Birch\n",
    );
    assert_eq!(
        refused(&["ragged.csv"]),
        "ragged.csv cannot be read as CSV: line 3 has 2 fields, and the first line names 3 \
         columns"
    );
    std::fs::write(
        project.join("latin.csv"),
        b"id,name,country\nc-1,Caf\xe9 Ltd,FR\n",
    )
    .unwrap();
    assert_eq!(
        refused(&["latin.csv"]),
        "latin.csv cannot be read as CSV: line 2: the text is not UTF-8. Save the file as UTF-8 \
         and send it again"
    );
    write(
        &project.join("ragged.tsv"),
        "id\tname\nc-1\tAcorn Tools\textra\n",
    );
    assert_eq!(
        refused(&["ragged.tsv"]),
        "ragged.tsv cannot be read as tab-separated text: line 2 has 3 fields, and the first \
         line names 2 columns"
    );

    // A column the source does not list, which is what a file of one's own
    // has first.
    write(
        &project.join("mine.csv"),
        "id,name,email\nc-1,Acorn Tools,hello@acorn.example\n",
    );
    assert_eq!(
        refused(&["mine.csv"]),
        "mine.csv has a column `email`, which sources/crm.yaml does not list. It lists `id`, \
         `name`, `country`. Add the column to that file to keep it, or leave it out of the rows."
    );
    write(
        &project.join("mine.csv"),
        "ID,name,country,email\nc-1,Acorn Tools,GB,hello@acorn.example\n",
    );
    assert_eq!(
        refused(&["mine.csv"]),
        "mine.csv has the columns `ID`, `email`, which sources/crm.yaml does not list. It lists \
         `id`, `name`, `country`. Add them to that file to keep them, or leave them out of the \
         rows."
    );

    // None of that started a run or stored a row.
    assert_eq!(load_runs(&server), Vec::<Value>::new());
    assert_eq!(records(&server, "crm"), Vec::new());

    // A file with the first line alone has nothing to load, and that is no
    // failure: an export can be empty.
    write(&project.join("none.csv"), "id,name,country\n");
    let (printed, _) = load(&server, &project, &["none.csv"]);
    assert_eq!(
        printed,
        "loaded  0 rows: none.csv has no rows after its first line\n"
    );
    assert_eq!(load_runs(&server), Vec::<Value>::new());

    // Rows without the column that is the source's key are kept, and the
    // reader is told what they lack.
    write(
        &project.join("keyless.csv"),
        "name,country\nAcorn Tools,GB\n",
    );
    let (printed, beside) = load(&server, &project, &["keyless.csv"]);
    assert!(
        printed.starts_with("loaded  1 row into source `crm`, type `company`: 1 new\n"),
        "{printed}"
    );
    assert_eq!(
        beside,
        "note: keyless.csv has no column `id`, which is the key of source `crm`. Its rows are \
         kept without a key: none can be named as crm:<key>, and a changed row does not replace \
         the one loaded before.\nnext: `spool match` places it in an identity\n"
    );
    assert_eq!(records(&server, "crm")[0].0, Value::Null);

    // No file at all is a mistake in the command.
    let output = server.spool_in(&project, &["load"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr)
            .contains("required arguments were not provided:\n  <FILE>\n\nUsage: spool load"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn load_says_when_no_server_answers_or_it_asks_for_a_token() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());

    // A port nothing listens on: the one a listener had a moment ago.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}");
    let output = spool()
        .current_dir(&project)
        .args(["load", "samples/crm-1.csv", "--url", &url])
        .output()
        .unwrap();
    let complaint = failure(&output, &["samples/crm-1.csv"]);
    assert!(
        complaint.starts_with(&format!("the server at {url} cannot be reached: ")),
        "{complaint}"
    );
    assert!(
        complaint.ends_with(
            "\nhint: start one with `spool dev`, or name another with `--url` or SPOOL_URL"
        ),
        "{complaint}"
    );

    // What is wrong with the file is said first: it needs no server.
    let output = spool()
        .current_dir(&project)
        .args(["load", "samples/crm-9.csv", "--url", &url])
        .output()
        .unwrap();
    assert_eq!(
        failure(&output, &["samples/crm-9.csv"]),
        "there is no file `samples/crm-9.csv`"
    );

    // A server that asks for a token, and gets none or a wrong one.
    let things = dir.path().join("things");
    write_things(&things);
    let server = Server::serve(&things, dir.path(), 1);
    write(&things.join("rows.csv"), "id,code\n1,anvil\n");
    let url = format!("http://{}", server.address);
    let without = spool()
        .current_dir(&things)
        .args(["load", "rows.csv", "--source", "shop", "--url", &url])
        .output()
        .unwrap();
    let complaint = failure(&without, &["rows.csv"]);
    assert!(
        complaint.starts_with(&format!("the server at {url} asks for a token\nhint: ")),
        "{complaint}"
    );
    let wrong = spool()
        .current_dir(&things)
        .args(["load", "rows.csv", "--source", "shop", "--url", &url])
        .args(["--token", "not-one"])
        .output()
        .unwrap();
    let complaint = failure(&wrong, &["rows.csv"]);
    assert!(
        complaint.starts_with(&format!(
            "the server at {url} did not accept the token\nhint: "
        )),
        "{complaint}"
    );
    // With its token, in the environment or as an option.
    let (printed, _) = load(&server, &things, &["rows.csv", "--source", "shop"]);
    assert_eq!(
        printed,
        "\
loaded  1 row into source `shop`, type `thing`: 1 new
"
    );
    let named = spool()
        .current_dir(&things)
        .args(["load", "rows.csv", "--source", "shop", "--url", &url])
        .args(["--token", server.token()])
        .output()
        .unwrap();
    assert!(named.status.success(), "{}", text(&named.stderr));
    assert_eq!(
        text(&named.stdout),
        "loaded  1 row into source `shop`, type `thing`: 0 new, 1 already held\n"
    );
}
