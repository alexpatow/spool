//! The people who may use a server, and their sessions.
//!
//! Accounts are a SQLite file beside the store. The store is rebuilt from
//! its inputs when its format changes; the accounts are not, so they are
//! kept apart. Passwords are stored as Argon2 hashes, and a session as the
//! hash of its ID, so the file holds nothing that signs anyone in.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use tower_sessions::cookie::time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, SessionStore};
use uuid::Uuid;

use crate::api::Person;

/// The key of a session under which the person it belongs to is kept.
pub const PERSON: &str = "person";

/// The fewest characters a password may have.
pub const MIN_PASSWORD: usize = 12;

/// The most bytes a username has.
pub const MAX_USERNAME: usize = 64;

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
CREATE TABLE IF NOT EXISTS session (
  id      TEXT PRIMARY KEY,    -- the SHA-256 of the session's ID
  data    TEXT NOT NULL,
  expires INTEGER NOT NULL     -- seconds since 1970
);
";

/// Why an account could not be made or changed. Each says what to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// What was sent cannot be used: the message says which part.
    Invalid(String),
    /// The username belongs to someone else.
    Taken(String),
    /// There is no such person.
    Unknown,
    /// The change would leave nobody able to manage people.
    LastAdmin,
    /// The first account has already been made.
    NotEmpty,
    /// The password given as the current one is not.
    WrongPassword,
    Failed(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Invalid(message) | Refused::Failed(message) => f.write_str(message),
            Refused::Taken(username) => write!(f, "the username `{username}` is taken"),
            Refused::Unknown => f.write_str("there is no such person"),
            Refused::LastAdmin => {
                f.write_str("this is the only admin who can sign in; make another one first")
            }
            Refused::NotEmpty => f.write_str("this server already has its first account"),
            Refused::WrongPassword => f.write_str("the current password is not that"),
        }
    }
}

impl From<rusqlite::Error> for Refused {
    fn from(error: rusqlite::Error) -> Self {
        Refused::Failed(format!("the accounts file could not be used: {error}"))
    }
}

/// What to change about a person. A field left out stays as it is.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub name: Option<String>,
    pub password: Option<String>,
    pub admin: Option<bool>,
    pub active: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct Accounts {
    connection: Arc<Mutex<Connection>>,
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

    fn over(connection: Connection) -> Result<Self, Refused> {
        connection.execute_batch(SCHEMA)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        // A panic while the lock was held leaves SQLite's own state intact.
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether nobody has an account yet.
    pub fn is_empty(&self) -> Result<bool, Refused> {
        let people: i64 =
            self.connection()
                .query_row("SELECT count(*) FROM person", [], |row| row.get(0))?;
        Ok(people == 0)
    }

    /// Makes the first account, an admin. Refused once anyone has one.
    pub fn first(&self, username: &str, name: &str, password: &str) -> Result<Person, Refused> {
        let person = drafted(username, name, true)?;
        let hash = hashed(password)?;
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let people: i64 =
            transaction.query_row("SELECT count(*) FROM person", [], |row| row.get(0))?;
        if people != 0 {
            return Err(Refused::NotEmpty);
        }
        insert(&transaction, &person, Some(&hash))?;
        transaction.commit()?;
        Ok(person)
    }

    pub fn add(
        &self,
        username: &str,
        name: &str,
        password: &str,
        admin: bool,
    ) -> Result<Person, Refused> {
        let person = drafted(username, name, admin)?;
        let hash = hashed(password)?;
        insert(&self.connection(), &person, Some(&hash))?;
        Ok(person)
    }

    /// The account `spool dev` works as: an admin nobody can sign in as with
    /// a password. Made the first time it is asked for.
    pub fn dev(&self) -> Result<Person, Refused> {
        let connection = self.connection();
        if let Some(person) = find(&connection, "username", "dev")? {
            return Ok(person);
        }
        let person = drafted("dev", "Dev", true)?;
        insert(&connection, &person, None)?;
        Ok(person)
    }

    pub fn get(&self, id: &str) -> Result<Option<Person>, Refused> {
        find(&self.connection(), "id", id)
    }

    /// Everyone, in the order they were added.
    pub fn list(&self) -> Result<Vec<Person>, Refused> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT id, username, name, admin, active FROM person ORDER BY created, username",
        )?;
        let people = statement
            .query_map([], person)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(people)
    }

    /// The person with this username, if the password is theirs and their
    /// account is on. Takes as long whether or not there is such a person.
    pub fn verify(&self, username: &str, password: &str) -> Result<Option<Person>, Refused> {
        let username = username.trim().to_lowercase();
        let found: Option<(Person, Option<String>)> = self
            .connection()
            .query_row(
                "SELECT id, username, name, admin, active, password FROM person WHERE username = ?",
                [&username],
                |row| Ok((person(row)?, row.get(5)?)),
            )
            .optional()?;
        match found {
            Some((person, Some(hash))) => {
                let right = password_auth::verify_password(password, &hash).is_ok();
                Ok((right && person.active).then_some(person))
            }
            _ => {
                // The work a real check does, so that the time taken does
                // not say whether the username exists.
                let _ = password_auth::verify_password(password, nobody());
                Ok(None)
            }
        }
    }

    /// Changes a person. Turning an account off, or changing its password,
    /// ends its sessions.
    pub fn change(&self, id: &str, change: Change) -> Result<Person, Refused> {
        let name = change.name.as_deref().map(named).transpose()?;
        let hash = change.password.as_deref().map(hashed).transpose()?;
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let before = find(&transaction, "id", id)?.ok_or(Refused::Unknown)?;
        let after = Person {
            name: name.unwrap_or(before.name.clone()),
            admin: change.admin.unwrap_or(before.admin),
            active: change.active.unwrap_or(before.active),
            ..before.clone()
        };
        transaction.execute(
            "UPDATE person SET name = ?, admin = ?, active = ?, password = coalesce(?, password) WHERE id = ?",
            params![after.name, after.admin, after.active, hash, id],
        )?;
        let admins: i64 = transaction.query_row(
            "SELECT count(*) FROM person WHERE admin AND active",
            [],
            |row| row.get(0),
        )?;
        if admins == 0 {
            return Err(Refused::LastAdmin);
        }
        if hash.is_some() || !after.active {
            end_sessions(&transaction, id)?;
        }
        transaction.commit()?;
        Ok(after)
    }

    /// Changes a person's own password, given the one they have. Their
    /// other sessions end.
    pub fn change_password(&self, id: &str, current: &str, new: &str) -> Result<(), Refused> {
        let stored: Option<Option<String>> = self
            .connection()
            .query_row("SELECT password FROM person WHERE id = ?", [id], |row| {
                row.get(0)
            })
            .optional()?;
        let stored = stored
            .ok_or(Refused::Unknown)?
            .ok_or(Refused::WrongPassword)?;
        password_auth::verify_password(current, &stored).map_err(|_| Refused::WrongPassword)?;
        let hash = hashed(new)?;
        let connection = self.connection();
        connection.execute(
            "UPDATE person SET password = ? WHERE id = ?",
            params![hash, id],
        )?;
        end_sessions(&connection, id)?;
        Ok(())
    }
}

fn person(row: &rusqlite::Row<'_>) -> rusqlite::Result<Person> {
    Ok(Person {
        id: row.get(0)?,
        username: row.get(1)?,
        name: row.get(2)?,
        admin: row.get(3)?,
        active: row.get(4)?,
    })
}

fn find(connection: &Connection, column: &str, value: &str) -> Result<Option<Person>, Refused> {
    Ok(connection
        .query_row(
            &format!("SELECT id, username, name, admin, active FROM person WHERE {column} = ?"),
            [value],
            person,
        )
        .optional()?)
}

fn insert(connection: &Connection, person: &Person, hash: Option<&str>) -> Result<(), Refused> {
    let inserted = connection.execute(
        "INSERT INTO person (id, username, name, password, admin, active) VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (username) DO NOTHING",
        params![
            person.id,
            person.username,
            person.name,
            hash,
            person.admin,
            person.active
        ],
    )?;
    match inserted {
        0 => Err(Refused::Taken(person.username.clone())),
        _ => Ok(()),
    }
}

fn end_sessions(connection: &Connection, person: &str) -> Result<(), Refused> {
    connection.execute(
        &format!("DELETE FROM session WHERE json_extract(data, '$.{PERSON}') = ?"),
        [person],
    )?;
    Ok(())
}

/// A new account as it would be stored, with a new ID.
fn drafted(username: &str, name: &str, admin: bool) -> Result<Person, Refused> {
    let username = username.trim().to_lowercase();
    let plain = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-@".contains(c);
    if username.is_empty() || username.len() > MAX_USERNAME || !username.chars().all(plain) {
        return Err(Refused::Invalid(
            "a username is 1 to 64 letters, digits, `.`, `_`, `-` or `@`".to_owned(),
        ));
    }
    Ok(Person {
        id: Uuid::now_v7().to_string(),
        username,
        name: named(name)?,
        admin,
        active: true,
    })
}

fn named(name: &str) -> Result<String, Refused> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(Refused::Invalid("a name is 1 to 200 characters".to_owned()));
    }
    Ok(name.to_owned())
}

fn hashed(password: &str) -> Result<String, Refused> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(Refused::Invalid(format!(
            "a password has at least {MIN_PASSWORD} characters"
        )));
    }
    Ok(password_auth::generate_hash(password))
}

/// A hash no password matches, to check against when there is no account.
fn nobody() -> &'static str {
    static HASH: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| password_auth::generate_hash(Uuid::now_v7().as_bytes()));
    &HASH
}

/// A code that is hard to guess: 32 random bytes as hex.
pub fn code() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system gives random bytes");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn stored_id(id: &Id) -> String {
    Sha256::digest(id.0.to_le_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn backend(error: impl std::fmt::Display) -> session_store::Error {
    session_store::Error::Backend(error.to_string())
}

#[async_trait]
impl SessionStore for Accounts {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        let data = serde_json::to_string(&record.data)
            .map_err(|error| session_store::Error::Encode(error.to_string()))?;
        let connection = self.connection();
        loop {
            let inserted = connection
                .execute(
                    "INSERT INTO session (id, data, expires) VALUES (?, ?, ?) ON CONFLICT (id) DO NOTHING",
                    params![stored_id(&record.id), data, record.expiry_date.unix_timestamp()],
                )
                .map_err(backend)?;
            if inserted == 1 {
                return Ok(());
            }
            record.id = Id::default();
        }
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let data = serde_json::to_string(&record.data)
            .map_err(|error| session_store::Error::Encode(error.to_string()))?;
        self.connection()
            .execute(
                "INSERT INTO session (id, data, expires) VALUES (?, ?, ?)
                 ON CONFLICT (id) DO UPDATE SET data = excluded.data, expires = excluded.expires",
                params![
                    stored_id(&record.id),
                    data,
                    record.expiry_date.unix_timestamp()
                ],
            )
            .map_err(backend)?;
        Ok(())
    }

    async fn load(&self, id: &Id) -> session_store::Result<Option<Record>> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let connection = self.connection();
        connection
            .execute("DELETE FROM session WHERE expires <= ?", [now])
            .map_err(backend)?;
        let found: Option<(String, i64)> = connection
            .query_row(
                "SELECT data, expires FROM session WHERE id = ?",
                [stored_id(id)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(backend)?;
        let Some((data, expires)) = found else {
            return Ok(None);
        };
        Ok(Some(Record {
            id: *id,
            data: serde_json::from_str(&data)
                .map_err(|error| session_store::Error::Decode(error.to_string()))?,
            expiry_date: OffsetDateTime::from_unix_timestamp(expires)
                .map_err(|error| session_store::Error::Decode(error.to_string()))?,
        }))
    }

    async fn delete(&self, id: &Id) -> session_store::Result<()> {
        self.connection()
            .execute("DELETE FROM session WHERE id = ?", [stored_id(id)])
            .map_err(backend)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "a long enough one";

    #[test]
    fn the_first_account_is_an_admin_and_there_is_only_one_first() {
        let accounts = Accounts::in_memory();
        assert!(accounts.is_empty().unwrap());
        let ada = accounts.first("Ada", " Ada Lovelace ", PASSWORD).unwrap();
        assert!(ada.admin && ada.active);
        assert_eq!(
            (ada.username.as_str(), ada.name.as_str()),
            ("ada", "Ada Lovelace")
        );
        assert_eq!(
            accounts.first("eve", "Eve", PASSWORD),
            Err(Refused::NotEmpty)
        );
        assert_eq!(accounts.list().unwrap(), [ada]);
    }

    #[test]
    fn a_password_signs_in_only_its_own_active_account() {
        let accounts = Accounts::in_memory();
        let ada = accounts.first("ada", "Ada", PASSWORD).unwrap();
        let bob = accounts
            .add("bob", "Bob", "another long one", false)
            .unwrap();
        assert_eq!(
            accounts.verify(" ADA ", PASSWORD).unwrap(),
            Some(ada.clone())
        );
        assert_eq!(accounts.verify("ada", "another long one").unwrap(), None);
        assert_eq!(accounts.verify("nobody", PASSWORD).unwrap(), None);

        let off = Change {
            active: Some(false),
            ..Change::default()
        };
        assert!(!accounts.change(&bob.id, off.clone()).unwrap().active);
        assert_eq!(accounts.verify("bob", "another long one").unwrap(), None);
        // Someone must be left to manage people.
        assert_eq!(accounts.change(&ada.id, off), Err(Refused::LastAdmin));
    }

    #[test]
    fn what_cannot_be_an_account_is_refused() {
        let accounts = Accounts::in_memory();
        accounts.first("ada", "Ada", PASSWORD).unwrap();
        for (username, name, password) in [
            ("", "Ada", PASSWORD),
            ("a b", "Ada", PASSWORD),
            ("bob", " ", PASSWORD),
            ("bob", "Bob", "short"),
        ] {
            let refused = accounts.add(username, name, password, false).unwrap_err();
            assert!(matches!(refused, Refused::Invalid(_)), "{refused:?}");
        }
        assert_eq!(
            accounts.add("ADA", "Other", PASSWORD, false),
            Err(Refused::Taken("ada".to_owned()))
        );
    }

    #[test]
    fn the_dev_account_has_no_password() {
        let accounts = Accounts::in_memory();
        let dev = accounts.dev().unwrap();
        assert_eq!(accounts.dev().unwrap(), dev);
        assert!(dev.admin);
        assert_eq!(accounts.verify("dev", "").unwrap(), None);
    }

    #[test]
    fn a_new_password_needs_the_old_one() {
        let accounts = Accounts::in_memory();
        let ada = accounts.first("ada", "Ada", PASSWORD).unwrap();
        assert_eq!(
            accounts.change_password(&ada.id, "not it at all", "the next long one"),
            Err(Refused::WrongPassword)
        );
        accounts
            .change_password(&ada.id, PASSWORD, "the next long one")
            .unwrap();
        assert_eq!(accounts.verify("ada", PASSWORD).unwrap(), None);
        assert!(
            accounts
                .verify("ada", "the next long one")
                .unwrap()
                .is_some()
        );
    }
}
