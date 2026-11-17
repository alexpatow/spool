//! Feature tables: one per declared entity type, generated from the type's
//! features query. Nothing here names a feature column other than
//! `record_id` and `declaration_version`.

use duckdb::Connection;
use spool_core::{EntityType, RunId};

use crate::error::Result;
use crate::project::{Declaration, FeatureColumn};

/// The feature table of an entity type. The name is safe to splice into SQL
/// because [`EntityType`] allows only lowercase letters, digits and `_`.
pub(crate) fn table(entity_type: &EntityType) -> String {
    format!("spool.features_{entity_type}")
}

/// The features query, reading from the records that `scope` selects.
/// Inside the query, `record` is the engine's view of the type's
/// records.
fn select(query: &str, scope: &str) -> String {
    format!(
        "WITH record AS (SELECT * FROM spool.record WHERE {scope})
         SELECT * FROM (\n{query}\n) AS features"
    )
}

/// The columns DuckDB infers for a features query.
pub(crate) fn describe(conn: &Connection, query: &str) -> Result<Vec<FeatureColumn>> {
    let mut statement = conn.prepare(&format!("DESCRIBE {}", select(query, "false")))?;
    let mut rows = statement.query([])?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next()? {
        columns.push(FeatureColumn {
            name: row.get(0)?,
            data_type: row.get(1)?,
        });
    }
    Ok(columns)
}

/// A features query must return `record_id`, and must leave to the
/// engine the columns it adds: `declaration_version` in the feature table,
/// `identity_id` in the entity view, and `identity_id` and `is_new` in the
/// feature rows a user-code strategy is given.
pub(crate) fn check_columns(columns: &[FeatureColumn]) -> Result<(), String> {
    let find = |name: &str| {
        columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(name))
    };
    match find("record_id") {
        None => return Err("the query must return a `record_id` column".to_owned()),
        Some(column) if column.data_type != "UUID" => {
            return Err(format!(
                "`record_id` must be the record's own ID, a UUID, and the query returns {}",
                column.data_type
            ));
        }
        Some(_) => {}
    }
    for reserved in ["declaration_version", "identity_id", "is_new"] {
        if find(reserved).is_some() {
            return Err(format!(
                "`{reserved}` is added by the engine; rename that column"
            ));
        }
    }
    Ok(())
}

/// Creates the empty feature table and the type's entity view.
pub(crate) fn create_table(
    conn: &Connection,
    entity_type: &EntityType,
    declaration: &Declaration,
    version: u32,
) -> Result<()> {
    let table = table(entity_type);
    conn.execute_batch(&format!(
        "CREATE TABLE {table} AS
         SELECT features.*, {version}::INTEGER AS declaration_version
         FROM ({}) AS features;

         CREATE VIEW spool.entity_{entity_type} AS
         SELECT m.identity_id, f.* EXCLUDE (declaration_version)
         FROM spool.membership_current m
         JOIN {table} f ON f.record_id = m.record_id;",
        select(&declaration.features, "false")
    ))?;
    Ok(())
}

/// Drops the feature table and the entity view, to rebuild them under a
/// changed declaration.
pub(crate) fn drop_table(conn: &Connection, entity_type: &EntityType) -> Result<()> {
    conn.execute_batch(&format!(
        "DROP VIEW IF EXISTS spool.entity_{entity_type};
         DROP TABLE IF EXISTS {};",
        table(entity_type)
    ))?;
    Ok(())
}

/// The records of one ingest run, as a filter on `spool.record`.
pub(crate) fn of_run(entity_type: &EntityType, ingest_run: RunId) -> String {
    format!("entity_type = '{entity_type}' AND ingest_run = {ingest_run}")
}

/// Every record of an entity type, as a filter on `spool.record`.
pub(crate) fn of_type(entity_type: &EntityType) -> String {
    format!("entity_type = '{entity_type}'")
}

/// Computes feature rows for the records that `scope` selects and
/// appends them. Returns what is wrong if the query returns a row that does
/// not belong to one of those records, or more than one row for an
/// record; nothing is appended then.
pub(crate) fn fill(
    conn: &Connection,
    entity_type: &EntityType,
    declaration: &Declaration,
    version: u32,
    scope: &str,
) -> Result<Option<String>> {
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE spool_new_features AS
         SELECT features.*, {version}::INTEGER AS declaration_version
         FROM ({}) AS features",
        select(&declaration.features, scope)
    ))?;

    let (stray, repeated): (i64, i64) = conn.query_row(
        &format!(
            "SELECT
               (SELECT count(*) FROM spool_new_features f
                WHERE NOT EXISTS (
                  SELECT 1 FROM spool.record o
                  WHERE o.record_id = f.record_id AND {scope})),
               (SELECT count(*) FROM (
                  SELECT record_id FROM spool_new_features
                  GROUP BY record_id HAVING count(*) > 1))"
        ),
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let problem = if stray > 0 {
        Some(format!(
            "it returned {stray} rows whose `record_id` is not one of the records it was given"
        ))
    } else if repeated > 0 {
        Some(format!(
            "it returned more than one row for {repeated} records"
        ))
    } else {
        None
    };
    if let Some(problem) = problem {
        conn.execute_batch("DROP TABLE spool_new_features")?;
        return Ok(Some(format!(
            "the features query of `{entity_type}` must return at most one row per record, and {problem}"
        )));
    }

    conn.execute_batch(&format!(
        "INSERT INTO {} SELECT * FROM spool_new_features;
         DROP TABLE spool_new_features;",
        table(entity_type)
    ))?;
    Ok(None)
}
