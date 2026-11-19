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
use spool_core::{EntityType, RecordId, RunId};

use crate::error::Result;
use crate::features;
use crate::project::Rule;
use crate::retract::{self, NOT_RETRACTED};
use crate::review;

/// The name of the temporary table that holds a run's new records.
pub(crate) const SCOPE: &str = "spool_scope";

/// A condition on `record_id` that leaves out the records of
/// dismissed review items: a reviewer decided they stay as they are, so no
/// run looks at them again, whether or not an earlier run got as far as
/// counting them as examined. That is about records an item left without an
/// identity. A dismissed `possible_split` says nothing of the kind: its
/// records had an identity, and one that later has none is placed like any
/// record.
pub(crate) const NOT_DISMISSED: &str = "record_id NOT IN (
    SELECT unnest(record_ids) FROM spool.review_item
    WHERE status = 'dismissed' AND kind <> 'possible_split')";

/// Lists the records that are new to a run: those of the entity type
/// that were loaded after `after_run` and before the run itself, those
/// that came back in that time after their source had taken them back, and
/// those of the given review items that still have no identity. A rematch
/// lists every record that was loaded before the run. The records of
/// dismissed review items are never among them, nor are records that are
/// taken back.
pub(crate) fn create_scope(
    conn: &Connection,
    entity_type: &EntityType,
    after_run: RunId,
    run: RunId,
    review_items: &[u64],
    rematch: bool,
) -> Result<()> {
    let after_run = if rematch { RunId(0) } else { after_run };
    let under_review = match review::unassigned_records(review_items) {
        Some(query) => format!("UNION {query}"),
        None => String::new(),
    };
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {SCOPE} AS
         SELECT record_id FROM (
           SELECT record_id FROM spool.record
           WHERE entity_type = '{entity_type}' AND load_run < {run}
             AND ((load_run > {after_run} {unmatched}) OR record_id IN ({restored}))
           {under_review}
         ) WHERE {NOT_DISMISSED} AND {NOT_RETRACTED}",
        restored = retract::restored_between(after_run, run),
        unmatched = if rematch {
            String::new()
        } else {
            not_matched_individually(after_run, Some(run))
        },
    ))?;
    Ok(())
}

/// A successful single-record match counts only for the record it examined.
pub(crate) fn not_matched_individually(after: RunId, before: Option<RunId>) -> String {
    let before = before
        .map(|run| format!("AND run_id < {run}"))
        .unwrap_or_default();
    format!(
        "AND record_id NOT IN (
           SELECT json_extract_string(config, '$.record')::UUID
           FROM spool.run WHERE kind = 'match' AND status = 'succeeded'
             AND run_id > {after} {before}
             AND json_extract_string(config, '$.record') IS NOT NULL)"
    )
}

pub(crate) fn create_record_scope(conn: &Connection, record: RecordId) -> Result<()> {
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {SCOPE} AS
         SELECT record_id FROM spool.record
         WHERE record_id = '{record}'::UUID AND {NOT_DISMISSED} AND {NOT_RETRACTED}"
    ))?;
    Ok(())
}

/// A single new record against members of existing identities. Which side
/// it is on depends on its ID, as it does for every other rule run.
pub(crate) fn generate_record_candidates(
    conn: &Connection,
    entity_type: &EntityType,
    run: RunId,
    record: RecordId,
    block: &[Rule],
) -> Result<()> {
    let table = features::table(entity_type);
    for rule in block {
        let pairs = |new: &str, existing: &str| {
            format!(
                "SELECT {run}, l.record_id, r.record_id, '{name}'
             FROM {table} l
             JOIN {table} r ON l.record_id < r.record_id AND (\n{predicate}\n)
             JOIN spool.membership m ON m.record_id = {existing}.record_id AND m.to_seq IS NULL
             WHERE {new}.record_id = '{record}'::UUID",
                name = rule.name,
                predicate = rule.predicate,
            )
        };
        conn.execute_batch(&format!(
            "INSERT INTO spool.candidate {} UNION ALL {}",
            pairs("l", "r"),
            pairs("r", "l"),
        ))?;
    }
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
    generate_candidates_into(conn, entity_type, run, block, "spool.candidate")
}

/// As [`generate_candidates`], into a table with the columns of
/// `spool.candidate`.
pub(crate) fn generate_candidates_into(
    conn: &Connection,
    entity_type: &EntityType,
    run: RunId,
    block: &[Rule],
    into: &str,
) -> Result<()> {
    let table = features::table(entity_type);
    for rule in block {
        conn.execute_batch(&format!(
            "INSERT INTO {into}
             SELECT {run}, l.record_id, r.record_id, '{name}'
             FROM {table} l
             JOIN {table} r ON l.record_id < r.record_id AND (\n{predicate}\n)
             WHERE l.record_id IN (SELECT record_id FROM {SCOPE})
                OR r.record_id IN (SELECT record_id FROM {SCOPE})",
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
    judge_candidates_into(
        conn,
        entity_type,
        run,
        no_match,
        match_,
        ("spool.candidate", "spool.judgement"),
    )
}

/// As [`judge_candidates`], from and into tables with the columns of
/// `spool.candidate` and `spool.judgement`.
pub(crate) fn judge_candidates_into(
    conn: &Connection,
    entity_type: &EntityType,
    run: RunId,
    no_match: &[Rule],
    match_: &[Rule],
    (candidates, judgements): (&str, &str),
) -> Result<()> {
    let table = features::table(entity_type);
    conn.execute_batch(&format!(
        "INSERT INTO {judgements}
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
             FROM {candidates} WHERE run_id = {run}
             GROUP BY left_id, right_id
           ) c
           JOIN {table} l ON l.record_id = c.left_id
           JOIN {table} r ON r.record_id = c.right_id
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
