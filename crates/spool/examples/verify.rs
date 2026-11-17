//! Checks a store that no server has open: that its projection tables equal
//! a replay of the log, and that they hold what the validator guarantees.
//!
//! ```text
//! cargo run --release -p spool --example verify -- <store>
//! ```
//!
//! It is how the stress tests in `packages/stress` look at a store after the
//! server that wrote it was stopped or killed. Opening a store marks a run
//! that was cut short as interrupted, as a server that starts on it would.

use std::process::ExitCode;
use std::time::Instant;

use spool::{Engine, Options};

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: verify <store>");
        return ExitCode::from(2);
    };
    if !std::path::Path::new(&path).is_file() {
        eprintln!("there is no store at {path}");
        return ExitCode::from(2);
    }
    let started = Instant::now();
    let checked = Engine::open(&path, Options::default()).and_then(|engine| {
        let head = engine.head()?;
        engine.verify()?;
        Ok(head)
    });
    match checked {
        Ok(head) => {
            println!(
                "verified {path}: the log ends at {head}, in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{path} does not verify: {error}");
            ExitCode::FAILURE
        }
    }
}
