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
mod documents;
mod drafts;
mod engine;
mod error;
mod features;
mod ingest;
mod lookup;
mod metrics;
mod project;
mod review;
mod rows;
mod rules;
mod runs;
mod schema;
mod store;
#[cfg(test)]
mod store_property;
mod time;
mod usercode;

/// The Arrow version the engine is built against.
pub use duckdb::arrow;
pub use spool_core as core;

pub use bench::{BenchReport, CorrectionSummary, IngestSummary, Scenario, StepReport};
pub use documents::{
    DocumentExtraction, DocumentFilter, DocumentRecord, ExtractOptions, ExtractReport,
    ExtractorRef, NewDocument, locator,
};
pub use drafts::{DraftFilter, DraftRecord, DraftStatus, DraftSummary};
pub use engine::{
    ApplyReport, Authored, ConstraintFilter, Disposition, Engine, JudgementCounts, MintedIds,
    Options, ResolveOptions, ResolveReport, Scope, SplitReport, StrategyRef, SuggestedMerge,
    SuggestedSplit, Suggestions, TypeResolution, watch_runs,
};
pub use error::{Error, Result};
pub use ingest::{IngestOptions, IngestReport};
pub use lookup::{
    HistoryEntry, IdentityFilter, IdentitySummary, IdentityView, Member, ObservationFilter,
    ObservationRecord, StoreInfo, TypeInfo,
};
pub use metrics::{
    BCubed, Churn, IdStability, Metrics, OrderSensitivity, OverridePersistence, PairMetrics,
    SplitRecovery, UpgradeStability,
};
pub use project::{
    Declaration, EntityTypeManifest, EntityTypeSource, Extractor, FeatureColumn, Manifest, Project,
    Rule, RuleSet, Runtime, SqlFile, Strategy, StrategySource,
};
pub use review::{Dismissal, ReviewFilter, ReviewKind, ReviewRecord, ReviewStatus};
pub use rows::Rows;
pub use runs::{RunFilter, RunKind, RunRecord, RunStatus};
pub use usercode::{Code, UserCode};
