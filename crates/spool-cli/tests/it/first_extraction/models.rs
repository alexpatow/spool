//! The models an extractor fetches and asks.

use super::*;

/// The Dockerfile `spool build` writes for a new project puts what the
/// extractor fetches in the image, its model with it, and has a run fetch
/// nothing.
#[test]
fn the_image_of_a_new_project_holds_what_its_extractor_fetches() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    succeed(&["build", path(&project)]);
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(project.join(".spool/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["extractors"][0]["runtime"], "python");
    assert_eq!(manifest["extractors"][0]["file"], "extractors/companies.py");
    let dockerfile = std::fs::read_to_string(project.join("Dockerfile")).unwrap();
    let lines: Vec<&str> = dockerfile.lines().collect();
    let at = |line: &str| {
        lines
            .iter()
            .position(|found| *found == line)
            .unwrap_or_else(|| panic!("no `{line}` in:\n{dockerfile}"))
    };
    // What `uv` fetches goes under /spool/cache, and so does what a model's
    // library fetches, which would otherwise go to the home directory: the
    // volume at /data, which a mount hides.
    let caches =
        at("ENV UV_CACHE_DIR=/spool/cache/uv UV_PYTHON_INSTALL_DIR=/spool/cache/python \\");
    assert_eq!(lines[caches + 1], "    XDG_CACHE_HOME=/spool/cache");
    // The file is started once while the image is built, which is when it
    // loads its model, and only then is everything told to stay offline.
    let install = at("RUN spool build --install");
    let offline = at("ENV UV_OFFLINE=1 HF_HUB_OFFLINE=1");
    assert!(caches < install && install < offline, "{dockerfile}");
    assert!(
        at("COPY --from=ghcr.io/astral-sh/uv:0.13.8 /uv /usr/local/bin/uv") < install,
        "{dockerfile}"
    );
    assert!(!dockerfile.contains("bun"), "{dockerfile}");
}

/// What the image is built to do, without the image: `spool build
/// --install` with the caches the Dockerfile names, on a machine that has
/// fetched nothing, and then a server that may fetch nothing.
#[test]
#[ignore = "fetches the sample extractor's model, and only with SPOOL_TEST_MODEL=1"]
fn build_install_fetches_the_sample_model_and_a_run_then_needs_no_network() {
    if !model_asked_for() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let cache = dir.path().join("cache");
    // A home directory with nothing in it: nothing that was fetched before
    // this test is read, and nothing may be written there.
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let caches = [
        ("HOME", path(&home).to_owned()),
        ("UV_CACHE_DIR", path(&cache.join("uv")).to_owned()),
        (
            "UV_PYTHON_INSTALL_DIR",
            path(&cache.join("python")).to_owned(),
        ),
        ("XDG_CACHE_HOME", path(&cache).to_owned()),
    ];
    // `uv` itself is the one this machine has, which the image copies in.
    let uv = which("uv");

    let installed = spool()
        .args(["build", path(&project), "--install"])
        .envs(caches.iter().map(|(name, value)| (*name, value.as_str())))
        .env("SPOOL_UV", &uv)
        .output()
        .unwrap();
    assert!(
        installed.status.success(),
        "{}{}",
        text(&installed.stdout),
        text(&installed.stderr)
    );
    assert!(
        text(&installed.stdout).ends_with("starts    extractors/companies.py\n"),
        "{}",
        text(&installed.stdout)
    );
    // The weights are where the Dockerfile sends them, in one file of the
    // eight the model is published as, and nothing went to the home
    // directory.
    let snapshots =
        cache.join("huggingface/hub/models--onnx-community--gliner_small-v2.1/snapshots");
    let weights = snapshots
        .join("8142fb00740ccea973e64b1272949ff48653df5e")
        .join("onnx");
    let held: Vec<String> = std::fs::read_dir(&weights)
        .unwrap_or_else(|error| panic!("{}: {error}", weights.display()))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(held, ["model_q4f16.onnx"]);
    let megabytes = std::fs::metadata(weights.join("model_q4f16.onnx"))
        .unwrap()
        .len()
        / 1_000_000;
    assert!((200..300).contains(&megabytes), "{megabytes} MB");
    assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);

    // The server of the image: it has the same caches and is told to stay
    // offline, and its home directory is the store's volume, where nothing
    // was fetched to.
    let mut environment: Vec<(&str, &str)> = caches
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    environment.extend([
        ("UV_OFFLINE", "1"),
        ("HF_HUB_OFFLINE", "1"),
        ("SPOOL_UV", uv.as_str()),
        // And for good measure nothing it asks for can be reached.
        ("HTTPS_PROXY", "http://127.0.0.1:9"),
        ("HTTP_PROXY", "http://127.0.0.1:9"),
        ("ALL_PROXY", "http://127.0.0.1:9"),
    ]);
    let server = Server::start_in(
        &[
            "serve",
            "--manifest",
            path(&project.join(".spool/manifest.json")),
            "--project",
            path(&project),
            "--store",
            path(&dir.path().join("store.duckdb")),
            "--listen",
            "127.0.0.1:0",
        ],
        &environment,
    )
    .unwrap_or_else(|stopped| panic!("the server did not start:\n{}", stopped.output));
    assert_eq!(
        extracted(&server, &project, &["samples/notes-1.txt"]),
        FIRST
    );
}

/// Another way to find what a note names: the example extractor that asks
/// a language model over HTTP, copied into the new project as its comment
/// says. The model here is a stand-in.
#[test]
#[ignore = "runs bun"]
fn the_example_that_asks_a_model_reads_the_note_of_a_new_project() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extractors/model.ts");
    std::fs::copy(example, project.join("extractors/model.ts")).unwrap();

    // The model names a company the note gives no legal form, and one the
    // note does not name at all.
    let note = NOTE.replace("with Dana Reyes", "with Dana Reyes of Halden Group");
    write(&project.join("samples/notes-1.txt"), &note);
    let said = |answer: Value| Value::String(answer.to_string());
    let (port, requests) = model(vec![
        said(json!({
            "quotes": ["Acorn Tools Ltd", "Halden Group", "Acorn", "Zennor Glass Ltd"]
        })),
        // What a model says in place of quotes: other JSON, a quote that is
        // no text, words, and nothing.
        said(json!({ "companies": ["Tidewater Marine LLC"] })),
        said(json!({ "quotes": ["Tidewater Marine LLC", 7] })),
        json!("I found one company: Tidewater Marine LLC."),
        Value::Null,
        said(json!({ "quotes": [] })),
    ]);
    let server = Server::start_in(
        &[
            "dev",
            path(&project),
            "--listen",
            "127.0.0.1:0",
            "--seed",
            "1",
        ],
        &[
            ("OPENAI_BASE_URL", &format!("http://127.0.0.1:{port}/v1")),
            ("OPENAI_API_KEY", "a-key"),
            ("SPOOL_EXTRACT_MODEL", "a-model"),
        ],
    )
    .unwrap_or_else(|stopped| panic!("the server did not start:\n{}", stopped.output));

    let output = server
        .spool_in(
            &project,
            &["extract", "samples/notes-1.txt", "--extractor", "model"],
        )
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "\
source     notes
read       1 document from samples/notes-1.txt
loaded     1 new, 0 already held, 0 new versions
extractor  model
found      2 records in 1 document: 2 new, 0 already held

  company  Halden Group     notes:notes-1.txt[24-36]
  company  Acorn Tools Ltd  notes:notes-1.txt[70-85]
"
    );

    // The model was asked once, about the note, and told to quote.
    let asked = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(requests.try_recv().is_err());
    assert_eq!(asked.path, "/v1/chat/completions");
    assert_eq!(asked.authorization, "Bearer a-key");
    assert_eq!(asked.body["model"], "a-model");
    let messages = asked.body["messages"].as_array().unwrap();
    let system = messages[0]["content"].as_str().unwrap();
    assert!(
        system.contains("every company a document names"),
        "{system}"
    );
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], note);

    // An answer that is not quotes fails the run and says what it was, and
    // the document is not taken as read with nothing in it: the next run
    // asks about it again.
    write(
        &project.join("visit.txt"),
        "We visited Tidewater Marine LLC.\n",
    );
    let visit = [
        "extract",
        "visit.txt",
        "--source",
        "notes",
        "--extractor",
        "model",
    ];
    for answer in [
        r#"visit.txt: the model's answer is not {"quotes": ["..."]}: {"companies":["Tidewater Marine LLC"]}"#,
        r#"visit.txt: the model's answer is not {"quotes": ["..."]}: {"quotes":["Tidewater Marine LLC",7]}"#,
        r#"visit.txt: the model's answer is not {"quotes": ["..."]}: I found one company: Tidewater Marine LLC."#,
        "visit.txt: the model gave no answer: ",
    ] {
        let output = server.spool_in(&project, &visit).output().unwrap();
        let said = text(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{said}");
        assert!(said.starts_with("error: extractor `model`: "), "{said}");
        assert!(said.contains(answer), "{said}");
    }
    let nothing = shown(&server, &project, &visit);
    assert!(
        nothing.contains("found      0 records in 1 document"),
        "{nothing}"
    );
    assert_eq!(requests.try_iter().count(), 5);
}
