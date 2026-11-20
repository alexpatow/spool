//! Feature tables: one per declared entity type, generated from the type's
//! features query. Nothing here names a feature column other than
//! `record_id` and `declaration_version`.

use duckdb::Connection;
use spool_core::{EntityType, RecordId, RunId};

use crate::batch::id_list;
use crate::error::Result;
use crate::project::{Declaration, FeatureColumn, Transform};
use crate::retract::{self, NOT_RETRACTED};

/// The feature table of an entity type. The name is safe to splice into SQL
/// because [`EntityType`] allows only lowercase letters, digits and `_`.
pub(crate) fn table(entity_type: &EntityType) -> String {
    format!("spool.features_{entity_type}")
}

/// The features query, reading from the records that `scope` selects.
/// Inside the query, `record` is the engine's view of the type's
/// records. A record its source took back is not among them: it has no
/// feature row.
fn build_features_query(query: &str, scope: &str) -> String {
    format!(
        "WITH record AS (SELECT * FROM spool.record WHERE ({scope}) AND {NOT_RETRACTED})
         SELECT * FROM (\n{query}\n) AS features"
    )
}

/// The SQL function a transform is called as. The prefix keeps a transform
/// from taking the name of a function DuckDB has.
pub(crate) fn transform_function(name: &str) -> String {
    format!("spool_transform_{name}")
}

/// Makes the transforms functions of this connection. A features query
/// calls them, so they are defined before one is prepared or run.
fn define_transforms(conn: &Connection, transforms: &[Transform]) -> Result<()> {
    for transform in transforms {
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP MACRO {}({}) AS (\n{}\n)",
            transform_function(&transform.name),
            transform.parameters.join(", "),
            transform.body
        ))?;
    }
    Ok(())
}

/// Defines a transform and prepares a call of it with text for every
/// argument, which fails if its expression is not one over its arguments.
pub(crate) fn check_transform(conn: &Connection, transform: &Transform) -> Result<()> {
    define_transforms(conn, std::slice::from_ref(transform))?;
    let arguments = vec!["NULL::VARCHAR"; transform.parameters.len()].join(", ");
    conn.prepare(&format!(
        "SELECT {}({arguments})",
        transform_function(&transform.name)
    ))?;
    Ok(())
}

/// The columns DuckDB infers for a features query.
pub(crate) fn describe(
    conn: &Connection,
    transforms: &[Transform],
    query: &str,
) -> Result<Vec<FeatureColumn>> {
    define_transforms(conn, transforms)?;
    let mut statement = conn.prepare(&format!(
        "DESCRIBE {}",
        build_features_query(query, "false")
    ))?;
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

/// Creates the empty feature table and the type's entity view.
pub(crate) fn create_table(
    conn: &Connection,
    entity_type: &EntityType,
    declaration: &Declaration,
    version: u32,
) -> Result<()> {
    let table = table(entity_type);
    define_transforms(conn, &declaration.transforms)?;
    conn.execute_batch(&format!(
        "CREATE TABLE {table} AS
         SELECT features.*, {version}::INTEGER AS declaration_version
         FROM ({}) AS features;

         CREATE VIEW spool.entity_{entity_type} AS
         SELECT m.identity_id, f.* EXCLUDE (declaration_version)
         FROM spool.membership_current m
         JOIN {table} f ON f.record_id = m.record_id;",
        build_features_query(&declaration.features, "false")
    ))?;
    Ok(())
}

/// Makes `spool.record_label` say what to call each record: the value of
/// the feature its type names as its label. A type that names none has no
/// rows there.
pub(crate) fn label_view<'a>(
    conn: &Connection,
    types: impl Iterator<Item = (&'a EntityType, &'a Declaration)>,
) -> Result<()> {
    let labelled: Vec<String> = types
        .filter_map(|(entity_type, declaration)| {
            let label = declaration.label.as_deref()?;
            Some(format!(
                "SELECT record_id, nullif(trim({label}::VARCHAR), '') AS label FROM {}",
                table(entity_type)
            ))
        })
        .collect();
    let query = if labelled.is_empty() {
        NO_LABELS.to_owned()
    } else {
        labelled.join("\nUNION ALL\n")
    };
    conn.execute_batch(&format!(
        "CREATE OR REPLACE VIEW spool.record_label AS\n{query}"
    ))?;
    Ok(())
}

/// The label view of a store in which no type names a label.
pub(crate) const NO_LABELS: &str =
    "SELECT NULL::UUID AS record_id, NULL::VARCHAR AS label WHERE false";

/// Recomputes labels for an ordered sample with a saved declaration. The
/// sample can include records since retracted and absent from today's features.
pub(crate) fn snapshot_labels(
    conn: &Connection,
    declaration: &Declaration,
    label: &str,
    records: &[RecordId],
) -> Result<Vec<String>> {
    define_transforms(conn, &declaration.transforms)?;
    let mut statement = conn.prepare(&format!(
        "WITH picked AS (SELECT id::UUID AS record_id, ord FROM arrow(?)),
              record AS (SELECT r.* FROM spool.record r JOIN picked USING (record_id))
         SELECT nullif(trim(f.{label}::VARCHAR), '')
         FROM (\n{}\n) f JOIN picked USING (record_id) ORDER BY picked.ord",
        declaration.features
    ))?;
    let mut rows = statement.query([&id_list(records)?])?;
    let mut labels = Vec::new();
    while let Some(row) = rows.next()? {
        if let Some(label) = row.get(0)? {
            labels.push(label);
        }
    }
    Ok(labels)
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

/// The records of one load run, as a filter on `spool.record`: those
/// it created, and those that came back in it after their source had taken
/// them back.
pub(crate) fn of_run(entity_type: &EntityType, load_run: RunId) -> String {
    format!(
        "entity_type = '{entity_type}'
         AND (load_run = {load_run} OR record_id IN ({}))",
        retract::restored_in(load_run)
    )
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
    define_transforms(conn, &declaration.transforms)?;
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE spool_new_features AS
         SELECT features.*, {version}::INTEGER AS declaration_version
         FROM ({}) AS features",
        build_features_query(&declaration.features, scope)
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
