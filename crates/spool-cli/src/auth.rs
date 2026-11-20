//! Who may use a server: the people with an account and their sessions,
//! the tokens of callers that are not people, what a person lets an
//! assistant do with OAuth, and sign-in with Google.
//!
//! All but the last is kept in one SQLite file, which [`Accounts`] is.
//! What the server does with them over HTTP is `server/auth.rs`.

mod accounts;
mod clients;
pub mod google;
mod grants;
mod people;
mod sessions;
mod sign_in;
#[cfg(test)]
mod testing;
mod tokens;

pub use accounts::{Accounts, Refused, code};
pub use clients::Client;
pub use grants::{
    ACCESS_PREFIX, ACCESS_SECONDS, Approval, CODE_SECONDS, Issued, REFRESH_PREFIX, REFRESH_SECONDS,
    SCOPE,
};
pub use people::{Change, MAX_EMAIL, MAX_USERNAME, New};
pub use sessions::{GOOGLE, MARK, PERSON};
pub use sign_in::{Entered, MIN_PASSWORD};
pub use tokens::TOKEN_PREFIX;
