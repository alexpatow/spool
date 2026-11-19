//! `spool reset`: the development store started over, and nothing else.

mod support;

use serde_json::json;
use support::*;

fn reset(arguments: &[&str]) -> std::process::Output {
    let mut all = vec!["reset"];
    all.extend(arguments);
    run(&all)
}

#[test]
fn reset_removes_the_development_store_and_keeps_everything_else() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let at = path(&project);
    let spool_dir = project.join(".spool");

    let server = Server::dev(&project);
    let loaded = server.post(
        "/v1/load",
        json!({
            "source": "crm",
            "entity_type": "company",
            "rows": json_rows(rows(&[&["id", "name"], &["c-1", "Acorn Tools"]])),
        }),
    );
    assert_eq!(loaded.ok()["run"]["status"], "succeeded");
    // What an extractor keeps under .spool/ is its own.
    std::fs::create_dir_all(spool_dir.join("cache")).unwrap();
    std::fs::write(spool_dir.join("cache/answers.json"), "{}").unwrap();
    succeed(&["build", at]);

    // Not while `spool dev` has the store open, even when told to go ahead.
    let refused = reset(&[at, "--yes"]);
    assert_eq!(refused.status.code(), Some(1));
    let said = text(&refused.stderr);
    assert!(
        said.starts_with("spool: the store .spool/dev.duckdb is open in another process (PID "),
        "{said}"
    );
    assert!(
        said.contains("Stop it with Ctrl-C, then run `spool reset` again. Nothing was removed."),
        "{said}"
    );
    assert!(spool_dir.join("dev.duckdb").is_file());
    server.stop();

    // Where nobody can be asked, it says what it would do and does nothing.
    let unasked = reset(&[at]);
    assert_eq!(unasked.status.code(), Some(1));
    let plan = text(&unasked.stdout);
    assert!(
        plan.starts_with(&format!(
            "This starts the development store of {at} over.\n\nremoves\n  .spool/dev.duckdb"
        )),
        "{plan}"
    );
    for kept in [
        ".spool/cache/ not the store's",
        ".spool/dev.auth.sqlite the accounts and tokens of `spool dev`",
        ".spool/manifest.json the manifest `spool build` wrote",
        "the project's files everything outside .spool/",
    ] {
        assert!(
            plan.lines()
                .any(|line| line.split_whitespace().eq(kept.split_whitespace())),
            "no `{kept}` in:\n{plan}"
        );
    }
    assert_eq!(
        text(&unasked.stderr),
        "spool: nothing was removed: nobody can be asked here, so add `--yes` to remove it\n"
    );
    assert!(spool_dir.join("dev.duckdb").is_file());

    let done = reset(&[at, "--yes"]);
    assert!(done.status.success(), "{}", text(&done.stderr));
    let done = text(&done.stdout);
    assert!(done.contains("\nremoved  .spool/dev.duckdb\n"), "{done}");
    assert!(
        done.ends_with(
            "\nThe next `spool dev` starts with an empty store. `spool load` and `spool extract` fill it again.\n"
        ),
        "{done}"
    );
    let mut left: Vec<String> = std::fs::read_dir(&spool_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert!(
        left.iter().all(|name| !name.starts_with("dev.duckdb")),
        "{left:?}"
    );
    assert!(left.contains(&"dev.auth.sqlite".to_owned()), "{left:?}");
    assert_eq!(
        std::fs::read_to_string(spool_dir.join("cache/answers.json")).unwrap(),
        "{}"
    );
    assert!(spool_dir.join("manifest.json").is_file());
    assert!(project.join("entities/company/entity.yaml").is_file());

    let again = reset(&[at, "--yes"]);
    assert!(again.status.success());
    assert_eq!(
        text(&again.stdout),
        "there is no development store at .spool/dev.duckdb: nothing to remove\n"
    );

    // The next `spool dev` starts on an empty store, with the accounts kept.
    let server = Server::dev(&project);
    let info = server.read("/v1/info");
    assert_eq!(info["store"]["entity_types"][0]["records"], 0, "{info}");
    server.stop();
}

#[test]
fn reset_starts_over_only_a_store_under_the_projects_spool_directory() {
    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let at = path(&project);

    let nothing = reset(&[at]);
    assert!(nothing.status.success());
    assert_eq!(
        text(&nothing.stdout),
        format!(
            "{} does not exist, so there is no development store to remove\n",
            project.join(".spool").display()
        )
    );

    std::fs::create_dir_all(project.join(".spool")).unwrap();
    let served = dir.path().join("spool.duckdb");
    std::fs::write(&served, "a store a server runs on").unwrap();
    let manifest = project.join(".spool/manifest.json");
    std::fs::write(&manifest, "{}").unwrap();
    for store in [&served, &manifest] {
        let refused = reset(&[at, "--store", path(store), "--yes"]);
        assert_eq!(refused.status.code(), Some(2));
        let said = text(&refused.stderr);
        assert!(
            said.starts_with("error: `spool reset` removes only a development store: a .duckdb file directly under "),
            "{said}"
        );
        assert!(said.contains("which no command removes."), "{said}");
        assert!(store.is_file());
    }

    // A store `spool dev` was given with `--store` is one, if it is there.
    let other = project.join(".spool/other.duckdb");
    std::fs::write(&other, "not a database DuckDB can read").unwrap();
    let removed = reset(&[at, "--store", path(&other), "--yes"]);
    assert!(removed.status.success(), "{}", text(&removed.stderr));
    assert!(text(&removed.stdout).contains("\nremoved  .spool/other.duckdb\n"));
    assert!(!other.exists());
}

#[cfg(unix)]
#[test]
fn reset_removes_only_what_duckdb_makes_and_follows_no_link() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let project = init_companies(dir.path());
    let at = path(&project);
    let spool_dir = project.join(".spool");
    std::fs::create_dir_all(&spool_dir).unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep.txt"), "not the project's").unwrap();
    let kept = || {
        assert_eq!(
            std::fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "not the project's"
        );
    };
    let refused = |arguments: &[&str], said: &str| {
        let output = reset(arguments);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        let complaint = text(&output.stderr);
        assert!(complaint.contains(said), "{complaint}");
        assert!(complaint.contains("othing was removed"), "{complaint}");
    };

    // A directory named like a store is not one, and is not emptied.
    let backups = spool_dir.join("backups.duckdb");
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::write(backups.join("monday.duckdb"), "a backup").unwrap();
    refused(
        &[at, "--store", path(&backups), "--yes"],
        "is a directory, where `spool reset` expects a file",
    );
    assert!(backups.join("monday.duckdb").is_file());

    // A store that is a link is refused, and what it points to is kept.
    let linked = spool_dir.join("linked.duckdb");
    symlink(outside.join("keep.txt"), &linked).unwrap();
    refused(&[at, "--store", path(&linked), "--yes"], "is a link");
    kept();

    // So is a log or a spill directory that is a link, or of the wrong kind,
    // and the store beside it stays.
    let store = spool_dir.join("dev.duckdb");
    std::fs::write(&store, "a store").unwrap();
    let wal = spool_dir.join("dev.duckdb.wal");
    symlink(outside.join("keep.txt"), &wal).unwrap();
    refused(&[at, "--yes"], "dev.duckdb.wal is a link");
    std::fs::remove_file(&wal).unwrap();
    let tmp = spool_dir.join("dev.duckdb.tmp");
    symlink(&outside, &tmp).unwrap();
    refused(&[at, "--yes"], "dev.duckdb.tmp is a link");
    std::fs::remove_file(&tmp).unwrap();
    std::fs::write(&tmp, "a file").unwrap();
    refused(
        &[at, "--yes"],
        "dev.duckdb.tmp is a file, where `spool reset` expects a directory",
    );
    std::fs::remove_file(&tmp).unwrap();
    assert!(store.is_file());
    kept();

    // A spill directory is removed whole, and a link in it is removed
    // without what it points to. The store goes last.
    std::fs::create_dir_all(&tmp).unwrap();
    symlink(&outside, tmp.join("elsewhere")).unwrap();
    std::fs::write(&wal, "a log").unwrap();
    let done = reset(&[at, "--yes"]);
    assert!(done.status.success(), "{}", text(&done.stderr));
    let done = text(&done.stdout);
    assert!(
        done.contains(
            "\nremoves\n  .spool/dev.duckdb.wal\n  .spool/dev.duckdb.tmp\n  .spool/dev.duckdb\n"
        ),
        "{done}"
    );
    assert!(
        done.contains(
            "removed  .spool/dev.duckdb.wal\nremoved  .spool/dev.duckdb.tmp\nremoved  .spool/dev.duckdb\n"
        ),
        "{done}"
    );
    assert!(!store.exists() && !wal.exists() && !tmp.exists());
    kept();
    assert!(backups.join("monday.duckdb").is_file());

    // A .spool that is a link takes reset out of the project: refused.
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("dev.duckdb"), "someone's store").unwrap();
    let other = dir.path().join("other");
    succeed(&["init", path(&other)]);
    symlink(&elsewhere, other.join(".spool")).unwrap();
    let output = reset(&[path(&other), "--yes"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains(
            "is a link to another place, and `spool reset` removes nothing outside the project"
        ),
        "{}",
        text(&output.stderr)
    );
    assert!(elsewhere.join("dev.duckdb").is_file());
}
