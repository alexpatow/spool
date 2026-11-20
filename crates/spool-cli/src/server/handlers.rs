//! The handlers of `/v1`: one typed function for each operation, which the
//! HTTP route and the MCP tool both call. Each file is named after the
//! module of the engine it mostly calls.

pub(super) mod changesets;
pub(super) mod documents;
pub(super) mod drafts;
pub(super) mod explain;
pub(super) mod identities;
pub(super) mod info;
pub(super) mod load;
pub(super) mod recompute;
pub(super) mod records;
pub(super) mod review;
pub(super) mod runs;
