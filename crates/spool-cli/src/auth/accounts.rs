//! The people who may use a server, their sessions, and the tokens of
//! callers that are not people.
//!
//! Accounts are a SQLite file beside the store. The store is rebuilt from
//! its inputs when its format changes; the accounts are not, so they are
//! kept apart. Passwords are stored as Argon2 hashes, and a session or a
//! token as its SHA-256, so the file holds nothing that lets anyone in.
//!
//! A person signs in with a password, with Google, or with either. Google
//! says who someone is; whether they have an account is decided here, by
//! the email an admin gave it.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::Connection;

use super::grants;

/// SQLite's clock, as the API writes a time.
pub(super) const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

pub(super) const MINUTE_AGO: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 minute')";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS person (
  id       TEXT PRIMARY KEY,
  username TEXT NOT NULL UNIQUE,
  name     TEXT NOT NULL,
  password TEXT,               -- an Argon2 hash; without one, nobody signs in with a password
  admin    INTEGER NOT NULL,
  active   INTEGER NOT NULL,
  created  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE TABLE IF NOT EXISTS token (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL UNIQUE,  -- what it changes is recorded under `agent:<name>`
  secret       TEXT NOT NULL UNIQUE,  -- the SHA-256 of the token
  created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  created_by   TEXT,                  -- the person who made it; nobody, when `spool token add` did
  last_used_at TEXT
);
CREATE TABLE IF NOT EXISTS session (
  id      TEXT PRIMARY KEY,    -- the SHA-256 of the session's ID
  data    TEXT NOT NULL,
  expires INTEGER NOT NULL     -- seconds since 1970
);
";

/// What each version of the file adds to the one before. `SCHEMA` is the
/// file as it first was, and `PRAGMA user_version` says how many of these it
/// has had.
const STEPS: [&str; 2] = [
    "
ALTER TABLE person ADD COLUMN email TEXT;   -- who they are to Google, as an admin gave it
ALTER TABLE person ADD COLUMN google TEXT;  -- the Google account that signed in with that email
ALTER TABLE person ADD COLUMN bound TEXT;   -- what its sessions began with: new each time one is bound
CREATE UNIQUE INDEX person_email ON person (email);
CREATE UNIQUE INDEX person_google ON person (google);
",
    grants::SCHEMA,
];

/// Why an account could not be made or changed. Each says what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// What was sent cannot be used: the message says which part.
    Invalid(String),
    /// The username belongs to someone else.
    Taken(String),
    /// The email belongs to someone else.
    EmailTaken(String),
    /// There is no such person.
    Unknown,
    /// A token already has the name.
    TokenTaken(String),
    /// There is no such token.
    UnknownToken,
    /// No approval of an assistant has the ID.
    UnknownGrant,
    /// The change would leave nobody able to manage people.
    LastAdmin,
    /// An admin can already sign in: the first account has been made.
    SetUp,
    /// The password given as the current one is not.
    WrongPassword,
    /// More of something anyone may ask for is waiting than is kept: the
    /// message says what.
    TooMany(String),
    Failed(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Invalid(message) | Refused::TooMany(message) | Refused::Failed(message) => {
                f.write_str(message)
            }
            Refused::Taken(username) => write!(f, "the username `{username}` is taken"),
            Refused::EmailTaken(email) => {
                write!(f, "another account has the email `{email}`")
            }
            Refused::Unknown => f.write_str("there is no such person"),
            Refused::TokenTaken(name) => write!(
                f,
                "there is a token called `{name}` already; revoke it first, or pick another name"
            ),
            Refused::UnknownToken => f.write_str("there is no such token"),
            Refused::UnknownGrant => f.write_str("there is no such approval"),
            Refused::LastAdmin => {
                f.write_str("this is the only admin who can sign in; make another one first")
            }
            Refused::SetUp => f.write_str("this server already has an admin who can sign in"),
            Refused::WrongPassword => f.write_str("the current password is not that"),
        }
    }
}

impl From<rusqlite::Error> for Refused {
    fn from(error: rusqlite::Error) -> Self {
        Refused::Failed(format!("the accounts file could not be used: {error}"))
    }
}

#[derive(Clone, Debug)]
pub struct Accounts {
    connection: Arc<Mutex<Connection>>,
    /// Whether the server signs people in with Google, and so whether an
    /// account with an email and no password is one that can sign in.
    pub(super) google: bool,
}

impl Accounts {
    pub fn open(path: &Path) -> Result<Self, Refused> {
        let accounts = Self::over(Connection::open(path)?)?;
        // The hashes are nobody else's to read, whoever shares the machine.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(
                |error| {
                    Refused::Failed(format!(
                        "{} could not be made private: {error}",
                        path.display()
                    ))
                },
            )?;
        }
        Ok(accounts)
    }

    /// Accounts that last as long as the process, for tests.
    pub fn in_memory() -> Self {
        Self::over(Connection::open_in_memory().expect("SQLite opens in memory"))
            .expect("the accounts schema is valid")
    }

    fn over(mut connection: Connection) -> Result<Self, Refused> {
        // Another process may have the file open: `spool token` works while
        // the server runs.
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute_batch(SCHEMA)?;
        let had: u32 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        for step in STEPS.iter().skip(had as usize) {
            transaction.execute_batch(step)?;
        }
        if (had as usize) < STEPS.len() {
            transaction.pragma_update(None, "user_version", STEPS.len() as u32)?;
        }
        transaction.commit()?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            google: false,
        })
    }

    /// These accounts on a server that signs people in with Google, or does
    /// not.
    pub fn with_google(mut self, google: bool) -> Self {
        self.google = google;
        self
    }

    pub(super) fn connection(&self) -> MutexGuard<'_, Connection> {
        // A panic while the lock was held leaves SQLite's own state intact.
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A code that is hard to guess: 32 random bytes as hex.
pub fn code() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system gives random bytes");
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{PASSWORD, email};
    use super::*;

    #[test]
    fn a_file_from_before_there_were_emails_is_brought_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spool.auth.sqlite");
        {
            let before = Connection::open(&path).unwrap();
            before.execute_batch(SCHEMA).unwrap();
            before
                .execute(
                    "INSERT INTO person (id, username, name, password, admin, active)
                     VALUES ('p-1', 'ada', 'Ada', ?, 1, 1)",
                    [password_auth::generate_hash(PASSWORD)],
                )
                .unwrap();
        }
        for _ in 0..2 {
            let accounts = Accounts::open(&path).unwrap().with_google(true);
            let people = accounts.list().unwrap();
            assert_eq!(people.len(), 1);
            assert_eq!((people[0].id.as_str(), &people[0].email), ("p-1", &None));
            assert!(accounts.verify("ada", PASSWORD).unwrap().is_some());
            assert!(!accounts.needs_setup().unwrap());
        }
        let accounts = Accounts::open(&path).unwrap().with_google(true);
        let ada = accounts.change("p-1", email("ada@example.com")).unwrap();
        let entered = accounts
            .enter_with_google("g-1", Some("ada@example.com"))
            .unwrap();
        assert_eq!(entered.map(|(person, _)| person), Some(ada));
    }
}
