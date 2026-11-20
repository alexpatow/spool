//! The handlers of `/v1`. Each file is named after the module of the
//! engine it mostly calls.
//!
//! An operation that an MCP tool shares with a route is a method of `App`:
//! it takes typed arguments, and the caller where it records who acted, and
//! answers with a value. The function the route is given reads the path,
//! the query string and the body, calls the method, and makes the response
//! of what it answers. The tool calls the same method. An operation only a
//! route has is that one function.

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
