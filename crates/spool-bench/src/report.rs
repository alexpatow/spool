//! What running a scenario reports: every step, and what each fed the
//! engine.

use serde::{Deserialize, Serialize};
use spool::core::Seq;
use spool::{Churn, IdStability, Metrics, ResolveReport};

use crate::metrics::{
    EndToEndMetrics, ExtractionMetrics, OrderSensitivity, OverridePersistence, Rebuild,
    SplitRecovery, UpgradeStability,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BenchReport {
    pub scenario: String,
    pub engine_version: String,
    pub manifest_hash: String,
    pub steps: Vec<StepReport>,
    /// `None` when fewer than two steps ingest anything, so there is no
    /// other order to try.
    pub order_sensitivity: Option<OrderSensitivity>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StepReport {
    pub name: String,
    /// The strategy the step resolved with.
    pub strategy: String,
    pub ingest: Vec<IngestSummary>,
    /// The documents the step stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<DocumentsSummary>,
    /// The extractions the step ran, after its documents were stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extract: Vec<ExtractSummary>,
    /// The rows the step took back, after its files were ingested.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retract: Vec<RetractSummary>,
    /// The corrections the step applied, in order, before it resolved.
    pub corrections: Vec<CorrectionSummary>,
    pub resolve: ResolveReport,
    /// The log position after the step.
    pub head: Seq,
    /// Committed state against the truth for everything ingested so far
    /// that its source has not taken back.
    pub metrics: Metrics,
    /// The mentions extracted so far against the labelled ones. Only a
    /// scenario with labelled mentions has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction: Option<ExtractionMetrics>,
    /// Extraction and resolution together: the identities of the mentions
    /// extracted so far against every labelled mention. Only a scenario
    /// with labelled mentions has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_to_end: Option<EndToEndMetrics>,
    /// Of the records that had an identity before the step, how many
    /// kept it.
    pub id_stability: IdStability,
    pub churn: Churn,
    /// Of the splits the scenario has made so far, how many still hold.
    pub split_recovery: SplitRecovery,
    /// Of the constraints the scenario has recorded so far, how many are
    /// still honoured.
    pub override_persistence: OverridePersistence,
    /// What matching everything again did to existing identities. Only a
    /// step with `rematch = true` has it.
    pub upgrade_stability: Option<UpgradeStability>,
    /// What the step left against the rows the sources still hold resolved
    /// in a store of their own. Only a step with `rebuild = true` has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<Rebuild>,
    /// The open `possible_split` review items against the truth. Only a
    /// store in which a retraction has raised one has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub possible_splits: Option<PossibleSplits>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestSummary {
    pub source: String,
    pub entity_type: String,
    pub received: u64,
    pub inserted: u64,
    pub duplicates: u64,
    /// Rows that replaced an earlier version of the same key.
    pub superseded: u64,
    /// Records that came back: their source had taken them back, and sent
    /// a row under their key again.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub restored: u64,
    /// Those of them that went back to the identity they left.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub returned: u64,
    /// Identities they went back to where nothing holds them with the
    /// records that are there, each put to a reviewer as a `possible_split`
    /// item.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub possible_splits: u64,
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// Rows one source took back in a step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetractSummary {
    pub source: String,
    pub entity_type: String,
    /// Keys that were named.
    pub received: u64,
    /// Those the source had no row under.
    pub unknown: u64,
    /// Records taken back: every version of each row.
    pub retracted: u64,
    /// Those that had an identity, and left it.
    pub detached: u64,
    /// Identities they left whose remaining records nothing holds together
    /// any more, each put to a reviewer as a `possible_split` item.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub possible_splits: u64,
}

/// The `possible_split` review items that are open after a step, against
/// the truth. An item asks about a pair of records when they are in
/// different groups of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PossibleSplits {
    pub open: u64,
    /// The records the open items name.
    pub records: u64,
    /// The pairs they ask about.
    pub pairs: u64,
    /// Of those pairs, the ones the truth says are two things.
    pub wrong_pairs: u64,
    /// Items that ask about at least one such pair: their identity holds
    /// more than one thing.
    pub wrong: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentsSummary {
    pub source: String,
    pub received: u64,
    pub inserted: u64,
    pub duplicates: u64,
    /// Documents that replaced an earlier version of the same key.
    pub superseded: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractSummary {
    pub extractor: String,
    /// Documents the extractor read.
    pub documents: u64,
    pub mentions: u64,
    /// Records created.
    pub inserted: u64,
    pub duplicates: u64,
    pub superseded: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrectionSummary {
    pub name: Option<String>,
    /// `merge`, `assign`, `split`, `must_link`, `cannot_link` or `undo`.
    pub kind: String,
    /// The log position of the changeset it committed.
    pub seq: Seq,
}
