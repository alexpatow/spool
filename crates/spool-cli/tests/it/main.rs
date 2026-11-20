//! The integration tests of the `spool` command and its server: one binary,
//! with a module for each thing they are about. A new test file is a module
//! here.
//!
//! `stress.rs` and `studio.rs`, beside this directory, are binaries of their
//! own: each is one long test that runs a TypeScript package's tests, and CI
//! runs it in parts.
//!
//! The tests that start `uv` or `bun` are ignored unless asked for:
//!
//! ```sh
//! cargo test -p spool-cli --test it -- --include-ignored
//! ```

mod accounts;
mod cli;
mod client;
mod commands;
mod extract;
mod first_extraction;
mod google;
mod lifecycle;
mod load;
mod matching;
mod mcp;
mod milestone_1;
mod milestone_3;
mod oauth;
mod output;
mod recompute;
mod reset;
mod retract;
mod server;
mod show;
mod support;
mod update;
mod usercode;
