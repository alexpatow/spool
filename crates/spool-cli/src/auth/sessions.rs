//! The sessions of people who are signed in: what a session keeps, and the
//! accounts file as where the server's session layer stores them.

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use tower_sessions::cookie::time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, SessionStore};

use super::accounts::{Accounts, Refused};
use super::grants;

/// The key of a session under which the person it belongs to is kept.
pub const PERSON: &str = "person";

/// The key of a session under which the mark of the password it began with
/// is kept.
pub const MARK: &str = "password";

/// The key of a session that began with Google, under which the mark of the
/// account being that Google account's is kept.
pub const GOOGLE: &str = "google";

/// Ends every session of a person, and every approval of an assistant they
/// gave: those stay ended when the account is turned on again.
pub(super) fn end_sessions(connection: &Connection, person: &str) -> Result<(), Refused> {
    connection.execute(
        &format!("DELETE FROM session WHERE json_extract(data, '$.{PERSON}') = ?"),
        [person],
    )?;
    grants::end_approvals(connection, person, None)
}

fn stored_id(id: &Id) -> String {
    hex::encode(Sha256::digest(id.0.to_le_bytes()))
}

fn to_backend_error(error: impl std::fmt::Display) -> session_store::Error {
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
                .map_err(to_backend_error)?;
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
            .map_err(to_backend_error)?;
        Ok(())
    }

    async fn load(&self, id: &Id) -> session_store::Result<Option<Record>> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let connection = self.connection();
        connection
            .execute("DELETE FROM session WHERE expires <= ?", [now])
            .map_err(to_backend_error)?;
        let found: Option<(String, i64)> = connection
            .query_row(
                "SELECT data, expires FROM session WHERE id = ?",
                [stored_id(id)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(to_backend_error)?;
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
            .map_err(to_backend_error)?;
        Ok(())
    }
}
