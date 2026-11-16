//! The Spool engine.
//!
//! Spool turns records about the same real-world thing into one durable
//! identity. A project declares its entity types as files: a features query
//! and SQL rules per type. The engine ingests observations, matches new ones
//! against what it holds, and changes identity state only by committing
//! changesets.
//!
//! ```no_run
//! use spool::{Engine, IngestOptions, Manifest, Options, ResolveOptions};
//! # fn rows() -> spool::arrow::record_batch::RecordBatchIterator<
//! #     std::vec::IntoIter<Result<spool::arrow::record_batch::RecordBatch, spool::arrow::error::ArrowError>>,
//! # > { unimplemented!() }
//!
//! # fn main() -> spool::Result<()> {
//! let engine = Engine::open("spool.duckdb", Options::default())?;
//! engine.apply(&Manifest::load("my-project")?)?;
//! engine.ingest("crm", "company", rows(), &IngestOptions::keyed_by("id"))?;
//! let report = engine.resolve(&ResolveOptions::default())?;
//! # Ok(())
//! # }
//! ```
//!
//! Identity rules live in [`spool_core`], which is re-exported as
//! [`core`].

mod batch;
mod bench;
mod engine;
mod error;
mod features;
mod ingest;
mod metrics;
mod project;
mod rows;
mod rules;
mod schema;
mod store;
#[cfg(test)]
mod store_property;

/// The Arrow version the engine is built against.
pub use duckdb::arrow;
pub use spool_core as core;

pub use bench::{BenchReport, IngestSummary, Scenario, StepReport};
pub use engine::{
    ApplyReport, Engine, JudgementCounts, Options, ResolveOptions, ResolveReport, Scope,
    StrategyRef, TypeResolution,
};
pub use error::{Error, Result};
pub use ingest::{IngestOptions, IngestReport};
pub use metrics::{BCubed, IdStability, Metrics, PairMetrics};
pub use project::{
    Declaration, EntityTypeManifest, EntityTypeSource, FeatureColumn, Manifest, Project, Rule,
    RuleSet, SqlFile,
};
pub use rows::Rows;
