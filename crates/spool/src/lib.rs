//! The Spool engine.

mod batch;
mod error;
mod features;
mod project;
mod schema;
mod store;
#[cfg(test)]
mod store_property;

pub use error::{Error, Result};
pub use project::{
    Declaration, EntityTypeManifest, EntityTypeSource, FeatureColumn, Manifest, Project, Rule,
    RuleSet, SqlFile,
};
