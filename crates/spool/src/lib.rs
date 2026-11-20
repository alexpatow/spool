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

mod attributes;
mod batch;
mod documents;
mod drafts;
mod engine;
mod error;
mod explain;
mod features;
mod following;
mod links;
mod load;
mod lookup;
mod metrics;
mod order;
mod possible_split;
mod project;
mod recompute;
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

pub use attributes::{
    AttributeFilter, AttributeGroup, AttributeSummary, COMMON_VALUES, DerivedFeature, ValueCount,
};
pub use documents::{
    AddDocumentReport, DocumentEntry, DocumentExtraction, DocumentFilter, DocumentReading,
    DocumentRecord, DocumentSort, ExtractOptions, ExtractReport, ExtractorRef, FailedDocument,
    NewDocument, locator,
};
pub use drafts::{DraftEntry, DraftFilter, DraftStatus, DraftSummary};
pub use engine::{
    Acting, AddRecordReport, ApplyReport, Authored, ConstraintFilter, Disposition, Engine,
    JudgementCounts, MintedIds, Options, ResolveOptions, ResolveReport, Scope, SplitReport,
    StrategyRef, SuggestedMerge, SuggestedSplit, Suggestions, TypeResolution, watch_runs,
};
pub use error::{Error, Result};
pub use explain::{
    Comparison, Explained, Explanation, IdentityExplanation, Link, LinkKind, LinkValue, NameCount,
    Named, Outcome, Placement, RecordedJudgement, RuleReading, RuleResult, Side, Split, how_linked,
};
pub use load::LoadReport;
pub use lookup::{
    ChangesetSummary, DeclaredSource, FoundBy, HistoryEntry, IdentityFilter, IdentityMatch,
    IdentitySnapshot, IdentitySort, IdentitySummary, IdentityView, LogSort, LoggedChangeset,
    Member, RecordEntry, RecordFilter, RecordSort, SourceCount, SourceInfo, StoreInfo, TypeInfo,
};
pub use metrics::{BCubed, Churn, IdStability, Metrics, PairMetrics};
pub use order::{ActorNames, Direction, Sort};
pub use project::{
    Declaration, EntityTypeManifest, EntityTypeSource, Extractor, FeatureColumn, Manifest, Mapped,
    Project, Rule, RuleSet, Runtime, Source, SqlFile, Strategy, StrategySource, Transform,
    YamlFile,
};
pub use recompute::{
    After, AppliedType, ApprovedPlacement, Before, Change, ChangeDetail, ChangeKind,
    ConstraintNote, Contradiction, DEFAULT_CHANGES, DecisionSummary, Fate, PersonDecision,
    RecomputeApplied, RecomputeReport, RecomputeRequest, RecomputeSummary, RecordMove,
    TypeRecompute,
};
pub use retract::{NOT_RETRACTED, RetractReport};
pub use review::{
    CASE_MEMBERS, Candidate, CaseConstraint, Dismissal, PairJudgement, ReviewCase, ReviewEntry,
    ReviewFilter, ReviewKind, ReviewStatus,
};
pub use rows::Rows;
pub use runs::{RunEntry, RunFilter, RunKind, RunSort, RunStatus};
pub use store::{StoreHold, hold_store};
pub use usercode::{Code, Running, UserCode};
