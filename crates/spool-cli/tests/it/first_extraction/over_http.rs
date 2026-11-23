//! What `spool extract` sends to a server, and what it says stands in its way.

use super::*;

/// `spool dev` over a project, on a machine with nothing installed: the
/// search path is an empty directory, and so is the home directory, where
/// the installers of `uv` and `bun` put them.
fn dev_with_nothing_installed(project: &Path, dir: &Path) -> Server {
    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    Server::start_in(
        &[
            "dev",
            path(project),
            "--listen",
            "127.0.0.1:0",
            "--seed",
            "1",
        ],
        &[
            ("PATH", path(&empty)),
            ("HOME", path(&empty)),
            ("SPOOL_UV", ""),
            ("SPOOL_BUN", ""),
        ],
    )
    .unwrap_or_else(|stopped| panic!("the server did not start:\n{}", stopped.output))
}

/// The directories of tasks that were kept under a project's `.spool`.
fn kept_tasks(project: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(project.join(".spool"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .flat_map(|dir| std::fs::read_dir(dir).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("run-"))
        })
        .collect()
}

/// The step as a program takes it: two requests, with no JSON to write.
#[test]
#[ignore = "runs uv"]
fn the_same_step_over_http_sends_the_file_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_with_stand_in(dir.path());
    let server = Server::dev(&project);
    let text_plain = |path: &str, body: &[u8]| {
        server
            .request("POST", path)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(body)
            .send()
    };

    // `curl --data-binary @samples/notes-1.txt -H 'content-type: text/plain'`.
    let stored = text_plain(
        "/v1/documents?source=notes&key=notes-1.txt",
        NOTE.as_bytes(),
    )
    .ok();
    assert_eq!(stored["run"]["status"], "succeeded", "{stored}");
    assert_eq!(stored["run"]["load"]["inserted"], 1, "{stored}");
    // A document sent as text is loaded by its caller, as one sent as JSON.
    let dev = server
        .request("GET", "/auth/session")
        .without("Authorization")
        .send()
        .ok();
    let dev = format!("user:{}", dev["person"]["id"].as_str().unwrap());
    assert_eq!(stored["run"]["started_by"], dev.as_str(), "{stored}");
    let documents = server.read("/v1/documents?source=notes");
    let document = &documents["documents"][0];
    assert_eq!(document["key"], "notes-1.txt");
    assert_eq!(document["length"], NOTE.chars().count());
    let held = server.read(&format!(
        "/v1/documents/{}",
        document["id"].as_str().unwrap()
    ));
    assert_eq!(held["text"], NOTE, "{held}");

    // `curl -X POST .../v1/extractions`: the project has one extractor, so
    // there is nothing to say.
    let read = server.request("POST", "/v1/extractions").send().ok();
    assert_eq!(read["run"]["status"], "succeeded", "{read}");
    assert_eq!(read["run"]["extractor"]["name"], "companies");
    assert_eq!(read["run"]["started_by"], dev.as_str(), "{read}");
    let report = &read["run"]["extraction"];
    assert_eq!(
        (
            &report["documents"],
            &report["mentions"],
            &report["inserted"]
        ),
        (&json!(1), &json!(3), &json!(3))
    );
    let names: Vec<Value> = found(&server)
        .iter()
        .map(|record| record["payload"]["name"].clone())
        .collect();
    assert_eq!(
        names,
        [
            "Acorn Tools Ltd",
            "Larkfield Logistics Ltd",
            "Brightwater Foods Ltd"
        ]
    );

    // It is the document `spool extract` stores: the command finds it held,
    // with everything read.
    let again = shown(&server, &project, &["extract", "samples/notes-1.txt"]);
    assert!(
        again.contains("loaded     0 new, 1 already held, 0 new versions\n"),
        "{again}"
    );
    assert!(again.contains("found      0 records: "), "{again}");

    // A body of JSON still names the extractor or leaves it out, and the
    // text form takes where the source keeps the document.
    for body in [
        json!({}),
        json!({ "extractor": "companies", "again": true }),
    ] {
        let read = server.call("/v1/extractions", body);
        assert_eq!(read["run"]["extractor"]["name"], "companies", "{read}");
    }
    text_plain(
        "/v1/documents?source=notes&key=visit.txt&uri=https://notes.test/visit",
        b"We visited Tidewater Marine LLC.",
    )
    .ok();
    let visit = server.read("/v1/documents?source=notes&key=visit.txt");
    assert_eq!(visit["documents"][0]["uri"], "https://notes.test/visit");

    send_documents_whatever_their_content_type(&server);

    name_the_extractor_over_http(&server, &project);
}

/// What a reader is told when the step cannot be taken, each as it is said.
/// The machine has neither `uv` nor `bun`, as a reader's may not.
#[test]
fn extract_says_what_is_in_the_way() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());

    // No server is running: said as every command says it, with what to do.
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = free.local_addr().unwrap();
    drop(free);
    let output = spool()
        .current_dir(&project)
        .args(["extract", "samples/notes-1.txt"])
        .args(["--url", &format!("http://{address}")])
        .output()
        .unwrap();
    let nobody = complaint(&output, &["samples/notes-1.txt"]);
    assert!(
        nobody.starts_with(&format!(
            "the server at http://{address} cannot be reached: "
        )),
        "{nobody}"
    );
    assert!(
        nobody.ends_with(
            "\nhint: start one with `spool dev`, or name another with `--url` or SPOOL_URL"
        ),
        "{nobody}"
    );

    let server = dev_with_nothing_installed(&project, dir.path());
    let refused = |arguments: &[&str]| refused(&server, &project, arguments);

    refuse_what_is_wrong_with_the_files(&server, &project);

    refuse_sources_and_extractors_it_cannot_use(&server, &project);
    // None of that stored a document or started a run.
    assert_eq!(server.read("/v1/documents")["documents"], json!([]));
    assert_eq!(runs(&server, "extract"), Vec::<Value>::new());

    // The step itself, on a machine without `uv`: the note is stored, and
    // the reader is told what to install, in one line to paste, before a
    // run is started and with no task left behind.
    let without_uv = "extractor `companies`: it is a Python file, and Spool runs those with \
         `uv`, which is not installed where Spool runs. Install it there and run this again:\n\n  \
         curl -LsSf https://astral.sh/uv/install.sh | sh\n\n\
         Spool finds it where that puts it, with no restart. Other ways to install it: \
         https://docs.astral.sh/uv/getting-started/installation/. If it is installed where the \
         search path does not reach, set SPOOL_UV to it where Spool is started";
    let output = server
        .spool_in(&project, &["extract", "samples/notes-1.txt"])
        .output()
        .unwrap();
    assert_eq!(
        text(&output.stdout),
        "\
source     notes
read       1 document from samples/notes-1.txt
loaded     1 new, 0 already held, 0 new versions
"
    );
    assert_eq!(complaint(&output, &["samples/notes-1.txt"]), without_uv);
    // The rows need nothing installed.
    shown(&server, &project, &["load", "samples/crm-1.csv"]);
    shown(&server, &project, &["match"]);

    // A second extractor: the command asks which. Nothing here runs a
    // TypeScript file either.
    write(
        &project.join("extractors/words.ts"),
        "import { extractor } from \"spool-strategy\";\n\nawait extractor(async () => []);\n",
    );
    server.printed("applied");
    assert_eq!(
        refused(&["samples/notes-1.txt"]),
        "the project has the extractors `companies` and `words`: say which with `--extractor`"
    );
    assert_eq!(refused(&["--extractor", "companies"]), without_uv);
    assert_eq!(
        refused(&["--extractor", "words"]),
        "extractor `words`: it is a TypeScript file, and Spool runs those with `bun`, which is \
         not installed where Spool runs. Install it there and run this again:\n\n  \
         curl -fsSL https://bun.sh/install | bash\n\n\
         Spool finds it where that puts it, with no restart. Other ways to install it: \
         https://bun.sh/docs/installation. If it is installed where the search path does not \
         reach, set SPOOL_BUN to it where Spool is started"
    );
    assert_eq!(runs(&server, "extract"), Vec::<Value>::new());
    assert_eq!(kept_tasks(&project), Vec::<PathBuf>::new());

    // The reader installs `uv`, which goes under the home directory, where
    // the search path of a server that is already running does not reach.
    // The next run starts it all the same: here a stand-in, which says so.
    let installed = dir.path().join("empty/.local/bin/uv");
    write(
        &installed,
        "#!/bin/sh\necho 'a stand-in for uv was started' >&2\nexit 3\n",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let started = refused(&["--extractor", "companies"]);
    assert!(
        started.starts_with(
            "extractor `companies`: it failed with exit status: 3:\n\
             a stand-in for uv was started\n"
        ),
        "{started}"
    );
}

/// The query string says that a body is a document, whatever its content
/// type; without one the body is JSON. What is wrong with a document sent
/// as text is said in its terms.
fn send_documents_whatever_their_content_type(server: &Server) {
    let text_plain = |path: &str, body: &[u8]| {
        server
            .request("POST", path)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(body)
            .send()
    };
    // The query string says that the body is a document, so a file sent
    // the way `curl` sends one unless told otherwise is still the file.
    let sent = server
        .request("POST", "/v1/documents?source=notes&key=memo.md")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(b"# Memo\n\nHalden Group Inc wrote.\n")
        .send()
        .ok();
    assert_eq!(sent["run"]["load"]["inserted"], 1, "{sent}");
    // And without one the body is JSON, as a browser sends it unless told
    // otherwise.
    let sent = server
        .request("POST", "/v1/documents")
        .header("Content-Type", "text/plain;charset=UTF-8")
        .body(br#"{"source": "notes", "documents": [{"key": "fax.txt", "text": "A fax."}]}"#)
        .send()
        .ok();
    assert_eq!(sent["run"]["load"]["inserted"], 1, "{sent}");
    assert_eq!(
        text_plain("/v1/documents", b"A note.").error(400, "invalid"),
        "the body is not the JSON expected: expected value at line 1 column 1. To send one \
         document as text, name it in the query string, as in \
         `/v1/documents?source=notes&key=call.txt`"
    );

    // What is wrong with a document sent as text is said in its terms.
    let unnamed = text_plain("/v1/documents?source=notes", b"text").error(400, "invalid");
    assert!(
        unnamed.starts_with(
            "a document sent as text is named in the query string, as in \
             `/v1/documents?source=notes&key=call.txt`: "
        ),
        "{unnamed}"
    );
    assert!(unnamed.contains("key"), "{unnamed}");
    assert_eq!(
        text_plain(
            "/v1/documents?source=notes&key=scan.pdf",
            b"%PDF-1.7\n\xff\xfe"
        )
        .error(400, "invalid"),
        "the body is not UTF-8 text: a document is stored as text, so turn a PDF or a web page \
         into text first"
    );
    assert_eq!(
        server.read("/v1/documents?source=notes")["documents"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

/// An extractor has to be named once there are two, and there is none to
/// name once there is none.
fn name_the_extractor_over_http(server: &Server, project: &Path) {
    // An extractor has to be named once there are two, and there is none
    // to name once there is none.
    write(
        &project.join("extractors/shouting.py"),
        &BY_LEGAL_FORM.replace(
            r"(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\b",
            "[A-Z]{4,}",
        ),
    );
    server.printed("applied");
    assert_eq!(
        server
            .request("POST", "/v1/extractions")
            .send()
            .error(400, "invalid"),
        "the project has the extractors `companies` and `shouting`: say which in `extractor`"
    );
    let read = server.call("/v1/extractions", json!({ "extractor": "shouting" }));
    assert_eq!(read["run"]["extractor"]["name"], "shouting", "{read}");
    for extractor in ["companies", "shouting"] {
        std::fs::remove_file(project.join(format!("extractors/{extractor}.py"))).unwrap();
        server.printed("applied");
    }
    assert_eq!(
        server
            .request("POST", "/v1/extractions")
            .send()
            .error(400, "invalid"),
        "the project has no extractors: one is a file under extractors/, and \
         `spool init <dir> --example companies` makes a project with a sample"
    );
}

/// What is wrong with a file or a folder is said before anything is sent.
fn refuse_what_is_wrong_with_the_files(server: &Server, project: &Path) {
    let refused = |arguments: &[&str]| refused(server, project, arguments);
    // The file: what is wrong with it is said before anything is sent.
    let missing = refused(&["samples/notes-9.txt"]);
    assert!(
        missing.starts_with("samples/notes-9.txt: No such file or directory"),
        "{missing}"
    );
    std::fs::write(project.join("report.pdf"), b"%PDF-1.7\n\xff\xfe\xfd").unwrap();
    assert_eq!(
        refused(&["report.pdf", "--source", "notes"]),
        "report.pdf is not text: a document is stored as UTF-8 text. Turn a PDF or a web page \
         into text first."
    );
    write(&project.join("notes-empty.txt"), "\n\n");
    assert_eq!(refused(&["notes-empty.txt"]), "notes-empty.txt is empty");
    write(&project.join("other/notes-1.txt"), "Acorn Tools Ltd.\n");
    assert_eq!(
        refused(&["samples/notes-1.txt", "other/notes-1.txt"]),
        "samples/notes-1.txt and other/notes-1.txt would both be stored as `notes-1.txt`: a \
         file is stored under its name, or under its path from the folder that was named, and \
         a second document under one key is taken for a new version of the first. Name the \
         folder that holds both, or extract them into sources of their own."
    );
    // A folder: what starts with a dot is left out, and every other file
    // has to be text.
    write(&project.join("empty/.DS_Store"), "\0\0");
    write(
        &project.join("empty/.cache/notes-1.txt"),
        "Acorn Tools Ltd.\n",
    );
    assert_eq!(
        refused(&["empty"]),
        "empty is a folder with no files in it, other than ones whose names start with a dot"
    );
    std::fs::create_dir_all(project.join("scans/2024")).unwrap();
    std::fs::write(project.join("scans/2024/report.pdf"), b"%PDF-1.7\n\xff").unwrap();
    assert_eq!(
        refused(&["scans", "--source", "notes"]),
        "scans/2024/report.pdf is not text: a document is stored as UTF-8 text. Turn a PDF or \
         a web page into text first."
    );
}

/// A source or an extractor `spool extract` cannot use is said before a
/// document is stored.
fn refuse_sources_and_extractors_it_cannot_use(server: &Server, project: &Path) {
    let refused = |arguments: &[&str]| refused(server, project, arguments);
    // The source: a file of the reader's own is named after neither.
    write(&project.join("call.txt"), "We met Acorn Tools Ltd.\n");
    assert_eq!(
        refused(&["call.txt"]),
        "the project has the sources `crm` and `notes`, and call.txt is named after neither: \
         say which its documents belong to with `--source`"
    );
    assert_eq!(
        refused(&["call.txt", "--source", "calls"]),
        "the project has no source `calls`: there is no sources/calls.yaml. It has `crm` and \
         `notes`."
    );
    assert_eq!(
        refused(&["call.txt", "--extractor", "people"]),
        "the project has no extractor `people`: it has `companies`"
    );
    // A source no entity type reads cannot hold what an extractor finds,
    // which is said before the document is stored.
    write(&project.join("sources/wire.yaml"), "columns: [name]\n");
    server.printed("applied");
    assert_eq!(
        refused(&["call.txt", "--source", "wire"]),
        "no entity type reads source `wire`, so nothing found in its documents can be a \
         record. Add the source to the `mapping` in an entities/<type>/entity.yaml."
    );
}
