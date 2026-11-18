//! The parts of the `spool` command that are worth using from a test: the
//! HTTP server over an [`Engine`](spool::Engine), the types it speaks, the
//! JSON Schema that describes them, and the studio the server answers a
//! browser with.

pub mod accounts;
pub mod api;
pub mod server;
pub mod studio;
