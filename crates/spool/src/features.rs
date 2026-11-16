//! Feature tables: one per declared entity type, generated from the type's
//! features query. Nothing here names a feature column other than
//! `observation_id` and `declaration_version`.

use duckdb::Connection;
use spool_core::{EntityType, RunId};

use crate::error::{Error, Result};
use crate::project::{Declaration, FeatureColumn};

/// The feature table of an entity type. The name is safe to splice into SQL
/// because [`EntityType`] allows only lowercase letters, digits and `_`.
pub(crate) fn table(entity_type: &EntityType) -> String {
    format!("spool.features_{entity_type}")
}

/// The features query, reading from the observations that `scope` selects.
/// Inside the query, `observation` is the engine's view of the type's
/// observations.
fn select(query: &str, scope: &str) -> String {
    format!(
        "WITH observation AS (SELECT * FROM spool.observation WHERE {scope})
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

/// A features query must return `observation_id`, and must leave
/// `declaration_version` to the engine.
pub(crate) fn check_columns(columns: &[FeatureColumn]) -> Result<(), String> {
    let find = |name: &str| {
        columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(name))
    };
    match find("observation_id") {
        None => return Err("the query must return an `observation_id` column".to_owned()),
        Some(column) if column.data_type != "UUID" => {
            return Err(format!(
                "`observation_id` must be the observation's own ID, a UUID, and the query returns {}",
                column.data_type
            ));
        }
        Some(_) => {}
    }
    if find("declaration_version").is_some() {
        return Err("`declaration_version` is added by the engine; rename that column".to_owned());
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
         JOIN {table} f ON f.observation_id = m.observation_id;",
        select(&declaration.features, "false")
    ))?;
    Ok(())
}

/// Computes feature rows for the observations of one ingest run and appends
/// them. Fails if the query returns a row that does not belong to one of
/// those observations, or more than one row for an observation.
pub(crate) fn fill(
    conn: &Connection,
    entity_type: &EntityType,
    declaration: &Declaration,
    version: u32,
    ingest_run: RunId,
) -> Result<()> {
    let scope = format!("entity_type = '{entity_type}' AND ingest_run = {ingest_run}");
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE spool_new_features AS
         SELECT features.*, {version}::INTEGER AS declaration_version
         FROM ({}) AS features",
        select(&declaration.features, &scope)
    ))?;

    let (stray, repeated): (i64, i64) = conn.query_row(
        &format!(
            "SELECT
               (SELECT count(*) FROM spool_new_features f
                WHERE NOT EXISTS (
                  SELECT 1 FROM spool.observation o
                  WHERE o.observation_id = f.observation_id AND {scope})),
               (SELECT count(*) FROM (
                  SELECT observation_id FROM spool_new_features
                  GROUP BY observation_id HAVING count(*) > 1))"
        ),
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let problem = if stray > 0 {
        Some(format!(
            "it returned {stray} rows whose `observation_id` is not one of the observations it was given"
        ))
    } else if repeated > 0 {
        Some(format!(
            "it returned more than one row for {repeated} observations"
        ))
    } else {
        None
    };
    if let Some(problem) = problem {
        conn.execute_batch("DROP TABLE spool_new_features")?;
        return Err(Error::Ingest(format!(
            "the features query of `{entity_type}` must return at most one row per observation, and {problem}"
        )));
    }

    conn.execute_batch(&format!(
        "INSERT INTO {} SELECT * FROM spool_new_features;
         DROP TABLE spool_new_features;",
        table(entity_type)
    ))?;
    Ok(())
}
