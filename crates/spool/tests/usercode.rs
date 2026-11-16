//! User-code strategies: files in the project that the engine runs as child
//! processes.
//!
//! Every test here starts `uv`, which fetches a Python and `pyarrow` the
//! first time, so they are ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool --test usercode -- --include-ignored
//! ```

mod common;

use std::path::Path;
use std::time::Duration;

use common::benchmarks;
use spool::{Engine, Options, ResolveOptions, Scenario, UserCode};

/// The content hash of every changeset in the log.
fn hashes(engine: &Engine) -> Vec<String> {
    engine
        .log()
        .unwrap()
        .iter()
        .map(|entry| entry.content_hash.to_string())
        .collect()
}

fn user_code(project: &Path, work: &Path) -> UserCode {
    UserCode {
        work_dir: work.to_owned(),
        timeout: Duration::from_secs(300),
        ..UserCode::new(project)
    }
}

#[test]
#[ignore = "runs uv"]
fn a_python_strategy_with_the_rules_commits_what_the_rules_commit() {
    let scenario = Scenario::load(benchmarks().join("identity/companies-small")).unwrap();
    let work = tempfile::tempdir().unwrap();

    let with_rules = Engine::open_in_memory(Options::seeded(7)).unwrap();
    let rules = scenario.run(&with_rules, |_| Ok(())).unwrap();

    let with_python = Engine::open_in_memory(Options::seeded(7)).unwrap();
    with_python.set_user_code(user_code(&scenario.project_dir(), work.path()));
    let python = scenario
        .with_strategy("in_python")
        .run(&with_python, |_| Ok(()))
        .unwrap();

    assert_eq!(hashes(&with_python), hashes(&with_rules));
    assert!(!hashes(&with_rules).is_empty());
    for (python, rules) in python.steps.iter().zip(&rules.steps) {
        assert_eq!(python.metrics, rules.metrics);
        assert_eq!(
            python.resolve.resolved[0].judgements,
            rules.resolve.resolved[0].judgements
        );
    }
    let _ = ResolveOptions::default();
}
