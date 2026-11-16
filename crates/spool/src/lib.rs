//! The Spool engine.

mod batch;
mod error;
mod features;
mod ingest;
mod metrics;
mod project;
mod rules;
mod schema;
mod store;
#[cfg(test)]
mod store_property;

pub use error::{Error, Result};
pub use ingest::{IngestOptions, IngestReport};
pub use metrics::{BCubed, IdStability, Metrics, PairMetrics};
pub use project::{
    Declaration, EntityTypeManifest, EntityTypeSource, FeatureColumn, Manifest, Project, Rule,
    RuleSet, SqlFile,
};
