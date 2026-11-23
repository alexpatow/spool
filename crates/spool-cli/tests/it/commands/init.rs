//! `spool init` in an empty folder, in git, and where it may not write; and `spool update`.

use super::*;

#[test]
fn init_makes_an_empty_project_and_says_what_goes_where() {
    let dir = tempfile::tempdir().unwrap();

    // The first thing a reader sees of a project, every line of it.
    let created = init_in(dir.path(), &["my-data"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    // What it made is the result, and what to do next is said beside it.
    assert!(
        text(&created.stdout)
            .ends_with("  .agents/skills/      skills that teach coding agents Spool, linked from .claude/skills/\n")
    );
    assert_eq!(
        format!("{}{}", text(&created.stdout), text(&created.stderr)),
        "\
created   my-data

  sources/             a YAML file for each source of rows or documents
  entities/            a folder for each entity type, with its features and its rules
  transforms/          SQL functions that features are made with
  extractors/          Python or TypeScript files that find records in text
  .gitignore           keeps the server's data, .spool/, and .env out of Git
  .env.example         the form of .env, where your code's secrets go
  AGENTS.md            what a coding agent should know here; CLAUDE.md imports it
  .agents/skills/      skills that teach coding agents Spool, linked from .claude/skills/

  cd my-data
  spool dev            runs the server, and applies each file as you save it

The project is empty. Declare a source under sources/ and an entity type
under entities/, then load your rows with `spool load`. To see a project that
works first, with sample rows: spool init companies --example companies
"
    );
    // The folders hold a file each, so that Git keeps them.
    let project = dir.path().join("my-data");
    assert!(project.join(".git/HEAD").is_file());
    let git = |arguments: &[&str]| {
        Command::new("git")
            .current_dir(&project)
            .args(arguments)
            .output()
            .unwrap()
    };
    let index = git(&["ls-files"]);
    assert!(index.status.success(), "{}", text(&index.stderr));
    assert!(index.stdout.is_empty());
    assert!(!git(&["rev-parse", "--verify", "HEAD"]).status.success());
    let ignored = git(&["check-ignore", ".env", ".spool/state.duckdb"]);
    assert!(ignored.status.success(), "{}", text(&ignored.stderr));
    assert_eq!(text(&ignored.stdout), ".env\n.spool/state.duckdb\n");
    assert_eq!(
        files_of(&project),
        [
            ".agents/skills/spool-api/SKILL.md",
            ".agents/skills/spool-data/SKILL.md",
            ".agents/skills/spool-extractor/SKILL.md",
            ".agents/skills/spool-project/SKILL.md",
            ".agents/skills/spool-rules/SKILL.md",
            ".claude/skills/spool-api/SKILL.md",
            ".claude/skills/spool-data/SKILL.md",
            ".claude/skills/spool-extractor/SKILL.md",
            ".claude/skills/spool-project/SKILL.md",
            ".claude/skills/spool-rules/SKILL.md",
            ".env.example",
            ".gitignore",
            "AGENTS.md",
            "CLAUDE.md",
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
    git_init(&here);
    let head = read(&here.join(".git/HEAD"));
    let config = read(&here.join(".git/config"));
    std::fs::write(here.join(".DS_Store"), "").unwrap();
    let created = init_in(&here, &[]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let created = text(&created.stdout);
    assert!(created.starts_with("created   .\n"), "{created}");
    assert!(!created.contains("  cd "), "{created}");
    assert!(here.join("entities/.gitkeep").is_file());
    assert_eq!(read(&here.join(".git/HEAD")), head);
    assert_eq!(read(&here.join(".git/config")), config);
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let created = init_in(dir.path(), &["empty"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(text(&created.stderr).contains("\n  cd empty\n"));

    // A directory that is not a plain word is quoted for the shell.
    let created = init_in(dir.path(), &["it's here"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let created = text(&created.stderr);
    assert!(created.contains("\n  cd 'it'\\''s here'\n"), "{created}");
}

#[test]
fn init_uses_the_project_directory_even_with_git_dir_set() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("elsewhere");
    let created = spool()
        .current_dir(dir.path())
        .args(["init", "parent/project", "--json"])
        .env("GIT_DIR", &elsewhere)
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", text(&created.stderr));
    let result: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(result["created"], "parent/project");
    assert!(dir.path().join("parent/project/.git/HEAD").is_file());
    assert!(!elsewhere.exists());
}

#[test]
fn init_requires_git_before_writing_project_files() {
    let dir = tempfile::tempdir().unwrap();
    let created = spool()
        .current_dir(dir.path())
        .args(["init", "project"])
        .env("PATH", dir.path())
        .output()
        .unwrap();
    assert_eq!(created.status.code(), Some(1));
    assert!(created.stdout.is_empty());
    let stderr = text(&created.stderr);
    assert!(
        stderr.contains("install Git and make sure git is on PATH"),
        "{stderr}"
    );
    assert!(!dir.path().join("project").exists());
}

#[test]
fn init_can_retry_after_git_leaves_a_partial_repository() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let failed = spool()
        .args(["init", path(&project)])
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_0", "invalid branch")
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(1));
    let stderr = text(&failed.stderr);
    assert!(stderr.contains("invalid branch"), "{stderr}");
    assert!(project.join(".git").is_dir());
    assert!(!project.join("entities").exists());

    succeed(&["init", path(&project)]);
    assert!(project.join(".git/HEAD").is_file());
    assert!(project.join("entities/.gitkeep").is_file());
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
        "error: this folder already holds files (Documents, Downloads, notes.txt and 2 more), \
         and `spool init` makes a project only in an empty one. Nothing was written. \
         `spool init <name>` makes the project in a new folder of that name\n"
    );
    assert_eq!(files_of(&home), before);
    assert!(!home.join("entities").exists());
    assert!(!home.join(".git").exists());

    // A folder that is named, with one file in it.
    let taken = dir.path().join("taken");
    std::fs::create_dir(&taken).unwrap();
    std::fs::write(taken.join("rows.csv"), "mine\n").unwrap();
    let refused = init_in(dir.path(), &["taken"]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        text(&refused.stderr),
        "error: taken already holds files (rows.csv). `spool init` makes a project only in a \
         new or empty folder: give it another name\n"
    );
    assert_eq!(files_of(&taken), ["rows.csv"]);
    assert!(!taken.join(".git").exists());

    // A project, named or not.
    assert!(init_in(dir.path(), &["project"]).status.success());
    let refused = init_in(dir.path(), &["project", "--example", "companies"]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        text(&refused.stderr),
        "error: project is a project already: it has an entities/ folder\n"
    );
    assert!(!dir.path().join("project/samples").exists());
    let refused = init_in(&dir.path().join("project"), &[]);
    assert_eq!(
        text(&refused.stderr),
        "error: this folder is a project already: it has an entities/ folder\n"
    );

    // A file.
    std::fs::write(dir.path().join("a-file"), "mine\n").unwrap();
    let refused = init_in(dir.path(), &["a-file"]);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        text(&refused.stderr),
        "error: a-file is a file, and a project is a folder\n"
    );
    assert_eq!(read(&dir.path().join("a-file")), "mine\n");
}

#[test]
fn update_skills_brings_the_files_for_agents_to_this_version_and_keeps_edits() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    succeed(&["init", path(&project)]);
    let skill = project.join(".agents/skills/spool-rules/SKILL.md");
    let written = read(&skill);
    assert!(written.starts_with("---\nname: spool-rules\n"), "{written}");
    assert!(
        written.contains("\nmetadata:\n  spool-sha256: \""),
        "{written}"
    );
    assert_eq!(
        std::fs::read_link(project.join(".claude/skills/spool-rules")).unwrap(),
        Path::new("../../.agents/skills/spool-rules")
    );
    assert_eq!(read(&project.join("CLAUDE.md")), "@AGENTS.md\n");
    let agents = read(&project.join("AGENTS.md"));
    assert!(agents.starts_with("<!-- spool:begin."), "{agents}");
    assert!(agents.ends_with("<!-- spool:end -->\n"), "{agents}");

    // Nothing to do just after `spool init`.
    let output = spool()
        .args(["update-skills", path(&project)])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    let said = text(&output.stdout);
    assert!(
        said.starts_with(&format!("project  {}\n\n", project.display())),
        "{said}"
    );
    assert_eq!(said.matches("  unchanged  ").count(), 12, "{said}");
    assert_eq!(
        text(&output.stderr),
        "\nThe files for agents are this version's already.\n"
    );

    // Notes of the user's own around Spool's part stay, and an edited
    // skill is kept until --force.
    std::fs::write(
        project.join("AGENTS.md"),
        format!("# Ours\n\nUse `make test`.\n\n{agents}"),
    )
    .unwrap();
    std::fs::write(&skill, format!("{written}\nCompare `country_code`.\n")).unwrap();
    let report: Value =
        serde_json::from_str(&succeed(&["update-skills", path(&project), "--json"])).unwrap();
    let status = |file: &str| {
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["path"] == file)
            .unwrap_or_else(|| panic!("{file}: {report}"))
            .clone()
    };
    assert_eq!(status("AGENTS.md")["status"], "unchanged");
    assert_eq!(
        status(".agents/skills/spool-rules/SKILL.md"),
        json!({
            "path": ".agents/skills/spool-rules/SKILL.md",
            "status": "kept",
            "note": "edited since Spool wrote it: --force replaces it",
        })
    );
    assert!(read(&skill).ends_with("\nCompare `country_code`.\n"));
    let table = succeed(&["update-skills", path(&project)]);
    assert!(
        table.contains(
            "  kept       .agents/skills/spool-rules/SKILL.md      edited since Spool wrote it: \
             --force replaces it\n"
        ),
        "{table}"
    );
    let forced = succeed(&["update-skills", path(&project), "--force"]);
    assert!(
        forced.contains("  updated    .agents/skills/spool-rules/SKILL.md\n"),
        "{forced}"
    );
    assert_eq!(read(&skill), written);
    assert!(read(&project.join("AGENTS.md")).starts_with("# Ours\n\nUse `make test`.\n\n"));

    // Removed by hand, they come back.
    std::fs::remove_dir_all(project.join(".claude")).unwrap();
    let restored = succeed(&["update-skills", path(&project)]);
    assert_eq!(
        restored.matches("  added      .claude/skills/").count(),
        5,
        "{restored}"
    );

    // A folder that is not a project is refused, and nothing is written.
    let output = spool()
        .args(["update-skills", path(dir.path())])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        format!(
            "error: {} is not a Spool project: it has no entities/ folder\nhint: `spool init` \
             makes a project, with these files in it\n",
            dir.path().display()
        )
    );
    assert!(!dir.path().join("AGENTS.md").exists());
}
