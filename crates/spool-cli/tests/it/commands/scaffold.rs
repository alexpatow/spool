//! The project `spool init` writes: its example, and its sample rows.

use super::*;

/// The files that are not for coding agents: what `spool update-skills` writes is
/// tested on its own.
fn without_agent_files(files: Vec<String>) -> Vec<String> {
    files
        .into_iter()
        .filter(|file| {
            ![".agents/", ".claude/", "AGENTS.md", "CLAUDE.md"]
                .iter()
                .any(|agent| file.starts_with(agent))
        })
        .collect()
}

#[test]
fn init_with_an_example_says_what_it_wrote_and_what_to_run_next() {
    let dir = tempfile::tempdir().unwrap();

    let created = init_in(dir.path(), &["companies", "--example", "companies"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(text(&created.stderr).starts_with("\n  cd companies\n"));
    assert_eq!(
        format!("{}{}", text(&created.stdout), text(&created.stderr)),
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
  AGENTS.md                          what a coding agent should know here; CLAUDE.md imports it
  .agents/skills/                    skills that teach coding agents Spool, linked from .claude/skills/

  cd companies
  spool dev                          runs the server on this machine
  spool load samples/crm-1.csv       loads the first rows, from a second terminal
  spool extract samples/notes-1.txt  finds the companies in the note

The extractor is a Python file, which Spool runs with `uv`. Its first run fetches
about 330 MB, most of it the model, and later runs fetch nothing.
"
    );
    let project = dir.path().join("companies");
    assert!(project.join(".git/HEAD").is_file());
    assert_eq!(
        without_agent_files(files_of(&project)),
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

    refuse_unknown_examples(dir.path());
    tell_of_a_missing_uv(dir.path(), &project);
}

/// An example that does not exist is refused with the ones that do, and
/// nothing is made.
fn refuse_unknown_examples(dir: &Path) {
    let refused = init_in(dir, &["people", "--example", "people"]);
    assert_eq!(refused.status.code(), Some(2));
    let complaint = text(&refused.stderr);
    assert!(
        complaint.starts_with(
            "error: invalid value 'people' for '--example <NAME>'\n  [possible values: companies]\n"
        ),
        "{complaint}"
    );
    assert!(!dir.join("people").exists());
    // The options that named an example's entity type and source are gone.
    for option in ["--entity", "--source"] {
        let refused = init_in(dir, &["other", option, "person"]);
        assert_eq!(refused.status.code(), Some(2), "{option}");
        assert!(!dir.join("other").exists());
    }
}

/// On a machine without `uv`, which the last step needs and the others do
/// not, the reader is told so now, with the line that installs it: the
/// project is written all the same.
fn tell_of_a_missing_uv(dir: &Path, project: &Path) {
    let empty = dir.join("empty");
    std::fs::create_dir(&empty).unwrap();
    // The isolated PATH also has no Git. Give each project a repository first.
    for name in ["bare", "ready", "plain"] {
        git_init(&dir.join(name));
    }
    let created = spool()
        .current_dir(dir)
        .args(["init", "bare", "--example", "companies"])
        .env("PATH", &empty)
        .env("HOME", &empty)
        .env_remove("SPOOL_UV")
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", text(&created.stderr));
    let created = text(&created.stderr);
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
    assert_eq!(files_of(&dir.join("bare")), files_of(project));
    // It is found where its installer puts it, with the search path as it
    // was.
    let installed = empty.join(".local/bin/uv");
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), &installed).unwrap();
    let created = spool()
        .current_dir(dir)
        .args(["init", "ready", "--example", "companies"])
        .env("PATH", &empty)
        .env("HOME", &empty)
        .env_remove("SPOOL_UV")
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(
        text(&created.stderr).ends_with("and later runs fetch nothing.\n"),
        "{}",
        text(&created.stderr)
    );
    // An empty project has no code to run, and says nothing of `uv`.
    let created = spool()
        .current_dir(dir)
        .args(["init", "plain"])
        .env("PATH", &empty)
        .env("HOME", dir.join("nobody"))
        .env_remove("SPOOL_UV")
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", text(&created.stderr));
    let said = format!("{}{}", text(&created.stdout), text(&created.stderr));
    assert!(!said.contains("uv"), "{said}");
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
    // Each source's columns, and the features each becomes.
    assert!(
        listing.contains(
            "\nsource `crm`\n  read by   company\n  column   becomes, in `company`\n  \
             id       the key of each row\n  name     name, name_key = name_key(name)\n  \
             country  country\n"
        ),
        "{listing}"
    );
    assert!(
        listing.contains(
            "\nsource `notes`\n  read by   company\n  column  becomes, in `company`\n  \
             name    name, name_key = name_key(name)\n"
        ),
        "{listing}"
    );
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
    assert_eq!(
        listing["entity_types"][0]["mapping"]["crm"]["name_key"],
        json!({ "from": ["name"], "transform": "name_key" })
    );
    assert_eq!(
        listing["sources"][0],
        json!({ "name": "crm", "key": "id", "columns": ["id", "name", "country"] })
    );
    assert_eq!(listing["ignored"], json!([]));
    assert_eq!(listing["warnings"], json!([]));

    // With more than one type, each column says which type it serves, and
    // a source no type reads says so.
    let two = dir.path().join("two");
    write_two_of_each(&two);
    let two_listing = succeed(&["info", path(&two)]);
    assert!(
        two_listing.contains(
            "\nsource `crm`\n  read by   company, person\n  column  becomes\n  \
             id      the key of each row\n  name    company: name; person: name\n"
        ),
        "{two_listing}"
    );
    assert!(
        two_listing.contains(
            "\nsource `spare`\n  read by   no entity type: no mapping in entities/*/entity.yaml names it\n  \
             column  becomes\n  id      the key of each row\n  name    no feature\n"
        ),
        "{two_listing}"
    );

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
