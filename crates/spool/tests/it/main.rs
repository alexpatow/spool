//! The engine's integration tests: one binary, with a module for each thing
//! they are about. A new test file is a module here.
//!
//! The tests that start `uv` or `bun` are ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool --test it -- --include-ignored
//! ```

mod drafts;
mod engine;
mod explain;
mod extract;
mod interrupted;
mod lifecycle;
mod lookup;
mod metrics;
mod possible_split;
mod project;
mod readable;
mod recompute;
mod rematch;
mod retract;
mod review;
mod review_answers;
mod runs;
mod sorting;
mod usercode;
