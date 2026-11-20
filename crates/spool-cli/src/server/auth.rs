//! The HTTP side of who may use the server: who is calling, signing in with
//! a password or with Google, the people and the tokens an admin manages,
//! and OAuth for `/mcp`. What is kept of them is [`crate::auth`].

mod accounts;
mod caller;
pub(super) mod google;
pub(super) mod oauth;
pub(super) mod people;
pub(super) mod sign_in;
pub(super) mod tokens;

pub(super) use accounts::list_actor_names;
pub(super) use caller::{Caller, authorize, authorize_mcp, own_pages_only};
pub(super) use google::Held;
pub(super) use sign_in::Attempts;
