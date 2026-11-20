//! The parts of the `spool` command that are worth using from a test: the
//! HTTP server over an [`Engine`](spool::Engine), the types it speaks, the
//! JSON Schema that describes them, the studio the server answers a
//! browser with, and the reader of CSV that the server and `spool load` share.

pub mod accounts;
pub mod api;
pub mod delimited;
pub mod google;
pub mod server;
pub mod studio;
pub mod text;
