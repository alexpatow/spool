//! The Spool engine.

mod batch;
mod error;
mod schema;
mod store;
#[cfg(test)]
mod store_property;

pub use error::{Error, Result};
