//! The commands a project goes through: `init`, `info`, `build`, `apply`,
//! `serve` and `dev`, run as a user runs them.

mod support;

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};
use spool::Rows;
use support::*;

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn serve_arguments<'a>(manifest: &'a Path, store: &'a Path, seed: &'a str) -> Vec<&'a str> {
    vec![
        "serve",
        "--manifest",
        path(manifest),
        "--store",
        path(store),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        seed,
    ]
}

fn load(server: &Server, table: &[&[&str]]) {
    let response = server.post(
        "/v1/load",
        json!({
            "source": "shop",
            "entity_type": "thing",
            "rows": json_rows(rows(table)),
        }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
}

fn resolve(server: &Server) -> Value {
    server.call("/v1/match", json!({ "entity_type": "thing" }))
}

#[test]
fn the_binary_starts_with_no_library_path_set() {
    let dir = tempfile::tempdir().unwrap();
    for arguments in [&["--version"][..], &["init", path(dir.path())]] {
        let output = Command::new(env!("CARGO_BIN_EXE_spool"))
            .env_clear()
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
    }
    // `init` opened DuckDB to check what it wrote.
    assert!(dir.path().join("entities/.gitkeep").is_file());
}

/// Every file under `dir`, as paths below it with `/` between their parts.
fn files_of(dir: &Path) -> Vec<String> {
    fn visit(root: &Path, dir: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap().path();
            if entry.is_dir() {
                visit(root, &entry, found);
            } else {
                let below: Vec<_> = entry
                    .strip_prefix(root)
                    .unwrap()
                    .iter()
                    .map(|part| part.to_string_lossy())
                    .collect();
                found.push(below.join("/"));
            }
        }
    }
    let mut found = Vec::new();
    visit(dir, dir, &mut found);
    found.sort();
    found
}

/// `spool init <arguments>`, run in `dir`, on a machine that has `uv`: the
/// command it is told to run Python files with is one that is there.
fn init_in(dir: &Path, arguments: &[&str]) -> std::process::Output {
    spool()
        .current_dir(dir)
        .arg("init")
        .args(arguments)
        .env("SPOOL_UV", std::env::current_exe().unwrap())
        .output()
        .unwrap()
}

/// What `spool init` writes into every project, beside what an example
/// adds.
const GITIGNORE: &str = ".spool/\n.env\n";

#[test]
fn init_makes_an_empty_project_and_says_what_goes_where() {
    let dir = tempfile::tempdir().unwrap();

    // The first thing a reader sees of a project, every line of it.
    let created = init_in(dir.path(), &["my-data"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert_eq!(text(&created.stderr), "");
    assert_eq!(
        text(&created.stdout),
        "\
created   my-data

  sources/          a YAML file for each source of rows or documents
  entities/         a folder for each entity type, with its features and its rules
  transforms/       SQL functions that features are made with
  extractors/       Python or TypeScript files that find records in text
  .gitignore        keeps the server's data, .spool/, and .env out of Git
  .env.example      the form of .env, where your code's secrets go

  cd my-data
  spool dev         runs the server, and applies each file as you save it

The project is empty. Declare a source under sources/ and an entity type
under entities/, then load your rows with `spool load`. To see a project that
works first, with sample rows: spool init companies --example companies
"
    );
    // The folders hold a file each, so that Git keeps them.
    let project = dir.path().join("my-data");
    assert_eq!(
        files_of(&project),
        [
            ".env.example",
            ".gitignore",
            "entities/.gitkeep",
            "extractors/.gitkeep",
            "sources/.gitkeep",
            "transforms/.gitkeep",
        ]
    );
    assert_eq!(read(&project.join(".gitignore")), GITIGNORE);
    // A copy of .env.example as it stands sets nothing.
    let example = read(&project.join(".env.example"));
    assert!(
        example
            .lines()
            .all(|line| line.is_empty() || line.starts_with('#')),
        "{example}"
    );
    assert!(example.contains("\n# OPENAI_API_KEY=sk-...\n"), "{example}");

    // It is a project that Spool takes as it is, and says what it lacks.
    let listing = succeed(&["info", path(&project)]);
    assert!(
        listing.contains("no entity types: add a directory under entities/ with an entity.yaml"),
        "{listing}"
    );
    assert!(!listing.contains("not used"), "{listing}");
    succeed(&["build", path(&project)]);
    assert_eq!(read(&project.join(".dockerignore")), ".git\n.spool\n.env\n");

    // A folder that is there and empty is taken, and so is the current
    // folder when it is empty but for a repository and what the Finder
    // leaves. There is nowhere to change to.
    let here = dir.path().join("here");
    std::fs::create_dir_all(here.join(".git")).unwrap();
    std::fs::write(here.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(here.join(".DS_Store"), "").unwrap();
    let created = init_in(&here, &[]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let created = text(&created.stdout);
    assert!(created.starts_with("created   .\n"), "{created}");
    assert!(!created.contains("  cd "), "{created}");
    assert!(here.join("entities/.gitkeep").is_file());
    assert_eq!(read(&here.join(".git/HEAD")), "ref: refs/heads/main\n");
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let created = init_in(dir.path(), &["empty"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(text(&created.stdout).contains("\n  cd empty\n"));

    // A directory that is not a plain word is quoted for the shell.
    let created = init_in(dir.path(), &["it's here"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let created = text(&created.stdout);
    assert!(created.contains("\n  cd 'it'\\''s here'\n"), "{created}");
}

#[test]
fn init_writes_nothing_into_a_folder_that_holds_files() {
    let dir = tempfile::tempdir().unwrap();

    // A home folder, where `spool init` was run without a name.
    let home = dir.path().join("home");
    for folder in ["Documents", "Downloads", ".config"] {
        std::fs::create_dir_all(home.join(folder)).unwrap();
    }
    std::fs::write(home.join("notes.txt"), "mine\n").unwrap();
    std::fs::write(home.join(".zshrc"), "mine\n").unwrap();
    let before = files_of(&home);
    let refused = init_in(&home, &[]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(text(&refused.stdout), "");
    assert_eq!(
        text(&refused.stderr),
        "spool: this folder already holds files (Documents, Downloads, notes.txt and 2 more), \
         and `spool init` makes a project only in an empty one. Nothing was written. \
         `spool init <name>` makes the project in a new folder of that name\n"
    );
    assert_eq!(files_of(&home), before);
    assert!(!home.join("entities").exists());

    // A folder that is named, with one file in it.
    let taken = dir.path().join("taken");
    std::fs::create_dir(&taken).unwrap();
    std::fs::write(taken.join("rows.csv"), "mine\n").unwrap();
    let refused = init_in(dir.path(), &["taken"]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        text(&refused.stderr),
        "spool: taken already holds files (rows.csv). `spool init` makes a project only in a \
         new or empty folder: give it another name\n"
    );
    assert_eq!(files_of(&taken), ["rows.csv"]);

    // A project, named or not.
    assert!(init_in(dir.path(), &["project"]).status.success());
    let refused = init_in(dir.path(), &["project", "--example", "companies"]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        text(&refused.stderr),
        "spool: project is a project already: it has an entities/ folder\n"
    );
    assert!(!dir.path().join("project/samples").exists());
    let refused = init_in(&dir.path().join("project"), &[]);
    assert_eq!(
        text(&refused.stderr),
        "spool: this folder is a project already: it has an entities/ folder\n"
    );

    // A file.
    std::fs::write(dir.path().join("a-file"), "mine\n").unwrap();
    let refused = init_in(dir.path(), &["a-file"]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        text(&refused.stderr),
        "spool: a-file is a file, and a project is a folder\n"
    );
    assert_eq!(read(&dir.path().join("a-file")), "mine\n");
}

#[test]
fn init_with_an_example_says_what_it_wrote_and_what_to_run_next() {
    let dir = tempfile::tempdir().unwrap();

    let created = init_in(dir.path(), &["companies", "--example", "companies"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert_eq!(
        text(&created.stdout),
        "\
created   companies

  sources/crm.yaml                   the columns of a CRM export
  sources/notes.yaml                 what a record found in a note may say
  entities/company/entity.yaml       what companies are compared by
  entities/company/rules/            three rules, one line of SQL each
  transforms/name_key.sql            the lower-case name the rules compare
  extractors/companies.py            finds company names in text, with a small model
  samples/crm-1.csv                  six rows to load first
  samples/crm-2.csv                  three more for later
  samples/notes-1.txt                a call note that names three companies
  .gitignore                         keeps the server's data, .spool/, and .env out of Git
  .env.example                       the form of .env, where your code's secrets go

  cd companies
  spool dev                          runs the server on this machine
  spool load samples/crm-1.csv       loads the first rows, from a second terminal
  spool extract samples/notes-1.txt  finds the companies in the note

The extractor is a Python file, which Spool runs with `uv`. Its first run fetches
about 330 MB, most of it the model, and later runs fetch nothing.
"
    );
    let project = dir.path().join("companies");
    assert_eq!(
        files_of(&project),
        [
            ".env.example",
            ".gitignore",
            "entities/company/entity.yaml",
            "entities/company/rules/block/name_prefix.sql",
            "entities/company/rules/match/similar_name.sql",
            "entities/company/rules/no_match/different_country.sql",
            "extractors/companies.py",
            "samples/crm-1.csv",
            "samples/crm-2.csv",
            "samples/notes-1.txt",
            "sources/crm.yaml",
            "sources/notes.yaml",
            "transforms/name_key.sql",
        ]
    );
    assert_eq!(read(&project.join(".gitignore")), GITIGNORE);
    assert_eq!(
        read(&project.join("samples/crm-1.csv")),
        "\
id,name,country
c-1,Acorn Tools,GB
c-2,ACORN TOOLS,GB
c-3,Acorn Tools Ltd,GB
c-4,Larkfield Logistics,GB
c-5,Halden Group,GB
c-6,Halden Group,US
"
    );
    assert_eq!(
        read(&project.join("samples/crm-2.csv")),
        "\
id,name,country
c-7,Larkfeild Logistics,GB
c-8,Kestrel Analytics,US
c-9,Acorn Tool,GB
"
    );
    // Each rule is one line of SQL under a comment that says what it is.
    for (rule, sql) in [
        (
            "block/name_prefix.sql",
            "left(l.name_key, 4) = left(r.name_key, 4)",
        ),
        (
            "match/similar_name.sql",
            "jaro_winkler_similarity(l.name_key, r.name_key) >= 0.92",
        ),
        ("no_match/different_country.sql", "l.country <> r.country"),
    ] {
        let rule = read(&project.join("entities/company/rules").join(rule));
        let lines: Vec<&str> = rule
            .lines()
            .filter(|line| !line.starts_with("--"))
            .collect();
        assert_eq!(lines, [sql], "{rule}");
    }

    // An example that does not exist is refused with the ones that do, and
    // nothing is made.
    let refused = init_in(dir.path(), &["people", "--example", "people"]);
    assert_eq!(refused.status.code(), Some(2));
    let complaint = text(&refused.stderr);
    assert!(
        complaint.starts_with(
            "spool: there is no example `people`: the examples are `companies`\n\nUsage: spool init"
        ),
        "{complaint}"
    );
    assert!(!dir.path().join("people").exists());
    // The options that named an example's entity type and source are gone.
    for option in ["--entity", "--source"] {
        let refused = init_in(dir.path(), &["other", option, "person"]);
        assert_eq!(refused.status.code(), Some(2), "{option}");
        assert!(!dir.path().join("other").exists());
    }

    // On a machine without `uv`, which the last step needs and the others
    // do not, the reader is told so now, with the line that installs it:
    // the project is written all the same.
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let created = spool()
        .current_dir(dir.path())
        .args(["init", "bare", "--example", "companies"])
        .env("PATH", &empty)
        .env("HOME", &empty)
        .env_remove("SPOOL_UV")
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert_eq!(text(&created.stderr), "");
    let created = text(&created.stdout);
    assert!(
        created.ends_with(
            "
  spool extract samples/notes-1.txt  finds the companies in the note

The extractor is a Python file, which Spool runs with `uv`. Its first run fetches
about 330 MB, most of it the model, and later runs fetch nothing.
`uv` is not installed here. Before `spool extract`, install it:

  curl -LsSf https://astral.sh/uv/install.sh | sh
"
        ),
        "{created}"
    );
    assert_eq!(files_of(&dir.path().join("bare")), files_of(&project));
    // It is found where its installer puts it, with the search path as it
    // was.
    let installed = empty.join(".local/bin/uv");
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), &installed).unwrap();
    let created = spool()
        .current_dir(dir.path())
        .args(["init", "ready", "--example", "companies"])
        .env("PATH", &empty)
        .env("HOME", &empty)
        .env_remove("SPOOL_UV")
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(
        text(&created.stdout).ends_with("and later runs fetch nothing.\n"),
        "{}",
        text(&created.stdout)
    );
    // An empty project has no code to run, and says nothing of `uv`.
    let created = spool()
        .current_dir(dir.path())
        .args(["init", "plain"])
        .env("PATH", &empty)
        .env("HOME", dir.path().join("nobody"))
        .env_remove("SPOOL_UV")
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(
        !text(&created.stdout).contains("uv"),
        "{}",
        text(&created.stdout)
    );
}

#[test]
fn init_scaffolds_a_project_that_info_lists_and_build_compiles() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("crm");

    let created = succeed(&["init", path(&project), "--example", "companies"]);
    assert!(
        created.starts_with(&format!("created   {}\n", project.display())),
        "{created}"
    );

    let listing = succeed(&["info", path(&project)]);
    assert!(listing.contains("entity type `company`"), "{listing}");
    assert!(
        listing
            .contains("features  record_id UUID, name VARCHAR, name_key VARCHAR, country VARCHAR"),
        "{listing}"
    );
    assert!(listing.contains("block     name_prefix"), "{listing}");
    assert!(listing.contains("no_match  different_country"), "{listing}");
    assert!(listing.contains("match     similar_name"), "{listing}");
    // The sample rows are the reader's to load, and no part of the project.
    assert!(!listing.contains("samples"), "{listing}");
    assert!(!listing.contains("not used"), "{listing}");
    let listing: Value =
        serde_json::from_str(&succeed(&["info", path(&project), "--json"])).unwrap();
    assert_eq!(listing["entity_types"][0]["name"], "company");
    assert_eq!(
        listing["entity_types"][0]["rules"],
        json!({
            "block": ["name_prefix"],
            "no_match": ["different_country"],
            "match": ["similar_name"],
        })
    );
    assert_eq!(listing["ignored"], json!([]));
    assert_eq!(listing["warnings"], json!([]));

    let built = succeed(&["build", path(&project)]);
    let manifest: Value =
        serde_json::from_str(&read(&project.join(".spool/manifest.json"))).unwrap();
    assert_eq!(manifest["hash"], listing["manifest_hash"]);
    assert!(
        built.contains(manifest["hash"].as_str().unwrap()),
        "{built}"
    );

    // The first build writes a Dockerfile, which then belongs to the project.
    // The image is not to carry the project's secrets.
    let dockerfile = read(&project.join("Dockerfile"));
    assert!(
        dockerfile.contains(&format!(
            "FROM ghcr.io/alexpatow/spool:{}",
            env!("CARGO_PKG_VERSION")
        )),
        "{dockerfile}"
    );
    assert!(dockerfile.contains("RUN spool build"), "{dockerfile}");
    assert_eq!(read(&project.join(".dockerignore")), ".git\n.spool\n.env\n");
    std::fs::write(project.join("Dockerfile"), "FROM mine\n").unwrap();
    let rebuilt = succeed(&["build", path(&project)]);
    assert!(!rebuilt.contains("Dockerfile"), "{rebuilt}");
    assert_eq!(read(&project.join("Dockerfile")), "FROM mine\n");
    let refused = run(&["build", path(&project), "--base-image", "other"]);
    assert_eq!(refused.status.code(), Some(1));

    let other = dir.path().join("other");
    succeed(&["init", path(&other)]);
    succeed(&[
        "build",
        path(&other),
        "--base-image",
        "registry.example/spool:1",
    ]);
    assert!(read(&other.join("Dockerfile")).contains("FROM registry.example/spool:1"));
}

#[test]
fn a_project_that_does_not_compile_is_reported_with_its_file() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let rule = dir.path().join("entities/thing/rules/match/same_code.sql");
    std::fs::write(&rule, "l.code = r.no_such_column\n").unwrap();

    for command in ["info", "build"] {
        let output = run(&[command, path(dir.path())]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        let stderr = text(&output.stderr);
        assert!(stderr.starts_with("spool: "), "{stderr}");
        assert!(stderr.contains("same_code.sql"), "{stderr}");
        assert!(stderr.contains("no_such_column"), "{stderr}");
    }
    assert!(!dir.path().join(".spool/manifest.json").exists());
}

#[test]
fn apply_declares_types_and_rebuilds_the_features_of_a_changed_declaration() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = build(&project);
    let store = dir.path().join("store.duckdb");

    let applied = succeed(&["apply", path(&project), "--store", path(&store)]);
    assert!(applied.contains("declared  thing"), "{applied}");
    let again = succeed(&[
        "apply",
        "--manifest",
        path(&manifest),
        "--store",
        path(&store),
    ]);
    assert!(again.contains("unchanged thing"), "{again}");
    assert!(again.contains("already at this manifest"), "{again}");

    // The store path can come from the environment, as it does in an image.
    let output = spool()
        .args(["apply", path(&project)])
        .env("SPOOL_STORE", &store)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));

    // Put two records in, then change what a thing's code is.
    let server = Server::start(&serve_arguments(&manifest, &store, "1"));
    load(&server, &[&["id", "code"], &["1", "anvil"]]);
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );
    server.stop();

    std::fs::write(
        project.join("entities/thing/entity.yaml"),
        thing_entity("{ from: code, transform: lower }"),
    )
    .unwrap();
    build(&project);
    let applied = succeed(&["apply", path(&project), "--store", path(&store)]);
    assert!(applied.contains("rebuilt   thing"), "{applied}");

    // Identities are untouched, and the new features are what match now.
    let server = Server::start(&serve_arguments(&manifest, &store, "2"));
    let info = server.read("/v1/info");
    assert_eq!(info["store"]["entity_types"][0]["declaration_version"], 2);
    assert_eq!(info["store"]["entity_types"][0]["identities"], 1);
    load(&server, &[&["id", "code"], &["2", "ANVIL"]]);
    let resolution = resolve(&server)["run"]["resolution"].clone();
    assert_eq!(resolution["identities_created"], 0, "{resolution}");
    assert_eq!(resolution["records_assigned"], 1, "{resolution}");

    // A manifest that was edited by hand is not applied.
    let edited = read(&manifest).replace("lower(", "upper(");
    std::fs::write(&manifest, edited).unwrap();
    let output = run(&["apply", path(&project), "--store", path(&store)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("manifest.json"));
}

#[test]
fn serve_sets_up_an_empty_store_and_refuses_one_applied_with_another_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = build(&project);
    let store = dir.path().join("store.duckdb");

    // An empty store is set up from the manifest.
    let server = Server::start(&serve_arguments(&manifest, &store, "1"));
    let hash = server.read("/v1/info")["store"]["manifest_hash"].clone();
    load(
        &server,
        &[&["id", "code"], &["1", "anvil"], &["2", "anvil"]],
    );
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );
    server.stop();

    // The same manifest starts again on the same store, which kept its state.
    let server = Server::start(&serve_arguments(&manifest, &store, "2"));
    let info = server.read("/v1/info");
    assert_eq!(info["store"]["manifest_hash"], hash);
    assert_eq!(info["store"]["head"], 1);
    assert_eq!(info["store"]["entity_types"][0]["identities"], 1);
    server.stop();

    // A changed project is refused until it is applied.
    std::fs::remove_file(project.join("entities/thing/rules/no_match/other_colour.sql")).unwrap();
    build(&project);
    let stopped = Server::start_with(&serve_arguments(&manifest, &store, "3")).unwrap_err();
    assert_eq!(stopped.code, Some(1));
    assert!(
        stopped.output.contains(hash.as_str().unwrap()),
        "{}",
        stopped.output
    );
    assert!(stopped.output.contains("spool apply"), "{}", stopped.output);

    succeed(&["apply", path(&project), "--store", path(&store)]);
    let server = Server::start(&serve_arguments(&manifest, &store, "3"));
    let info = server.read("/v1/info");
    assert_ne!(info["store"]["manifest_hash"], hash);
    assert_eq!(info["store"]["entity_types"][0]["identities"], 1);
    server.stop();

    // No manifest, no server.
    std::fs::remove_file(&manifest).unwrap();
    let stopped = Server::start_with(&serve_arguments(&manifest, &store, "4")).unwrap_err();
    assert_eq!(stopped.code, Some(1));
    assert!(
        stopped.output.contains("manifest.json"),
        "{}",
        stopped.output
    );
}

#[test]
fn a_run_cut_short_by_a_killed_server_is_recorded_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    let manifest = build(&project);
    let store = dir.path().join("store.duckdb");

    let server = Server::start(&serve_arguments(&manifest, &store, "1"));
    load(&server, &[&["id", "code"], &["1", "anvil"]]);
    let ids: Vec<String> = (0..300_000).map(|index| format!("bulk-{index}")).collect();
    let bulk: Vec<Vec<Option<&str>>> = ids
        .iter()
        .map(|id| vec![Some(id.as_str()), Some(id.as_str())])
        .collect();
    let response = server.post_arrow(
        "/v1/load?source=bulk&entity_type=thing&wait_ms=0",
        Rows::from_text(&["id", "code"], &bulk).unwrap(),
    );
    let accepted = response.expect(202);
    assert_eq!(accepted["run"]["status"], "running");
    let run = accepted["run"]["run"].as_u64().unwrap();
    server.kill();

    let server = Server::start(&serve_arguments(&manifest, &store, "2"));
    let record = server.read(&format!("/v1/runs/{run}"));
    assert_eq!(record["run"]["status"], "failed", "{record}");
    let error = record["run"]["error"].as_str().unwrap();
    assert!(error.starts_with("interrupted"), "{error}");
    assert!(record["run"]["finished_at"].is_string());
    assert_eq!(server.read("/v1/runs?status=running")["runs"], json!([]));

    // Nothing of the batch was kept, and the store works as before.
    let info = server.read("/v1/info");
    assert_eq!(info["store"]["entity_types"][0]["records"], 1);
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );
}

#[test]
fn dev_serves_a_project_directory_and_applies_changes_as_they_are_saved() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let entity = dir.path().join("entities/thing/entity.yaml");
    // A transform that cannot work on a code that is a word. Nothing uses it yet.
    std::fs::create_dir_all(dir.path().join("transforms")).unwrap();
    std::fs::write(
        dir.path().join("transforms/number.sql"),
        "lambda code: code::INTEGER\n",
    )
    .unwrap();
    let folded = thing_entity("{ from: code, transform: lower }");

    let server = Server::start_with(&[
        "dev",
        path(dir.path()),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ])
    .unwrap();
    assert!(dir.path().join(".spool/dev.duckdb").is_file());
    let first = server.read("/v1/info")["store"]["manifest_hash"].clone();

    load(&server, &[&["id", "code"], &["1", "anvil"]]);
    assert_eq!(
        resolve(&server)["run"]["resolution"]["identities_created"],
        1
    );

    // A saved change to the declaration is applied without a restart.
    std::fs::write(&entity, &folded).unwrap();
    let applied = server.printed("applied");
    assert!(applied.contains("rebuilt thing"), "{applied}");
    let second = server.read("/v1/info")["store"]["manifest_hash"].clone();
    assert_ne!(second, first);
    assert!(applied.contains(second.as_str().unwrap()), "{applied}");
    load(&server, &[&["id", "code"], &["2", "ANVIL"]]);
    let resolution = resolve(&server)["run"]["resolution"].clone();
    assert_eq!(resolution["identities_created"], 0, "{resolution}");
    assert_eq!(resolution["records_assigned"], 1, "{resolution}");

    // A change that does not compile is reported, and the server carries on
    // with what it had.
    std::fs::write(&entity, "features: [code\n").unwrap();
    let complaint = server.complained("entity.yaml");
    assert!(complaint.starts_with("spool: "), "{complaint}");
    server.complained("still running the last version that compiled");
    assert_eq!(server.read("/v1/info")["store"]["manifest_hash"], second);

    // So is a change that compiles and that the stored records do not
    // fit: here, a code that has to be a number.
    std::fs::write(&entity, thing_entity("{ from: code, transform: number }")).unwrap();
    server.complained("still running the last version that compiled");
    assert_eq!(server.read("/v1/info")["store"]["manifest_hash"], second);
    load(&server, &[&["id", "code"], &["3", "Anvil"]]);
    let resolution = resolve(&server)["run"]["resolution"].clone();
    assert_eq!(resolution["records_assigned"], 1, "{resolution}");
    assert_eq!(resolution["identities_created"], 0, "{resolution}");

    // A rule is only a rule: adding one rebuilds nothing.
    std::fs::write(&entity, &folded).unwrap();
    std::fs::write(
        dir.path().join("entities/thing/rules/match/never.sql"),
        "false\n",
    )
    .unwrap();
    let applied = server.printed("applied");
    assert!(applied.contains("rules only"), "{applied}");
    let third = server.read("/v1/info")["store"]["manifest_hash"].clone();
    assert_ne!(third, second);

    server.stop();
}

/// The rows of a sample file, as the objects `POST /v1/load` takes: the
/// first line is the column names, and no value has a comma in it.
fn sample_rows(file: &Path) -> Vec<Value> {
    let contents = read(file);
    let mut lines = contents.lines();
    let columns: Vec<&str> = lines.next().unwrap().split(',').collect();
    lines
        .map(|line| {
            let values: Vec<&str> = line.split(',').collect();
            assert_eq!(values.len(), columns.len(), "{line}");
            Value::Object(
                columns
                    .iter()
                    .zip(values)
                    .map(|(column, value)| ((*column).to_owned(), value.into()))
                    .collect(),
            )
        })
        .collect()
}

/// Loads a sample file of a new project into its server, and returns what
/// the load did.
fn load_sample(server: &Server, file: &Path) -> Value {
    let loaded = server.call(
        "/v1/load",
        json!({ "source": "crm", "entity_type": "company", "rows": sample_rows(file) }),
    );
    assert_eq!(loaded["run"]["status"], "succeeded", "{loaded}");
    loaded["run"]["load"].clone()
}

/// Matches the companies that were loaded, and returns what the run did.
fn match_companies(server: &Server) -> Value {
    let matched = server.call("/v1/match", json!({ "entity_type": "company" }));
    assert_eq!(matched["run"]["status"], "succeeded", "{matched}");
    matched["run"]["resolution"].clone()
}

/// The records of the source `crm`, by key: each one's record ID, the name
/// it is called by, and its identity.
fn companies(server: &Server) -> std::collections::BTreeMap<String, (String, String, String)> {
    let text = |value: &Value| value.as_str().unwrap().to_owned();
    server.read("/v1/records?source=crm")["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            (
                text(&record["key"]),
                (
                    text(&record["id"]),
                    text(&record["label"]),
                    text(&record["identity"]),
                ),
            )
        })
        .collect()
}

#[test]
fn the_sample_rows_of_a_new_project_match_and_keep_their_identities() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("companies");
    succeed(&["init", path(&project), "--example", "companies"]);
    let server = Server::start(&[
        "dev",
        path(&project),
        "--listen",
        "127.0.0.1:0",
        "--seed",
        "1",
    ]);
    let explain = |a: &str, b: &str| {
        let url = format!("http://{}", server.address);
        succeed(&["explain", a, b, "--url", &url])
    };

    // The first file: six rows, and four companies.
    let first = project.join("samples/crm-1.csv");
    let loaded = load_sample(&server, &first);
    assert_eq!(loaded["received"], 6, "{loaded}");
    assert_eq!(loaded["inserted"], 6, "{loaded}");
    let matched = match_companies(&server);
    assert_eq!(matched["new_records"], 6, "{matched}");
    assert_eq!(matched["candidates"], 4, "{matched}");
    assert_eq!(
        matched["judgements"],
        json!({ "match": 3, "no_match": 1, "unsure": 0 })
    );
    assert_eq!(matched["records_assigned"], 6, "{matched}");
    assert_eq!(matched["identities_created"], 4, "{matched}");
    assert_eq!(matched["review"], json!([]));

    let before = companies(&server);
    let identity = |key: &str| before[key].2.as_str();
    let names: Vec<(&str, &str)> = before
        .iter()
        .map(|(key, (_, name, _))| (key.as_str(), name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("c-1", "Acorn Tools"),
            ("c-2", "ACORN TOOLS"),
            ("c-3", "Acorn Tools Ltd"),
            ("c-4", "Larkfield Logistics"),
            ("c-5", "Halden Group"),
            ("c-6", "Halden Group"),
        ]
    );
    // Three spellings of Acorn Tools are one company. The two Halden Groups
    // are in different countries, and are two.
    assert_eq!(identity("c-2"), identity("c-1"));
    assert_eq!(identity("c-3"), identity("c-1"));
    let apart = ["c-1", "c-4", "c-5", "c-6"].map(identity);
    for (index, one) in apart.iter().enumerate() {
        assert!(!apart[index + 1..].contains(one), "{apart:?}");
    }
    // Each for the reason its rule gives.
    let why = explain("crm:c-1", "crm:c-3");
    assert!(
        why.contains(
            "Run 2 (strategy `rules`) judged them `match` (match rule `similar_name` held)."
        ),
        "{why}"
    );
    let why = explain("crm:c-5", "crm:c-6");
    assert!(
        why.contains(
            "Run 2 (strategy `rules`) judged them `no_match` (no_match rule `different_country` held)."
        ),
        "{why}"
    );

    // Loading the file again adds nothing.
    let again = load_sample(&server, &first);
    assert_eq!(again["inserted"], 0, "{again}");
    assert_eq!(again["duplicates"], 6, "{again}");

    // The second file: two misspellings join the companies they are, which
    // keep their IDs, and one company is new.
    let loaded = load_sample(&server, &project.join("samples/crm-2.csv"));
    assert_eq!(loaded["inserted"], 3, "{loaded}");
    let matched = match_companies(&server);
    assert_eq!(matched["new_records"], 3, "{matched}");
    assert_eq!(matched["candidates"], 4, "{matched}");
    assert_eq!(
        matched["judgements"],
        json!({ "match": 4, "no_match": 0, "unsure": 0 })
    );
    assert_eq!(matched["records_assigned"], 3, "{matched}");
    assert_eq!(matched["identities_created"], 1, "{matched}");
    assert_eq!(matched["review"], json!([]));

    let after = companies(&server);
    for (key, record) in &before {
        assert_eq!(&after[key], record, "{key} is as it was");
    }
    let (_, name, joined) = &after["c-7"];
    assert_eq!(name, "Larkfeild Logistics");
    assert_eq!(joined, identity("c-4"));
    let (_, name, joined) = &after["c-9"];
    assert_eq!(name, "Acorn Tool");
    assert_eq!(joined, identity("c-1"));
    let (_, name, new) = &after["c-8"];
    assert_eq!(name, "Kestrel Analytics");
    assert!(
        before.values().all(|(_, _, identity)| identity != new),
        "{after:?}"
    );
    let info = server.read("/v1/info");
    assert_eq!(info["store"]["entity_types"][0]["records"], 9, "{info}");
    assert_eq!(info["store"]["entity_types"][0]["identities"], 5, "{info}");

    // A record's ID is made from its source, its key and what the row
    // holds, so these are the ones every reader gets, and a page that shows
    // the sample prints them. If a sample row changes, so does that page.
    let ids: Vec<(&str, &str)> = after
        .iter()
        .map(|(key, (id, _, _))| (key.as_str(), id.as_str()))
        .collect();
    assert_eq!(
        ids,
        [
            ("c-1", "ab8e9d59-0aaa-96b9-ddb7-db0bff7a9356"),
            ("c-2", "a780fcfe-cb2f-21dd-25ea-11db898fd6a2"),
            ("c-3", "31075f20-6900-838b-f5af-f8c6576744a4"),
            ("c-4", "4b1bc703-9041-0989-d0eb-4df5e9eae0f2"),
            ("c-5", "38f040c9-82b9-2f94-813b-697309e52dab"),
            ("c-6", "34a5d12a-11a0-e461-c6e4-0576f546d165"),
            ("c-7", "bdbab4b2-e260-2b34-b08f-3fae6a2018a9"),
            ("c-8", "7da2b0d2-3601-e533-efa5-e1068cce5344"),
            ("c-9", "34c3d52d-e53b-1559-7404-1892fb1fd4e3"),
        ]
    );

    server.stop();
}

#[test]
fn a_second_server_on_a_store_or_a_port_says_what_is_wrong() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("companies");
    succeed(&["init", path(&project)]);
    let store = project.join(".spool/dev.duckdb");
    let dev = |project: &Path, listen: &str| {
        Server::start_with(&["dev", path(project), "--listen", listen])
    };
    let running = dev(&project, "127.0.0.1:0").unwrap();

    // The same project again. DuckDB refuses the second process the store,
    // in words about a lock and a page of its own documentation.
    let stopped = dev(&project, "127.0.0.1:0").unwrap_err();
    assert_eq!(stopped.code, Some(1));
    let said = stopped.output;
    assert!(
        said.starts_with(&format!(
            "spool: the store {} is open in another process",
            store.display()
        )),
        "{said}"
    );
    assert!(
        said.contains("most likely a `spool dev` that is already running on this project"),
        "{said}"
    );
    assert!(said.contains("stop it with Ctrl-C"), "{said}");
    for raw in ["IO Error", "lock", "duckdb.org"] {
        assert!(!said.contains(raw), "{said}");
    }

    // Any other command that opens the store is told the same thing.
    build(&project);
    let applied = run(&["apply", path(&project), "--store", path(&store)]);
    assert_eq!(applied.status.code(), Some(1));
    let said = text(&applied.stderr);
    assert!(
        said.starts_with(&format!(
            "spool: the store at {} is open in another process",
            store.display()
        )),
        "{said}"
    );
    assert!(said.contains("stop that one first"), "{said}");
    assert!(!said.contains("duckdb.org"), "{said}");

    // Another project on the port the first one has.
    let other = dir.path().join("other");
    succeed(&["init", path(&other)]);
    let stopped = dev(&other, &running.address).unwrap_err();
    assert_eq!(stopped.code, Some(1));
    let said = stopped.output;
    let (host, port) = running.address.rsplit_once(':').unwrap();
    let next = match port.parse::<u16>().unwrap() {
        u16::MAX => u16::MAX - 1,
        port => port + 1,
    };
    let next = format!("{host}:{next}");
    assert!(
        said.contains(&format!(
            "spool: cannot listen on {}: something else is listening there",
            running.address
        )),
        "{said}"
    );
    assert!(said.contains(&format!("`--listen {next}`")), "{said}");
    assert!(said.contains(&format!("SPOOL_URL=http://{next}")), "{said}");
    assert!(!said.contains("os error"), "{said}");

    // The first server is as it was, and the second project starts on a
    // port of its own.
    assert_eq!(running.read("/v1/info")["store"]["head"], 0);
    let second = dev(&other, "127.0.0.1:0").unwrap();
    assert_eq!(second.read("/v1/info")["store"]["head"], 0);
    second.stop();
    running.stop();
}
