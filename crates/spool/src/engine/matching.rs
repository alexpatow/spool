//! Match runs: starting one, its candidate pairs, and the judgements it
//! takes from a caller, from the rules or from user code.

use duckdb::arrow::record_batch::{RecordBatch, RecordBatchReader};
use duckdb::vtab::arrow::ArrowBatchRegistration;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use spool_core::{Actor, EntityType, RecordId, RunId};

use super::{Engine, Inner, RunRow, run_row};
use crate::error::{Error, Result};
use crate::features;
use crate::project::Strategy;
use crate::review;
use crate::rows::Rows;
use crate::rules;
use crate::runs::RunStatus;
use crate::usercode::{self, Task, TaskInfo, UserCode};

/// Names the strategy that produces a match run's judgements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StrategyRef {
    pub name: String,
    pub version: Option<String>,
}

impl StrategyRef {
    /// The built-in runner of the project's SQL rules.
    pub const RULES: &'static str = "rules";

    pub fn rules() -> Self {
        Self {
            name: Self::RULES.to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }
    }

    /// A strategy outside the engine that reads a run's candidates and
    /// submits judgements.
    pub fn external(name: impl Into<String>, version: Option<String>) -> Self {
        Self {
            name: name.into(),
            version,
        }
    }
}

/// Which records a match run treats as new: those of one entity type
/// loaded since the last batch match of that type that succeeded and still
/// need to be examined.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub entity_type: EntityType,
    /// Examine every record of the type again, not only the new ones.
    /// See [`ResolveOptions::rematch`](super::ResolveOptions::rematch).
    pub rematch: bool,
}

impl Scope {
    /// The records of a type that are new.
    pub fn new(entity_type: EntityType) -> Self {
        Self {
            entity_type,
            rematch: false,
        }
    }

    /// Every record of a type.
    pub fn rematch(entity_type: EntityType) -> Self {
        Self {
            entity_type,
            rematch: true,
        }
    }
}

/// What a match run recorded about itself when it began.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct MatchConfig {
    pub(super) entity_type: EntityType,
    pub(super) declaration_version: u32,
    pub(super) manifest_hash: String,
    /// New records are those loaded after this run.
    pub(super) after_run: RunId,
    /// The review items that were open when the run began. Their records
    /// are examined again.
    #[serde(default)]
    pub(super) review_items: Vec<u64>,
    /// Set for a match that examined only this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) record: Option<RecordId>,
    /// Every record of the type is examined, not only the new ones.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) rematch: bool,
    /// The run judged every record for a recomputation that was applied.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) recompute: bool,
    pub(super) rules: crate::project::RuleSet,
}

impl MatchConfig {
    pub(super) fn create_scope(&self, conn: &Connection, run: RunId) -> Result<()> {
        match self.record {
            Some(record) => rules::create_record_scope(conn, record),
            None => rules::create_scope(
                conn,
                &self.entity_type,
                self.after_run,
                run,
                &self.review_items,
                self.rematch,
            ),
        }
    }
}

impl Engine {
    /// Starts a match run and generates its candidate pairs with the
    /// project's block rules. The run examines the records loaded
    /// since the last match run of the type that succeeded, and again those
    /// of open review items.
    ///
    /// The run is `open`: it takes judgements until it is reconciled, and it
    /// stays open across a restart.
    pub fn begin_run(&self, strategy: &StrategyRef, scope: &Scope) -> Result<RunId> {
        self.acting(Actor::Engine).begin_run(strategy, scope)
    }

    /// A run's candidate pairs with both sides' features: `left_id`,
    /// `right_id`, then every feature column prefixed with `left_` and with
    /// `right_`.
    pub fn candidates(&self, run: RunId) -> Result<Vec<RecordBatch>> {
        Ok(self.candidate_rows(run)?.into_batches())
    }

    /// The same as [`candidates`](Self::candidates), with the schema, which
    /// is known even when there are no pairs.
    pub fn candidate_rows(&self, run: RunId) -> Result<Rows> {
        self.read(|conn| read_candidates(conn, run))
    }

    /// Adds judgements to an open match run and returns how many were
    /// stored. The rows need `left_id`, `right_id` and `verdict`, and may
    /// have `score` and `detail`.
    ///
    /// The whole submission is refused if a row names a record the
    /// store does not hold as this run's entity type, or a pair in which
    /// neither record is new to the run.
    pub fn submit_judgements(&self, run: RunId, rows: impl RecordBatchReader) -> Result<u64> {
        self.lock().submit_judgements(run, rows)
    }

    /// Gives up an open match run, or a running extract run. Either is
    /// recorded as failed. The records of a match run are still new to the
    /// next run. An extract run's extractor is stopped with every process it
    /// started, nothing it found is stored, and its documents are still
    /// unread; the run ends when the extractor has stopped, which may be
    /// after this returns. Once what the extractor found is being stored,
    /// it is too late, and the cancellation is refused.
    pub fn cancel_run(&self, run: RunId) -> Result<()> {
        let row = self.read(|conn| run_row(conn, run))?;
        if row.kind == "extract" {
            if row.status != RunStatus::Running {
                return Err(Error::Run {
                    run,
                    message: format!("it is {} and cannot be cancelled", row.status),
                });
            }
            if let Some(user_code) = self.user_code()
                && !user_code.running.cancel(run)
            {
                return Err(Error::Run {
                    run,
                    message: "its extractor has finished and what it found is being stored, so \
                              it is too late to cancel it"
                        .to_owned(),
                });
            }
            return Ok(());
        }
        self.lock().cancel_run(run)
    }
}

/// The candidate pairs of a run with both sides' features.
fn read_candidates(conn: &Connection, run: RunId) -> Result<Rows> {
    let (_, config) = match_run(conn, run)?;
    let table = features::table(&config.entity_type);
    let columns = feature_columns(conn, &table)?;
    let mut statement = conn.prepare(&candidates_query(&table, &columns, run, false))?;
    let arrow = statement.query_arrow([])?;
    let schema = arrow.get_schema();
    Ok(Rows::new(schema, arrow.collect()))
}

/// The feature columns of a feature table with their types, without the two
/// columns the engine keeps for itself.
fn feature_columns(conn: &Connection, table: &str) -> Result<Vec<(String, String)>> {
    let mut statement = conn.prepare(&format!("DESCRIBE {table}"))?;
    let mut rows = statement.query([])?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        if name != "record_id" && name != "declaration_version" {
            columns.push((name, row.get(1)?));
        }
    }
    Ok(columns)
}

/// The feature columns of the table aliased `alias`, as the end of a select
/// list, each named with `prefix` before its own name. In a file, where IDs
/// are text, a UUID is written as text.
fn select_features(columns: &[(String, String)], alias: &str, prefix: &str, file: bool) -> String {
    let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
    columns
        .iter()
        .map(|(name, data_type)| {
            let cast = if file && data_type == "UUID" {
                "::VARCHAR"
            } else {
                ""
            };
            format!(
                ", {alias}.{}{cast} AS {}",
                quote(name),
                quote(&format!("{prefix}{name}"))
            )
        })
        .collect()
}

/// A query for a run's candidate pairs with both sides' features: `left_id`,
/// `right_id`, then each feature column as `left_<name>` and as
/// `right_<name>`.
fn candidates_query(table: &str, columns: &[(String, String)], run: RunId, file: bool) -> String {
    let id = if file { "::VARCHAR" } else { "" };
    format!(
        "SELECT c.left_id{id} AS left_id, c.right_id{id} AS right_id{left}{right}
         FROM (SELECT DISTINCT left_id, right_id FROM spool.candidate WHERE run_id = {run}) c
         JOIN {table} l ON l.record_id = c.left_id
         JOIN {table} r ON r.record_id = c.right_id
         ORDER BY c.left_id, c.right_id",
        left = select_features(columns, "l", "left_", file),
        right = select_features(columns, "r", "right_", file),
    )
}

fn match_run(conn: &Connection, run: RunId) -> Result<(RunRow, MatchConfig)> {
    let row = run_row(conn, run)?;
    let fail = |message: &str| Error::Run {
        run,
        message: message.to_owned(),
    };
    if row.kind != "match" {
        return Err(fail("it is not a match run"));
    }
    let config = row
        .config
        .as_deref()
        .and_then(|config| serde_json::from_str(config).ok())
        .ok_or_else(|| fail("its recorded configuration cannot be read"))?;
    Ok((row, config))
}

impl Inner {
    pub(super) fn match_run(&self, run: RunId) -> Result<(RunRow, MatchConfig)> {
        match_run(&self.store.conn, run)
    }

    /// Starts a match run and generates its candidates. A run that waits for
    /// someone else's judgements becomes `open` together with its candidates;
    /// a run the engine drives stays `running` until it is concluded.
    pub(super) fn begin_match(
        &mut self,
        by: &Actor,
        strategy: &StrategyRef,
        scope: &Scope,
        then: RunStatus,
        record: Option<RecordId>,
    ) -> Result<RunId> {
        let (manifest, entry, version) = self.declared(scope.entity_type.as_str())?;
        let entity_type = entry.name.clone();
        let manifest_hash = manifest.hash.clone();
        let rules = entry.rules.clone();
        let after_run = self.last_batch_match(&entity_type)?;
        let review_items = match record {
            Some(_) => Vec::new(),
            None => review::open_ids(&self.store.conn, &entity_type)?,
        };
        let config = MatchConfig {
            entity_type: entity_type.clone(),
            declaration_version: version,
            manifest_hash,
            after_run,
            review_items,
            record,
            rematch: scope.rematch,
            recompute: false,
            rules,
        };

        let run = self.start_run("match", by, Some(strategy), &json!(config))?;
        self.in_run(run, |inner| {
            let tx = inner.store.conn.transaction()?;
            config.create_scope(&tx, run)?;
            match record {
                Some(record) => rules::generate_record_candidates(
                    &tx,
                    &entity_type,
                    run,
                    record,
                    &config.rules.block,
                )?,
                None => rules::generate_candidates(&tx, &entity_type, run, &config.rules.block)?,
            }
            if then != RunStatus::Running {
                tx.execute(
                    "UPDATE spool.run SET status = ? WHERE run_id = ?",
                    params![then.as_str(), run.0 as i64],
                )?;
            }
            tx.commit()?;
            Ok(run)
        })
    }

    /// The last successful match covering the entity type, or run 0. A
    /// single-record match does not advance this position.
    pub(super) fn last_batch_match(&self, entity_type: &EntityType) -> Result<RunId> {
        let run: i64 = self.store.conn.query_row(
            "SELECT coalesce(max(run_id), 0) FROM spool.run
             WHERE kind = 'match' AND status = 'succeeded'
               AND json_extract_string(config, '$.record') IS NULL
               AND json_extract_string(config, '$.entity_type') = ?",
            [entity_type.as_str()],
            |row| row.get(0),
        )?;
        Ok(RunId(run as u64))
    }

    /// Recreates the scope table of a match run that still takes judgements:
    /// from callers while it is `open`, from the engine's own strategy while
    /// it is `running`.
    fn judged_run(&self, run: RunId, expected: RunStatus) -> Result<MatchConfig> {
        let (row, config) = self.match_run(run)?;
        if row.status != expected {
            return Err(Error::Run {
                run,
                message: format!("it is {} and takes no judgements", row.status),
            });
        }
        config.create_scope(&self.store.conn, run)?;
        Ok(config)
    }

    fn submit_judgements(&mut self, run: RunId, rows: impl RecordBatchReader) -> Result<u64> {
        self.store_judgements(run, rows, RunStatus::Open)
    }

    /// Checks judgements and adds them to a match run that is in the state
    /// `expected`: `open` for a caller's, `running` for those of the
    /// engine's own user code.
    fn store_judgements(
        &mut self,
        run: RunId,
        rows: impl RecordBatchReader,
        expected: RunStatus,
    ) -> Result<u64> {
        let config = self.judged_run(run, expected)?;
        let entity_type = &config.entity_type;
        let schema = rows.schema();
        for required in ["left_id", "right_id", "verdict"] {
            if schema.index_of(required).is_err() {
                return Err(Error::Judgements(format!(
                    "the rows have no `{required}` column"
                )));
            }
        }
        let score = match schema.index_of("score") {
            Ok(_) => "j.score::DOUBLE",
            Err(_) => "NULL::DOUBLE",
        };
        let detail = match schema.index_of("detail") {
            Ok(_) => "j.detail::VARCHAR",
            Err(_) => "NULL::VARCHAR",
        };

        let tx = self.store.conn.transaction()?;
        let mut stored = 0;
        for incoming in rows {
            let incoming = incoming?;
            let problem: Option<(Option<String>, Option<String>, String)> = {
                let mut statement = tx.prepare(&format!(
                    "SELECT left_id, right_id, problem FROM (
                       SELECT j.left_id::VARCHAR AS left_id, j.right_id::VARCHAR AS right_id,
                         CASE
                           WHEN try_cast(j.left_id AS UUID) IS NULL OR try_cast(j.right_id AS UUID) IS NULL
                             THEN 'an ID is missing or is not a UUID'
                           WHEN try_cast(j.left_id AS UUID) = try_cast(j.right_id AS UUID)
                             THEN 'both sides are the same record'
                           WHEN j.verdict IS NULL OR j.verdict NOT IN ('match', 'no_match', 'unsure')
                             THEN 'the verdict must be match, no_match or unsure'
                           WHEN {detail} IS NOT NULL AND NOT json_valid({detail})
                             THEN 'the detail is not valid JSON'
                           WHEN l.record_id IS NULL OR r.record_id IS NULL
                             THEN 'it names a record that is not a known {entity_type}'
                           WHEN ls.record_id IS NULL AND rs.record_id IS NULL
                             THEN 'neither record is new to this run'
                         END AS problem
                       FROM arrow(?) j
                       LEFT JOIN spool.record l
                         ON l.record_id = try_cast(j.left_id AS UUID) AND l.entity_type = '{entity_type}'
                       LEFT JOIN spool.record r
                         ON r.record_id = try_cast(j.right_id AS UUID) AND r.entity_type = '{entity_type}'
                       LEFT JOIN {scope} ls ON ls.record_id = l.record_id
                       LEFT JOIN {scope} rs ON rs.record_id = r.record_id
                     ) WHERE problem IS NOT NULL LIMIT 1",
                    scope = rules::SCOPE,
                ))?;
                let mut found =
                    statement.query([&ArrowBatchRegistration::new(incoming.clone())])?;
                match found.next()? {
                    Some(row) => Some((row.get(0)?, row.get(1)?, row.get(2)?)),
                    None => None,
                }
            };
            if let Some((left, right, problem)) = problem {
                return Err(Error::Judgements(format!(
                    "the judgement between {} and {}: {problem}",
                    left.as_deref().unwrap_or("nothing"),
                    right.as_deref().unwrap_or("nothing"),
                )));
            }
            stored += tx.execute(
                &format!(
                    "INSERT INTO spool.judgement
                     SELECT ?, least(j.left_id::UUID, j.right_id::UUID),
                            greatest(j.left_id::UUID, j.right_id::UUID),
                            j.verdict, {score}, ({detail})::JSON
                     FROM arrow(?) j"
                ),
                params![run.0 as i64, &ArrowBatchRegistration::new(incoming)],
            )? as u64;
        }
        tx.commit()?;
        Ok(stored)
    }

    /// The built-in strategy: fills a match run's judgements from the
    /// project's `no_match` and `match` rules.
    pub(super) fn judge_with_rules(&mut self, run: RunId) -> Result<()> {
        let config = self.judged_run(run, RunStatus::Running)?;
        rules::judge_candidates(
            &self.store.conn,
            &config.entity_type,
            run,
            &config.rules.no_match,
            &config.rules.match_,
        )
    }

    fn cancel_run(&mut self, run: RunId) -> Result<()> {
        let (row, _) = self.match_run(run)?;
        if row.status != RunStatus::Open {
            return Err(Error::Run {
                run,
                message: format!("it is {} and cannot be cancelled", row.status),
            });
        }
        self.fail_run(run, "cancelled")
    }

    /// Writes what a user-code strategy is given: the run's candidate pairs
    /// with both sides' features, and the feature row of every record
    /// of the type, with whether it is new to the run and the identity it
    /// has.
    pub(super) fn write_task(
        &mut self,
        run: RunId,
        strategy: &Strategy,
        user_code: &UserCode,
    ) -> Result<Task> {
        let config = self.judged_run(run, RunStatus::Running)?;
        let columns = self
            .manifest
            .as_deref()
            .and_then(|manifest| manifest.entity_type(config.entity_type.as_str()))
            .map(|entry| entry.declaration.columns.clone())
            .unwrap_or_default();
        let task = Task::create(
            user_code,
            strategy,
            &TaskInfo {
                run,
                entity_type: &config.entity_type,
                declaration_version: config.declaration_version,
                rematch: config.rematch,
                features: &columns,
            },
        )?;
        let conn = &self.store.conn;
        let table = features::table(&config.entity_type);
        let columns = feature_columns(conn, &table)?;
        let copy = |query: &str, file: &str| {
            conn.execute_batch(&format!(
                "COPY (\n{query}\n) TO {} (FORMAT parquet)",
                usercode::sql_path(&task.path(file))
            ))
        };
        copy(
            &candidates_query(&table, &columns, run, true),
            usercode::CANDIDATES,
        )?;
        copy(
            &format!(
                "SELECT f.record_id::VARCHAR AS record_id,
                        f.record_id IN (SELECT record_id FROM {scope}) AS is_new,
                        m.identity_id::VARCHAR AS identity_id{features}
                 FROM {table} f
                 LEFT JOIN spool.membership m
                   ON m.record_id = f.record_id AND m.to_seq IS NULL
                 ORDER BY f.record_id",
                scope = rules::SCOPE,
                features = select_features(&columns, "f", "", true),
            ),
            usercode::FEATURES,
        )?;
        Ok(task)
    }

    /// Reads the judgements a user-code strategy wrote and stores them,
    /// under the same checks as judgements a caller submits.
    pub(super) fn take_judgements(&mut self, run: RunId, task: &Task) -> Result<u64> {
        let rows = {
            let mut statement = self.store.conn.prepare(&format!(
                "SELECT * FROM read_parquet({})",
                usercode::sql_path(&task.path(usercode::JUDGEMENTS))
            ))?;
            let arrow = statement.query_arrow([])?;
            let schema = arrow.get_schema();
            Rows::new(schema, arrow.collect())
        };
        self.store_judgements(run, rows, RunStatus::Running)
    }
}
