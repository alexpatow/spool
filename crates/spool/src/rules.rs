//! The built-in strategy: a runner for the SQL rules a project declares. It
//! knows nothing about any entity type and contains no rules of its own.
//!
//! - Each `block` predicate generates candidate pairs.
//! - A candidate for which any `no_match` predicate holds is judged `no_match`.
//! - Otherwise, a candidate for which any `match` predicate holds is judged
//!   `match`.
//! - Other candidates get no judgement.
//!
//! The names of the predicates that held are recorded in the judgement's
//! `detail`.

use duckdb::Connection;
use spool_core::{EntityType, RunId};

use crate::error::Result;
use crate::features;
use crate::project::Rule;
use crate::review;

/// The name of the temporary table that holds a run's new observations.
pub(crate) const SCOPE: &str = "spool_scope";

/// A condition on `observation_id` that leaves out the observations of
/// dismissed review items: a reviewer decided they stay as they are, so no
/// run looks at them again, whether or not an earlier run got as far as
/// counting them as examined.
pub(crate) const NOT_DISMISSED: &str = "observation_id NOT IN (
    SELECT unnest(observation_ids) FROM spool.review_item WHERE status = 'dismissed')";

/// Lists the observations that are new to a run: those of the entity type
/// that were ingested after `after_run` and before the run itself, and those
/// of the given review items that still have no identity. The observations
/// of dismissed review items are never among them.
pub(crate) fn create_scope(
    conn: &Connection,
    entity_type: &EntityType,
    after_run: RunId,
    run: RunId,
    review_items: &[u64],
) -> Result<()> {
    let under_review = match review::unassigned_observations(review_items) {
        Some(query) => format!("UNION {query}"),
        None => String::new(),
    };
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {SCOPE} AS
         SELECT observation_id FROM (
           SELECT observation_id FROM spool.observation
           WHERE entity_type = '{entity_type}' AND ingest_run > {after_run} AND ingest_run < {run}
           {under_review}
         ) WHERE {NOT_DISMISSED}"
    ))?;
    Ok(())
}

/// Runs each block rule as its own join, keeping the pairs in which at least
/// one side is new. A pair found by several rules has one row per rule.
pub(crate) fn generate_candidates(
    conn: &Connection,
    entity_type: &EntityType,
    run: RunId,
    block: &[Rule],
) -> Result<()> {
    let table = features::table(entity_type);
    for rule in block {
        conn.execute_batch(&format!(
            "INSERT INTO spool.candidate
             SELECT {run}, l.observation_id, r.observation_id, '{name}'
             FROM {table} l
             JOIN {table} r ON l.observation_id < r.observation_id AND (\n{predicate}\n)
             WHERE l.observation_id IN (SELECT observation_id FROM {SCOPE})
                OR r.observation_id IN (SELECT observation_id FROM {SCOPE})",
            name = rule.name,
            predicate = rule.predicate,
        ))?;
    }
    Ok(())
}

/// Judges the run's candidates with the `no_match` and `match` rules.
pub(crate) fn judge_candidates(
    conn: &Connection,
    entity_type: &EntityType,
    run: RunId,
    no_match: &[Rule],
    match_: &[Rule],
) -> Result<()> {
    let table = features::table(entity_type);
    conn.execute_batch(&format!(
        "INSERT INTO spool.judgement
         SELECT {run}, left_id, right_id,
                CASE WHEN len(no_match) > 0 THEN 'no_match' ELSE 'match' END,
                NULL,
                json_object('block', block, 'no_match', no_match, 'match', \"match\")
         FROM (
           SELECT c.left_id, c.right_id, c.block,
                  {no_match} AS no_match,
                  {match_} AS \"match\"
           FROM (
             SELECT left_id, right_id, list(rule ORDER BY rule) AS block
             FROM spool.candidate WHERE run_id = {run}
             GROUP BY left_id, right_id
           ) c
           JOIN {table} l ON l.observation_id = c.left_id
           JOIN {table} r ON r.observation_id = c.right_id
         )
         WHERE len(no_match) > 0 OR len(\"match\") > 0",
        no_match = names_that_hold(no_match),
        match_ = names_that_hold(match_),
    ))?;
    Ok(())
}

/// A SQL expression for the list of rule names whose predicate holds.
fn names_that_hold(rules: &[Rule]) -> String {
    if rules.is_empty() {
        return "[]::VARCHAR[]".to_owned();
    }
    let cases: Vec<String> = rules
        .iter()
        .map(|rule| {
            format!(
                "CASE WHEN (\n{}\n) THEN '{}' END",
                rule.predicate, rule.name
            )
        })
        .collect();
    format!(
        "list_filter([{}], lambda name: name IS NOT NULL)",
        cases.join(", ")
    )
}
