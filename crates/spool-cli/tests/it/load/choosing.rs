//! Which source and type the rows of a file are loaded as.

use super::*;

#[test]
fn load_asks_which_source_and_type_when_the_project_has_several() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_two_of_each(&project);
    let server = Server::serve(&project, dir.path(), 1);
    write(&project.join("rows.csv"), "id,name\n1,Ada\n");
    let refused = |arguments: &[&str]| refused(&server, &project, arguments);

    assert_eq!(
        refused(&["rows.csv"]),
        "the project has the sources `crm`, `registry` and `spare`, and rows.csv is named after \
         none of them: say which its rows come from with `--source`"
    );
    assert_eq!(
        refused(&["rows.csv", "--source", "crm"]),
        "say which entity type the rows are records of with `--type`: source `crm` is read by \
         `company`, `person`"
    );
    assert_eq!(
        refused(&["rows.csv", "--type", "company"]),
        "`company` reads the sources `crm` and `registry`, and rows.csv is named after neither: \
         say which its rows come from with `--source`"
    );
    // Standard input has no name to go by.
    let mut child = server
        .spool_in(&project, &["load", "-", "--type", "company"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"id,name\n1,Ada\n")
        .unwrap();
    assert_eq!(
        failure(&child.wait_with_output().unwrap(), &["-"]),
        "`company` reads the sources `crm` and `registry`: say which the rows come from with \
         `--source`"
    );

    // Names the project does not have, and a pair that does not go together.
    assert_eq!(
        refused(&["rows.csv", "--source", "erp"]),
        "the project has no source `erp`: there is no sources/erp.yaml. It has `crm`, \
         `registry`, `spare`."
    );
    assert_eq!(
        refused(&["rows.csv", "--type", "place"]),
        "the project has no entity type `place`. It has `company`, `person`."
    );
    assert_eq!(
        refused(&["rows.csv", "--source", "registry", "--type", "person"]),
        "the mapping of `person` has nothing for source `registry`, so its rows cannot be \
         `person` records. They can be loaded as `company`."
    );
    assert_eq!(
        refused(&["rows.csv", "--source", "spare"]),
        "no entity type reads source `spare`, so its rows cannot be loaded yet. Add the source \
         to the `mapping` in an entities/<type>/entity.yaml."
    );
    assert_eq!(
        refused(&["rows.csv", "--source", "spare", "--type", "person"]),
        "the mapping of `person` has nothing for source `spare`: add it to \
         entities/person/entity.yaml"
    );
    assert_eq!(load_runs(&server), Vec::<Value>::new());

    // One of the two is enough where it leaves one choice of the other, and
    // the next step names the type, since `spool match` would ask.
    let (printed, _) = load(&server, &project, &["rows.csv", "--type", "person"]);
    assert_eq!(
        printed,
        "\
loaded  1 row into source `crm`, type `person`: 1 new
"
    );
    let (printed, _) = load(&server, &project, &["rows.csv", "--source", "registry"]);
    assert_eq!(
        printed,
        "\
loaded  1 row into source `registry`, type `company`: 1 new
"
    );
    // A file named after a source holds that source's rows: the name up to
    // the first character that is no letter or digit, wherever the file is.
    write(
        &project.join("exports/registry-2024.csv"),
        "id,name\n7,Lin\n",
    );
    let (printed, _) = load(&server, &project, &["exports/registry-2024.csv"]);
    assert!(
        printed.starts_with("loaded  1 row into source `registry`, type `company`: 1 new\n"),
        "{printed}"
    );
    // The name says the source and nothing of the type, and a name that
    // only begins like a source's is not after it.
    write(&project.join("crm.csv"), "id,name\n8,Kay\n");
    assert_eq!(
        refused(&["crm.csv"]),
        "say which entity type the rows are records of with `--type`: source `crm` is read by \
         `company`, `person`"
    );
    write(&project.join("crms.csv"), "id,name\n8,Kay\n");
    assert_eq!(
        refused(&["crms.csv", "--type", "company"]),
        "`company` reads the sources `crm` and `registry`, and crms.csv is named after neither: \
         say which its rows come from with `--source`"
    );
    // What is asked for is what is done, whatever the file is called.
    let (printed, _) = load(&server, &project, &["crm.csv", "--source", "registry"]);
    assert!(
        printed.starts_with("loaded  1 row into source `registry`, type `company`: 1 new\n"),
        "{printed}"
    );

    write(&project.join("more.csv"), "id,name\n2,Grace\n");
    let (printed, _) = load(
        &server,
        &project,
        &["more.csv", "--source", "crm", "--type", "company"],
    );
    assert!(
        printed.starts_with("loaded  1 row into source `crm`, type `company`: 1 new\n"),
        "{printed}"
    );

    // What the server refuses is said as it says it: a row is a record of
    // one entity type.
    assert_eq!(
        refused(&["rows.csv", "--source", "crm", "--type", "company"]),
        "load rejected: a row with the same source, key and content is already stored as \
         entity type `person`"
    );
}

#[test]
fn a_project_with_a_source_of_documents_loads_its_rows_by_the_name_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let server = Server::dev(&project);
    let refused = |arguments: &[&str]| refused(&server, &project, arguments);

    // The sample rows need no `--source`: the files are named after theirs.
    for (file, rows) in [("samples/crm-1.csv", 6), ("samples/crm-2.csv", 3)] {
        let (printed, _) = load(&server, &project, &[file]);
        assert_eq!(
            printed,
            format!(
                "\
loaded  {rows} rows into source `crm`, type `company`: {rows} new
"
            )
        );
    }
    assert_eq!(records(&server, "crm").len(), 9);
    assert_eq!(records(&server, "notes"), Vec::new());

    // A document is not rows, and another command reads it.
    assert_eq!(
        refused(&["samples/notes-1.txt"]),
        "samples/notes-1.txt does not look like rows: its first line names no column that a \
         source of the project lists. `spool load` reads CSV, with the names of the columns on \
         its first line.\nhint: \
         if it is a document, `spool extract samples/notes-1.txt` reads it: an extractor finds \
         the records in its text"
    );
    write(
        &project.join("call.md"),
        "# A call\n\nWith Acorn Tools Ltd.\n",
    );
    assert!(
        refused(&["call.md"]).ends_with("\nhint: if it is a document, `spool extract call.md` reads it: an extractor finds the records in its text"),
    );
    // A first line that cannot name columns is said as it is of any file,
    // with where a document goes.
    write(&project.join("letter.txt"), "Dear Dana,\n\nThank you.\n");
    assert_eq!(
        refused(&["letter.txt"]),
        "letter.txt cannot be read as CSV: line 1: column 2 has no name. The first line has to \
         name the columns\n\
         hint: if it is a document, `spool extract letter.txt` reads it: an extractor finds the \
         records in its text"
    );
    // Nothing is said of documents where they are not the question: a file
    // with nothing in it, a file of rows, and one said to hold rows.
    write(&project.join("empty.txt"), "");
    assert_eq!(
        refused(&["empty.txt"]),
        "empty.txt cannot be read as CSV: it is empty: the first line has to name the columns"
    );
    assert_eq!(
        refused(&["samples/notes-1.txt", "--delimiter", ","]),
        "samples/notes-1.txt has a column `Call with Dana Reyes on 12 November. She has moved \
         to Acorn Tools Ltd and runs`, which sources/notes.yaml does not list. It lists `name`. \
         Add the column to that file to keep it, or leave it out of the rows."
    );
    assert_eq!(load_runs(&server).len(), 2);

    // Rows can be kept in a text file, and rows of the other source are
    // loaded as any are.
    write(
        &project.join("crm-export.txt"),
        "id,name\nc-30,Zennor Glass\n",
    );
    let (printed, _) = load(&server, &project, &["crm-export.txt"]);
    assert!(
        printed.starts_with("loaded  1 row into source `crm`, type `company`: 1 new\n"),
        "{printed}"
    );
    write(&project.join("notes.csv"), "name\nAcorn Tools Ltd\n");
    let (printed, _) = load(&server, &project, &["notes.csv"]);
    assert!(
        printed.starts_with("loaded  1 row into source `notes`, type `company`: 1 new\n"),
        "{printed}"
    );

    // A file named after neither source is the one's that lists more of its
    // columns, and has to be told when both list as many.
    write(
        &project.join("more.csv"),
        "id,name,country\nc-31,Yarrow Mills,GB\n",
    );
    let (printed, _) = load(&server, &project, &["more.csv"]);
    assert!(
        printed.starts_with("loaded  1 row into source `crm`, type `company`: 1 new\n"),
        "{printed}"
    );
    write(&project.join("names.csv"), "name\nXanthe Dairy\n");
    assert_eq!(
        refused(&["names.csv"]),
        "the project has the sources `crm` and `notes`, and names.csv is named after neither: \
         say which its rows come from with `--source`"
    );
    let (printed, _) = load(&server, &project, &["names.csv", "--source", "crm"]);
    assert!(
        printed.starts_with("loaded  1 row into source `crm`, type `company`: 1 new\n"),
        "{printed}"
    );
}
