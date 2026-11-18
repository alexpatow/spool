//! The Spool engine.
//!
//! Spool turns records about the same real-world thing into one durable
//! identity. A project declares what its sources send and its entity types
//! as files: per type, its features, how each source's columns become them,
//! and SQL rules. The engine loads records, matches new ones
//! against what it holds, and changes identity state only by committing
//! changesets.
//!
//! ```no_run
//! use spool::{Engine, Manifest, Options, ResolveOptions};
//! # fn rows() -> spool::arrow::record_batch::RecordBatchIterator<
//! #     std::vec::IntoIter<Result<spool::arrow::record_batch::RecordBatch, spool::arrow::error::ArrowError>>,
//! # > { unimplemented!() }
//!
//! # fn main() -> spool::Result<()> {
//! let engine = Engine::open("spool.duckdb", Options::default())?;
//! engine.apply(&Manifest::load("my-project")?)?;
//! engine.load("crm", "company", rows())?;
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
mod explain;
mod features;
mod load;
mod lookup;
mod metrics;
mod possible_split;
mod project;
mod retract;
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
mod versions;

/// The Arrow version the engine is built against.
pub use duckdb::arrow;
pub use spool_core as core;

pub use bench::{
    BenchReport, CorrectionSummary, DocumentsSummary, ExtractSummary, IngestSummary,
    PossibleSplits, RetractSummary, Scenario, StepReport,
};
pub use documents::{
    DocumentEntry, DocumentExtraction, DocumentFilter, ExtractOptions, ExtractReport, ExtractorRef,
    NewDocument, locator,
};
pub use drafts::{DraftEntry, DraftFilter, DraftStatus, DraftSummary};
pub use engine::{
    ApplyReport, Authored, ConstraintFilter, Disposition, Engine, JudgementCounts, MintedIds,
    Options, ResolveOptions, ResolveReport, Scope, SplitReport, StrategyRef, SuggestedMerge,
    SuggestedSplit, Suggestions, TypeResolution, watch_runs,
};
pub use error::{Error, Result};
pub use explain::{
    Comparison, Explained, Explanation, Link, LinkKind, Outcome, Placement, RecordedJudgement,
    RuleReading, RuleResult,
};
pub use load::LoadReport;
pub use lookup::{
    DeclaredSource, HistoryEntry, IdentityFilter, IdentitySummary, IdentityView, Member,
    RecordEntry, RecordFilter, SourceCount, SourceInfo, StoreInfo, TypeInfo,
};
pub use metrics::{
    BCubed, Churn, ExtractionMetrics, IdStability, Metrics, OrderSensitivity, OverridePersistence,
    PairMetrics, Rebuild, SplitRecovery, UpgradeStability,
};
pub use project::{
    Declaration, EntityTypeManifest, EntityTypeSource, Extractor, FeatureColumn, Manifest, Project,
    Rule, RuleSet, Runtime, Source, SqlFile, Strategy, StrategySource, Transform, YamlFile,
};
pub use retract::RetractReport;
pub use review::{
    CASE_MEMBERS, Candidate, CaseConstraint, Dismissal, PairJudgement, ReviewCase, ReviewEntry,
    ReviewFilter, ReviewKind, ReviewStatus,
};
pub use rows::Rows;
pub use runs::{RunEntry, RunFilter, RunKind, RunStatus};
pub use usercode::{Code, UserCode};
