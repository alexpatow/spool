//! A folder read in batches: what each says, and what is kept when one stops or fails.

use super::*;

/// A first run fetches what the extractor depends on, which can take
/// minutes, and the reader is shown what the file says meanwhile.
#[test]
#[ignore = "runs uv"]
fn a_run_that_takes_a_while_says_what_its_extractor_says() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    // Says what it fetches, as the sample does, and then shows a bar that
    // writes over itself, as the libraries that fetch models do.
    write(
        &project.join("extractors/companies.py"),
        &BY_LEGAL_FORM.replacen(
            "import re\n",
            "import re\nimport sys\nimport time\n\n\
             print(\"Fetching the model (245 MB), once.\", file=sys.stderr, flush=True)\n\
             for done in range(60):\n    \
                 print(f\"\\r{done} of 60\", end=\"\", file=sys.stderr, flush=True)\n    \
                 time.sleep(0.1)\n",
            1,
        ),
    );
    let server = Server::dev(&project);

    // The command says which file is running once the run has taken a
    // while, and under it what the file said last: its own line, and not a
    // bar that is still moving. The answer is on standard output as ever.
    let started = Instant::now();
    let output = server
        .spool_in(&project, &["extract", "samples/notes-1.txt"])
        .output()
        .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(6));
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), FIRST);
    // Where the output is piped, what the file said is said once the run
    // has ended, and then how long it took to start.
    let said = text(&output.stderr);
    let lines: Vec<&str> = said
        .lines()
        .filter(|line| !line.starts_with("next: "))
        .collect();
    assert_eq!(
        lines[0], "             Fetching the model (245 MB), once.",
        "{said}"
    );
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with("    Prepared extractors/companies.py in "),
        "{said}"
    );
    // A line is said once, however often it was the last one.
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("Fetching"))
            .count(),
        1,
        "{said}"
    );
    assert!(!said.contains("of 60"), "{said}");

    // Over HTTP the run answers at once when it is asked to, and says the
    // same under `said` while it runs.
    let started = server
        .request("POST", "/v1/extractions")
        .json(&json!({ "again": true, "wait_ms": 0 }))
        .send()
        .expect(202);
    assert_eq!(started["run"]["status"], "running", "{started}");
    assert_eq!(started["run"]["kind"], "extract");
    let run = started["run"]["run"].clone();
    let asked = Instant::now();
    let finished = loop {
        let entry = server.read(&format!("/v1/runs/{run}?wait_ms=200"))["run"].clone();
        if entry["status"] != "running" {
            break entry;
        }
        if entry["said"] != Value::Null {
            assert_eq!(entry["said"], "Fetching the model (245 MB), once.");
        }
        assert!(asked.elapsed() < Duration::from_secs(120), "{entry}");
    };
    assert_eq!(finished["status"], "succeeded", "{finished}");
    assert_eq!(finished["said"], Value::Null, "{finished}");
    assert_eq!(finished["extraction"]["mentions"], 3, "{finished}");
}

/// The stand-in, made to take `seconds` over each document, and longer
/// over one whose text has `slow` in it: as long as `slow_seconds`, after
/// it writes a file `slow-started` in the project.
fn slow_stand_in(seconds: f64, slow_seconds: u32) -> String {
    BY_LEGAL_FORM
        .replacen("import re\n", "import re\nimport time\n", 1)
        .replacen(
            "    for found in",
            &format!(
                "    if \"slow\" in document.text:\n        \
                 open(\"slow-started\", \"w\").close()\n        \
                 time.sleep({slow_seconds})\n    \
                 time.sleep({seconds})\n    for found in"
            ),
            1,
        )
}

/// A folder is read in batches, each kept as it ends, and the command says
/// how far it is. The files under the folder are its documents, by their
/// paths from it, and what starts with a dot is left out.
#[test]
#[ignore = "runs uv"]
fn a_folder_is_read_in_batches_that_say_how_far_they_are() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_with_stand_in(dir.path());
    let documents = project.join("documents");
    notes(&documents.join("dev"), 6, None);
    notes(&documents.join("train"), 6, None);
    write(&documents.join(".DS_Store"), "\0");
    write(&documents.join("train/.hidden/note-99.txt"), "Kestrel Ltd.");
    let server = Server::dev(&project);

    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let printed = text(&output.stdout);
    assert!(
        printed.starts_with(
            "\
source     notes
read       12 documents from documents
loaded     12 new, 0 already held, 0 new versions
extractor  companies
found      12 records in 12 documents: 12 new, 0 already held

  company  Acorn Tools Ltd  notes:dev/note-01.txt["
        ),
        "{printed}"
    );
    // The first batch is five documents, and the next is made larger.
    let said = text(&output.stderr);
    let lines: Vec<&str> = said
        .lines()
        .filter(|line| !line.starts_with("next: "))
        .collect();
    assert_eq!(lines.len(), 1, "{said}");
    assert!(
        lines[0].starts_with("   Extracted 12 documents in ")
            && lines[0].ends_with(": 12 records, 0 failed"),
        "{said}"
    );
    let batches = runs(&server, "extract");
    assert!(batches.len() >= 2, "{batches:?}");
    assert!(batches.len() < 12, "{batches:?}");
    let keys: Vec<String> = server.read("/v1/documents?source=notes&limit=100")["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|document| document["key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(keys.len(), 12, "{keys:?}");
    assert!(keys.contains(&"train/note-06.txt".to_owned()), "{keys:?}");

    // The same command again has nothing left to read.
    let again = extracted(&server, &project, &["documents", "--source", "notes"]);
    assert!(
        again.contains(
            "loaded     0 new, 12 already held, 0 new versions\nextractor  companies\n\
             found      0 records: `companies` has read every document."
        ),
        "{again}"
    );

    // `--again` reads each document once more, in batches too.
    let again = extracted(&server, &project, &["--again", "--source", "notes"]);
    assert!(
        again.contains("found      12 records in 12 documents: 0 new, 12 already held"),
        "{again}"
    );
    let read: Vec<usize> = server.read("/v1/documents?source=notes&limit=100")["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|document| document["extractions"].as_array().unwrap().len())
        .collect();
    assert_eq!(read, vec![2; 12]);
}

/// A batch that fails, here one stopped for taking longer than Spool lets
/// user code run, keeps nothing, and the batches before it are kept. The
/// message says what the limit is, what raises it, and that the same
/// command reads the rest.
#[test]
#[ignore = "runs uv"]
fn a_batch_that_is_stopped_keeps_the_ones_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    write(
        &project.join("extractors/companies.py"),
        &slow_stand_in(0.05, 60),
    );
    let documents = project.join("documents");
    notes(&documents, 12, Some(9));
    let server = Server::start(&[
        "dev",
        path(&project),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
        "--task-timeout",
        "20",
    ]);

    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    let said = text(&output.stderr);
    let lines: Vec<&str> = said.lines().collect();
    let kept: usize = lines[lines.len() - 2]
        .strip_prefix("        Kept ")
        .and_then(|line| line.split_once(" documents read before run "))
        .and_then(|(kept, rest)| {
            rest.ends_with(": the same command again reads the rest")
                .then(|| kept.parse().ok())?
        })
        .unwrap_or_else(|| panic!("{said}"));
    assert!((5..9).contains(&kept), "{said}");
    let failed = lines.last().unwrap();
    assert!(
        failed.contains(
            "failed: extractor `companies`: it was stopped after 20 seconds without finishing. \
             That is the longest a strategy or an extractor may run here, and \
             `--task-timeout <seconds>` sets it, on `spool dev`, `spool serve` or `spool bench`"
        ),
        "{said}"
    );
    assert_eq!(found(&server).len(), kept);

    // Without the slow note's wait, the same command reads what is left.
    drop(server);
    write(
        &project.join("extractors/companies.py"),
        &slow_stand_in(0.05, 0),
    );
    let server = Server::dev(&project);
    let read = extracted(&server, &project, &["documents", "--source", "notes"]);
    assert!(
        read.contains(&format!(
            "found      {} records in {} documents: {} new, 0 already held",
            12 - kept,
            12 - kept,
            12 - kept
        )),
        "{read}"
    );
}

/// A batch whose extractor fails at once is refused within the wait, and
/// the command still says what the batches before it kept, and prints them
/// with `--json`.
#[test]
#[ignore = "runs uv"]
fn a_batch_that_fails_at_once_keeps_the_ones_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    // It fails for every document of a batch that holds the slow note:
    // what fails a run, and not one document.
    write(
        &project.join("extractors/companies.py"),
        r#"import re

from spool import extractor

NAME = re.compile(r"(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\b")


@extractor
def find(documents):
    if any("slow" in document.text for document in documents):
        raise RuntimeError("cannot read this one")
    for document in documents:
        for found in NAME.finditer(document.text):
            yield document.mention("company", found.start(), found.end(), name=found.group())
"#,
    );
    notes(&project.join("documents"), 8, Some(7));
    let server = Server::dev(&project);

    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    let said = text(&output.stderr);
    assert!(
        said.contains(
            "        Kept 5 documents read before the batch that failed: the same command again \
             reads the rest\n"
        ),
        "{said}"
    );
    assert!(said.contains("cannot read this one"), "{said}");
    assert_eq!(found(&server).len(), 5);

    // As JSON, from the start in a project of its own: the run that stored
    // the documents, and the batch that was kept.
    drop(server);
    let dir = tempfile::tempdir().unwrap();
    let other = init_companies(dir.path());
    std::fs::copy(
        project.join("extractors/companies.py"),
        other.join("extractors/companies.py"),
    )
    .unwrap();
    notes(&other.join("documents"), 8, Some(7));
    let server = Server::dev(&other);
    let output = server
        .spool_in(
            &other,
            &["extract", "documents", "--source", "notes", "--json"],
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let answered: Value = serde_json::from_str(&text(&output.stdout))
        .unwrap_or_else(|_| panic!("{}{}", text(&output.stdout), text(&output.stderr)));
    assert_eq!(
        answered["documents"].as_array().unwrap().len(),
        1,
        "{answered}"
    );
    assert_eq!(
        answered["documents"][0]["load"]["inserted"], 8,
        "{answered}"
    );
    let [kept] = &answered["extractions"].as_array().unwrap()[..] else {
        panic!("one batch kept: {answered}");
    };
    assert_eq!(kept["status"], "succeeded", "{answered}");
    assert_eq!(kept["extraction"]["documents"], 5, "{answered}");
}

/// The stand-in, made to fail with `RuntimeError("cannot read this one")`
/// for a note with `slow` in it, or for every note.
fn failing_stand_in(every: bool) -> String {
    let when = if every {
        "True"
    } else {
        "\"slow\" in document.text"
    };
    BY_LEGAL_FORM.replacen(
        "    for found in",
        &format!(
            "    if {when}:\n        raise RuntimeError(\"cannot read this one\")\n    for found in"
        ),
        1,
    )
}

/// A document the extractor cannot read is failed, with why: the command
/// lists it, `spool show documents` says so, and the same command reads it
/// alone once the extractor can. A batch none of whose documents could be
/// read stops the command.
#[test]
#[ignore = "runs uv"]
fn a_document_that_cannot_be_read_is_failed_and_read_again_alone() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    notes(&project.join("documents"), 8, Some(7));

    // Every document fails: a key that is wrong, say. The first batch
    // says so, and nothing more is tried.
    write(
        &project.join("extractors/companies.py"),
        &failing_stand_in(true),
    );
    let server = Server::dev(&project);
    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let said = text(&output.stderr);
    assert!(
        said.trim_end().ends_with(
            "could not read any of its 5 documents, as notes:note-01.txt says: RuntimeError: \
             cannot read this one. They are not read, and the same command reads them again."
        ),
        "{said}"
    );
    assert_eq!(runs(&server, "extract").len(), 1);

    // One document fails: the others are read, and it is listed.
    drop(server);
    write(
        &project.join("extractors/companies.py"),
        &failing_stand_in(false),
    );
    let server = Server::dev(&project);
    let output = server
        .spool_in(&project, &["extract", "documents", "--source", "notes"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let printed = text(&output.stdout);
    assert!(
        printed.contains(
            "found      7 records in 7 documents: 7 new, 0 already held\n\
             failed     1 document: they are not read, and the same command reads them again\n  \
             notes:note-07.txt  RuntimeError: cannot read this one\n"
        ),
        "{printed}"
    );
    // What the helper wrote of it reaches the terminal, after its batch.
    let said = text(&output.stderr);
    assert!(
        said.contains("             notes:note-07.txt: RuntimeError: cannot read this one\n"),
        "{said}"
    );
    assert!(
        said.contains("   Extracted 7 documents in ") && said.contains(": 7 records, 1 failed\n"),
        "{said}"
    );
    let listed = shown(
        &server,
        &project,
        &["show", "documents", "--source", "notes"],
    );
    let line = listed
        .lines()
        .find(|line| line.contains("notes:note-07.txt"))
        .unwrap_or_else(|| panic!("{listed}"));
    assert!(
        line.contains("companies failed in run ")
            && line.ends_with(": RuntimeError: cannot read this one"),
        "{listed}"
    );

    // Once it can be read, the same command reads it and nothing else.
    drop(server);
    write(&project.join("extractors/companies.py"), BY_LEGAL_FORM);
    let server = Server::dev(&project);
    let read = extracted(&server, &project, &["documents", "--source", "notes"]);
    assert!(
        read.contains("found      1 record in 1 document: 1 new, 0 already held"),
        "{read}"
    );
}

/// What an extractor writes to standard error reaches whoever runs
/// `spool extract`, however short the run, and the terminal of `spool dev`.
#[test]
#[ignore = "runs uv"]
fn what_an_extractor_says_reaches_both_terminals_on_a_short_run() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    write(
        &project.join("extractors/companies.py"),
        &BY_LEGAL_FORM
            .replacen("import re\n", "import re\nimport sys\n", 1)
            .replacen(
                "    for found in",
                "    print(f\"{document.key}: dropped a quote the text does not have\", file=sys.stderr)\n    for found in",
                1,
            ),
    );
    let server = Server::dev(&project);
    let output = server
        .spool_in(&project, &["extract", "samples/notes-1.txt"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), FIRST);
    assert_eq!(
        text(&output.stderr),
        "             notes-1.txt: dropped a quote the text does not have\nnext: `spool match` \
         places them in identities\n"
    );
    let run = runs(&server, "extract")[0]["run"].as_u64().unwrap();
    assert_eq!(
        server.printed("extractor `companies`"),
        format!("extractor `companies`, run {run}:")
    );
    assert_eq!(
        server.printed("dropped a quote").trim(),
        "notes-1.txt: dropped a quote the text does not have"
    );
}

/// Ctrl-C cancels the batch that is running: the server stops the
/// extractor and keeps nothing of the batch, the batches before it are
/// kept, and the command says so. While the batch runs, its documents say
/// they are being read.
#[test]
#[ignore = "runs uv"]
fn ctrl_c_cancels_the_batch_that_is_running_and_keeps_the_ones_before() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    write(
        &project.join("extractors/companies.py"),
        &slow_stand_in(0.0, 120),
    );
    let documents = project.join("documents");
    notes(&documents, 10, Some(8));
    let server = Server::dev(&project);

    let mut command = server.spool_in(&project, &["extract", "documents", "--source", "notes"]);
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = project.join("slow-started");
    let deadline = Instant::now() + Duration::from_secs(120);
    while !started.exists() {
        assert!(Instant::now() < deadline, "the slow note was never read");
        assert!(child.try_wait().unwrap().is_none(), "the command ended");
        std::thread::sleep(Duration::from_millis(50));
    }

    // The documents of the batch say which run reads them.
    let running: Vec<Value> = runs(&server, "extract")
        .into_iter()
        .filter(|run| run["status"] == "running")
        .collect();
    let [running] = &running[..] else {
        panic!("one batch runs: {running:?}");
    };
    let run = running["run"].as_u64().unwrap();
    let listed = shown(
        &server,
        &project,
        &["show", "documents", "--source", "notes"],
    );
    let state = |listed: &str, key: &str| {
        listed
            .lines()
            .find(|line| line.contains(&format!("notes:{key}")))
            .unwrap_or_else(|| panic!("{listed}"))
            .split("  ")
            .filter(|cell| !cell.is_empty())
            .last()
            .unwrap()
            .trim()
            .to_owned()
    };
    assert_eq!(
        state(&listed, "note-08.txt"),
        format!("being read by companies in run {run}"),
        "{listed}"
    );
    assert!(
        state(&listed, "note-01.txt").starts_with("companies in run "),
        "{listed}"
    );

    let interrupted = Instant::now();
    let killed = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let output = child.wait_with_output().unwrap();
    assert!(interrupted.elapsed() < Duration::from_secs(20));
    assert_eq!(output.status.code(), Some(130), "{}", text(&output.stderr));
    let said = text(&output.stderr);
    assert!(
        said.contains(&format!("    Stopping run {run}, the batch in progress")),
        "{said}"
    );
    assert!(
        said.trim_end().ends_with(&format!(
            "error: stopped: run {run} was cancelled, and nothing it read is kept. The 5 \
             documents read before it are kept, and the same command again reads the rest."
        )),
        "{said}"
    );

    // The server stopped the extractor, which would have slept two minutes,
    // and the run failed. Its documents are not read.
    let ended = server.read(&format!("/v1/runs/{run}"))["run"].clone();
    assert_eq!(ended["status"], "failed", "{ended}");
    assert!(
        ended["error"]
            .as_str()
            .unwrap()
            .contains("it was cancelled"),
        "{ended}"
    );
    assert_eq!(found(&server).len(), 5);
    let listed = shown(
        &server,
        &project,
        &["show", "documents", "--source", "notes"],
    );
    assert_eq!(state(&listed, "note-08.txt"), "not read yet", "{listed}");

    // The same command reads what is left.
    drop(server);
    write(
        &project.join("extractors/companies.py"),
        &slow_stand_in(0.0, 0),
    );
    let server = Server::dev(&project);
    let read = extracted(&server, &project, &["documents", "--source", "notes"]);
    assert!(
        read.contains("found      5 records in 5 documents: 5 new, 0 already held"),
        "{read}"
    );
}
