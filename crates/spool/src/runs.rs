//! Runs as records: what a caller reads to learn how a run is doing.
//!
//! `spool.run` is the task record. A call that starts work is given the run's
//! ID, and reads the run until its status says the engine has stopped working
//! on it.

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{Committed, ContentHash, EntityType, RunId, Seq, StateError};

use crate::engine::{StrategyRef, TypeResolution};
use crate::error::{Error, Result};
use crate::ingest::IngestReport;
use crate::store::parse;
use crate::time::rfc3339;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Ingest,
    Match,
    Reconcile,
}

impl RunKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            RunKind::Ingest => "ingest",
            RunKind::Match => "match",
            RunKind::Reconcile => "reconcile",
        }
    }
}

/// How far a run has got.
///
/// Only `running` means the engine is at work. An `open` or `drafted` match
/// run waits for someone else, and stays as it is across a restart. A run
/// found `running` when a store is opened was interrupted, and is recorded as
/// `failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// The engine is working on it.
    Running,
    /// A match run whose candidates are ready. It takes judgements until it
    /// is reconciled.
    Open,
    /// A match run that was reconciled into a draft that has not been
    /// committed. The draft's own status says what became of it.
    Drafted,
    /// Finished. For a match run: its draft was committed, or there was
    /// nothing to commit.
    Succeeded,
    /// It stopped with an error, was interrupted, or was cancelled.
    Failed,
}

impl RunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Open => "open",
            RunStatus::Drafted => "drafted",
            RunStatus::Succeeded => "succeeded",
            RunStatus::Failed => "failed",
        }
    }

    pub(crate) fn from_stored(text: &str) -> Result<Self> {
        Ok(match text {
            "running" => RunStatus::Running,
            "open" => RunStatus::Open,
            "drafted" => RunStatus::Drafted,
            "succeeded" => RunStatus::Succeeded,
            "failed" => RunStatus::Failed,
            other => return Err(StateError::new(format!("unknown run status {other:?}")).into()),
        })
    }
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A run as a caller sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RunRecord {
    pub run: RunId,
    pub kind: RunKind,
    pub status: RunStatus,
    /// The entity type an ingest or a match run is about.
    pub entity_type: Option<EntityType>,
    /// The strategy of a match run.
    pub strategy: Option<StrategyRef>,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Why a failed run failed.
    pub error: Option<String>,
    /// What an ingest run reported when it finished.
    pub ingest: Option<IngestReport>,
    /// What a match run reported when it was reconciled. `committed` is
    /// filled in once its draft is committed.
    pub resolution: Option<TypeResolution>,
}

/// Which runs to list. The default lists the latest runs of every kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunFilter {
    pub kind: Option<RunKind>,
    pub status: Option<RunStatus>,
    pub entity_type: Option<EntityType>,
    /// At most this many, newest first.
    pub limit: u64,
}

impl Default for RunFilter {
    fn default() -> Self {
        Self {
            kind: None,
            status: None,
            entity_type: None,
            limit: 100,
        }
    }
}

const RECORD: &str = "
    SELECT r.run_id, r.kind, r.status, r.strategy, r.strategy_version,
           json_extract_string(r.config, '$.entity_type'),
           epoch_us(r.started_at), epoch_us(r.finished_at), r.error, r.result::VARCHAR,
           c.seq, c.changeset_id::VARCHAR, c.content_hash
    FROM spool.run r
    LEFT JOIN spool.draft d ON d.match_run = r.run_id AND d.status = 'committed'
    LEFT JOIN spool.changeset c ON c.seq = d.closed_seq";

fn record(row: &duckdb::Row<'_>) -> Result<RunRecord> {
    let run = RunId(row.get::<_, i64>(0)? as u64);
    let unreadable = |what: &str| Error::Run {
        run,
        message: format!("its recorded {what} cannot be read"),
    };
    let kind = match row.get::<_, String>(1)?.as_str() {
        "ingest" => RunKind::Ingest,
        "match" => RunKind::Match,
        "reconcile" => RunKind::Reconcile,
        _ => return Err(unreadable("kind")),
    };
    let result: Option<String> = row.get(9)?;
    let (mut ingest, mut resolution) = (None, None);
    if let Some(result) = &result {
        match kind {
            RunKind::Ingest => {
                ingest = Some(serde_json::from_str(result).map_err(|_| unreadable("result"))?);
            }
            RunKind::Match => {
                let mut report: TypeResolution =
                    serde_json::from_str(result).map_err(|_| unreadable("result"))?;
                // The report was written when the run was reconciled. If its
                // draft was committed later, the log knows.
                if report.committed.is_none()
                    && let Some(seq) = row.get::<_, Option<i64>>(10)?
                {
                    report.committed = Some(Committed {
                        seq: Seq(seq as u64),
                        changeset: parse(&row.get::<_, String>(11)?)?,
                        content_hash: ContentHash::from_stored(row.get(12)?),
                    });
                }
                resolution = Some(report);
            }
            RunKind::Reconcile => {}
        }
    }
    Ok(RunRecord {
        run,
        kind,
        status: RunStatus::from_stored(&row.get::<_, String>(2)?)?,
        strategy: row.get::<_, Option<String>>(3)?.map(|name| StrategyRef {
            name,
            version: row.get(4).unwrap_or(None),
        }),
        entity_type: match row.get::<_, Option<String>>(5)? {
            Some(name) => Some(EntityType::new(name).map_err(StateError::new)?),
            None => None,
        },
        started_at: rfc3339(row.get(6)?),
        finished_at: row.get::<_, Option<i64>>(7)?.map(rfc3339),
        error: row.get(8)?,
        ingest,
        resolution,
    })
}

/// One run, or `None` if there is no run by that ID.
pub(crate) fn get(conn: &Connection, run: RunId) -> Result<Option<RunRecord>> {
    let mut statement = conn.prepare(&format!("{RECORD} WHERE r.run_id = ?"))?;
    let mut rows = statement.query([run.0 as i64])?;
    match rows.next()? {
        Some(row) => Ok(Some(record(row)?)),
        None => Ok(None),
    }
}

/// Runs, newest first.
pub(crate) fn list(conn: &Connection, filter: &RunFilter) -> Result<Vec<RunRecord>> {
    let mut statement = conn.prepare(&format!(
        "{RECORD}
         WHERE ($1::VARCHAR IS NULL OR r.kind = $1::VARCHAR)
           AND ($2::VARCHAR IS NULL OR r.status = $2::VARCHAR)
           AND ($3::VARCHAR IS NULL
                OR json_extract_string(r.config, '$.entity_type') = $3::VARCHAR)
         ORDER BY r.run_id DESC
         LIMIT $4"
    ))?;
    let mut rows = statement.query(params![
        filter.kind.map(RunKind::as_str),
        filter.status.map(RunStatus::as_str),
        filter.entity_type.as_ref().map(EntityType::as_str),
        filter.limit.min(i64::MAX as u64) as i64,
    ])?;
    let mut runs = Vec::new();
    while let Some(row) = rows.next()? {
        runs.push(record(row)?);
    }
    Ok(runs)
}
