//! The parts of the `spool` command that are worth using from a test: the
//! HTTP server over an [`Engine`](spool::Engine), the types it speaks, and
//! the JSON Schema that describes them.

pub mod api;
pub mod server;
