//! What the tests of the accounts share: a password, and people to add.

use super::{Change, New};

pub(super) const PASSWORD: &str = "a long enough one";

/// Someone who signs in with a password.
pub(super) fn new(username: &str, name: &str, password: &str) -> New {
    New {
        username: username.to_owned(),
        name: name.to_owned(),
        password: Some(password.to_owned()),
        ..New::default()
    }
}

/// Someone who signs in with Google.
pub(super) fn with_email(username: &str, email: &str) -> New {
    New {
        username: username.to_owned(),
        name: username.to_owned(),
        email: Some(email.to_owned()),
        ..New::default()
    }
}

pub(super) fn email(email: &str) -> Change {
    Change {
        email: Some(email.to_owned()),
        ..Change::default()
    }
}
