//! The store's tables. Everything lives in the `spool` schema, so a user's own
//! tables can share the file.

use duckdb::Connection;

use crate::error::{Error, Result};

/// The format written to `spool.meta` after creation or migration.
pub(crate) const FORMAT_VERSION: &str = "12";

const BASE_VERSION: &str = "11";
const BASE_SCHEMA: &str = include_str!("../migrations/011_baseline.sql");

// From version, to version, and SQL. Append steps in upgrade order and
// keep the SQL for released formats fixed.
const MIGRATIONS: &[(&str, &str, &str)] = &[(
    "11",
    "12",
    include_str!("../migrations/012_extraction_history.sql"),
)];

/// Creates or upgrades the schema and records its version in one transaction.
pub(crate) fn ensure(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    let exists: bool = tx.query_row(
        "SELECT count(*) > 0 FROM information_schema.schemata WHERE schema_name = 'spool'",
        [],
        |row| row.get(0),
    )?;
    let found = if exists {
        read_meta(&tx, "format_version")?.unwrap_or_else(|| "none".to_owned())
    } else {
        tx.execute_batch(BASE_SCHEMA)?;
        BASE_VERSION.to_owned()
    };

    if !exists || found != FORMAT_VERSION {
        let mut version = found.as_str();
        for &(from, to, sql) in MIGRATIONS {
            if version == FORMAT_VERSION {
                break;
            }
            if version == from {
                tx.execute_batch(sql)?;
                version = to;
            }
        }
        if version != FORMAT_VERSION {
            return Err(Error::FormatVersion {
                found,
                expected: FORMAT_VERSION.to_owned(),
            });
        }
        set_meta(&tx, "format_version", FORMAT_VERSION)?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn read_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut statement = conn.prepare("SELECT value FROM spool.meta WHERE key = ?")?;
    let mut rows = statement.query([key])?;
    Ok(match rows.next()? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

pub(crate) fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute("DELETE FROM spool.meta WHERE key = ?", [key])?;
    conn.execute("INSERT INTO spool.meta VALUES (?, ?)", [key, value])?;
    Ok(())
}
