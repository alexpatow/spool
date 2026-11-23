//! Datasets a scenario downloads: once, checked, and unpacked.

use super::*;

#[test]
fn a_dataset_is_downloaded_once_and_checked_against_its_checksum() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    write(
        root,
        "bench/scenario.toml",
        &scenario("dataset = \"papers\""),
    );
    write(root, "upstream/papers.csv", PAPERS);
    let datasets = |sha256: &str| {
        format!(
            "[[dataset]]\nname = \"papers\"\nurl = \"file://{}\"\nsha256 = \"{sha256}\"\nlicence = \"invented for this test\"\n",
            root.join("upstream/papers.csv").display()
        )
    };

    // No datasets.toml above the scenario.
    let message = scenario_error(run(root));
    assert!(message.contains("no datasets.toml"), "{message}");

    // A download that is not the file the checksum names is refused and not kept.
    write(root, "datasets.toml", &datasets(&sha256("something else")));
    let message = scenario_error(run(root));
    assert!(message.contains("SHA-256"), "{message}");
    assert_eq!(fs::read_dir(root.join(".cache")).unwrap().count(), 0);

    write(root, "datasets.toml", &datasets(&sha256(PAPERS)));
    assert_eq!(run(root).unwrap().steps[0].ingest[0].inserted, 4);
    assert_eq!(
        fs::read_to_string(root.join(".cache/papers.csv")).unwrap(),
        PAPERS
    );

    // The kept copy is used from then on.
    fs::remove_file(root.join("upstream/papers.csv")).unwrap();
    assert_eq!(run(root).unwrap().steps[0].ingest[0].inserted, 4);

    // A kept copy that no longer matches is fetched again, which now fails.
    write(root, ".cache/papers.csv", "id,title,work\n");
    let message = scenario_error(run(root));
    assert!(message.contains("downloading"), "{message}");

    write(
        root,
        "bench/scenario.toml",
        &scenario("dataset = \"posters\""),
    );
    let message = scenario_error(run(root));
    assert!(message.contains("no dataset named `posters`"), "{message}");
}

#[test]
fn a_dataset_that_several_runs_ask_for_at_once_reaches_each_of_them_whole() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    // A file that takes a moment to copy, of which each run reads four rows.
    let mut papers = String::from(PAPERS);
    for id in 5..200_000 {
        papers += &format!("{id},A paper that nobody reads but everybody downloads {id},\n");
    }
    write(root, "upstream/papers.csv", &papers);
    write(
        root,
        "datasets.toml",
        &format!(
            "[[dataset]]\nname = \"papers\"\nurl = \"file://{}\"\nsha256 = \"{}\"\nlicence = \"invented for this test\"\n",
            root.join("upstream/papers.csv").display(),
            sha256(&papers)
        ),
    );
    write(
        root,
        "bench/scenario.toml",
        &scenario("dataset = \"papers\"\nwhere = \"id::BIGINT < 5\""),
    );

    // Tests of one binary run at once, and so do two people's benchmarks.
    let inserted: Vec<_> = std::thread::scope(|scope| {
        let runs: Vec<_> = (0..8).map(|_| scope.spawn(|| run(root))).collect();
        runs.into_iter()
            .map(|run| {
                run.join()
                    .unwrap()
                    .map(|report| report.steps[0].ingest[0].inserted)
            })
            .collect()
    });
    for inserted in inserted {
        assert_eq!(inserted.unwrap(), 4);
    }
    assert_eq!(
        fs::read_to_string(root.join(".cache/papers.csv")).unwrap(),
        papers
    );
    let left: Vec<_> = fs::read_dir(root.join(".cache"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(left, ["papers.csv"]);
}

#[test]
fn a_dataset_that_is_an_archive_is_unpacked_once_and_an_ingest_names_a_file_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    write(root, "upstream/shelves/papers.csv", PAPERS);
    write(root, "upstream/shelves/more.csv", MORE);
    let archive = root.join("upstream/shelves.tar.gz");
    let packed = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(root.join("upstream"))
        .arg("shelves")
        .status()
        .unwrap();
    assert!(packed.success());
    let checksum = hex::encode(Sha256::digest(fs::read(&archive).unwrap()));
    let datasets = |archive_line: &str| {
        format!(
            "[[dataset]]\nname = \"shelves\"\nurl = \"file://{}\"\nsha256 = \"{checksum}\"\n\
             licence = \"invented for this test\"\n{archive_line}",
            archive.display()
        )
    };
    let ingest = |member: &str| scenario(&format!("dataset = \"shelves\"\n{member}"));

    write(root, "datasets.toml", &datasets("archive = \"tar\"\n"));
    write(
        root,
        "bench/scenario.toml",
        &ingest("member = \"shelves/papers.csv\""),
    );
    assert_eq!(run(root).unwrap().steps[0].ingest[0].inserted, 4);
    assert!(root.join(".cache/shelves/shelves/more.csv").is_file());
    // What is kept is the unpacked copy, not the archive beside it.
    assert!(!root.join(".cache/shelves.tar").exists());

    // The unpacked copy is used from then on, and another file of it is
    // there without a second download.
    fs::remove_file(&archive).unwrap();
    write(
        root,
        "bench/scenario.toml",
        &ingest("member = \"shelves/more.csv\""),
    );
    assert_eq!(run(root).unwrap().steps[0].ingest[0].inserted, 3);

    let cases = [
        (
            ingest("member = \"shelves/posters.csv\""),
            "the archive has no file `shelves/posters.csv`",
        ),
        (
            ingest("member = \"../papers.csv\""),
            "`../papers.csv` does not name a file within the archive",
        ),
        (
            ingest(""),
            "dataset `shelves` is an archive: the ingest names the file to read with `member`",
        ),
    ];
    for (scenario, expected) in cases {
        write(root, "bench/scenario.toml", &scenario);
        let message = scenario_error(run(root));
        assert!(
            message.contains(expected),
            "expected `{expected}` in: {message}"
        );
    }

    // A dataset that is one file has no members, and only `tar` is an archive.
    write(root, "datasets.toml", &datasets(""));
    write(
        root,
        "bench/scenario.toml",
        &ingest("member = \"shelves/papers.csv\""),
    );
    let message = scenario_error(run(root));
    assert!(message.contains("is one file"), "{message}");
    write(root, "datasets.toml", &datasets("archive = \"zip\"\n"));
    let message = scenario_error(run(root));
    assert!(message.contains("unknown variant `zip`"), "{message}");

    // `member` belongs to a dataset.
    write(
        root,
        "bench/scenario.toml",
        &scenario("file = \"papers.csv\"\nmember = \"shelves/papers.csv\""),
    );
    let message = scenario_error(Scenario::load(root.join("bench")));
    assert!(
        message.contains("`member` names a file within a `dataset`"),
        "{message}"
    );
}
