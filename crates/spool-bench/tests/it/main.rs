//! The integration tests of the benchmark runner: one binary, with a module
//! for each thing they are about. A new test file is a module here.
//!
//! The tests that start `uv` or `bun`, or download a dataset, are ignored
//! unless asked for:
//!
//! ```sh
//! cargo test -p spool-bench --test it -- --include-ignored
//! ```

mod news;
mod scenario;
mod splink;
