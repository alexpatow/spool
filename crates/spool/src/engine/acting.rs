//! The engine in someone's name: who a run is recorded as started by.

use duckdb::arrow::record_batch::RecordBatchReader;
use duckdb::params;
use spool_core::{Actor, Draft, EntityType, Policy, RunId};

use super::extract::choose_extractor;
use super::resolve::Chosen;
use super::{
    AddRecordReport, Disposition, Engine, ResolveOptions, ResolveReport, RunStatus, Scope,
    StrategyRef, TypeResolution, lock,
};
use crate::documents::{
    AddDocumentReport, DocumentRecord, ExtractOptions, ExtractReport, NewDocument,
};
use crate::error::{Error, Result};
use crate::load::LoadReport;
use crate::recompute::RecomputeApplied;
use crate::rows::Rows;
use crate::store::parse;

/// The engine as one caller of it. Each run started through this is
/// recorded as started by that caller, and does what the method of the same
/// name on [`Engine`] does.
///
/// Who started a run is not who authored what it changed. A changeset says
/// who decided its operations, and those of a match run are the engine's.
pub struct Acting<'a> {
    engine: &'a Engine,
    actor: Actor,
}

impl Engine {
    /// The engine as `actor`: the runs started through what this returns
    /// are recorded as started by them.
    ///
    /// A run started on the engine itself is recorded as the engine's own.
    /// That is right where nobody asked for it, as in a benchmark. A server
    /// names its caller.
    pub fn acting(&self, actor: Actor) -> Acting<'_> {
        Acting {
            engine: self,
            actor,
        }
    }
}

impl Acting<'_> {
    /// As [`Engine::load`].
    pub fn load(
        &self,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
    ) -> Result<LoadReport> {
        self.engine
            .lock()
            .load(&self.actor, source, entity_type, rows, true)
    }

    /// As [`Engine::load_without_replacing`].
    pub fn load_without_replacing(
        &self,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
    ) -> Result<LoadReport> {
        self.engine
            .lock()
            .load(&self.actor, source, entity_type, rows, false)
    }

    /// As [`Engine::add_record`].
    pub fn add_record(
        &self,
        source: &str,
        entity_type: &EntityType,
        row: &serde_json::Map<String, serde_json::Value>,
        replace: bool,
    ) -> Result<AddRecordReport> {
        let rows = Rows::from_json(std::slice::from_ref(row))?;
        let _turn = lock(&self.engine.resolving);
        self.engine
            .lock()
            .add_record(&self.actor, source, entity_type, rows, replace)
    }

    /// As [`Engine::add_documents`].
    pub fn add_documents(&self, source: &str, documents: &[NewDocument]) -> Result<LoadReport> {
        self.engine
            .lock()
            .add_documents(&self.actor, source, documents)
    }

    /// As [`Engine::add_document`].
    pub fn add_document(
        &self,
        source: &str,
        document: &NewDocument,
        extractor: Option<&str>,
    ) -> Result<AddDocumentReport> {
        let _turn = lock(&self.engine.resolving);
        let mut inner = self.engine.lock();
        let manifest = inner.manifest.as_deref().ok_or(Error::NoManifest)?;
        if manifest.source(source).is_none() {
            return Err(Error::Documents(format!(
                "source `{source}` is not declared: the project has no sources/{source}.yaml"
            )));
        }
        let extractor = choose_extractor(manifest, extractor)?;
        inner.add_documents(&self.actor, source, std::slice::from_ref(document))?;
        let document = inner.store.conn.query_row(
            "SELECT d.document_id::VARCHAR FROM spool.document d
             WHERE d.source_id = ? AND d.source_key = ? AND NOT EXISTS (
                 SELECT 1 FROM spool.document later WHERE later.supersedes = d.document_id)",
            params![source, document.key],
            |row| row.get::<_, String>(0),
        )?;
        let document = parse(&document)?;
        let mentions = match inner.document_reading(document, &extractor)? {
            Some(mentions) => mentions,
            None => {
                let options = ExtractOptions::with(&extractor.name);
                let report;
                (inner, report) = self.engine.extract_locked(
                    inner,
                    &self.actor,
                    &options,
                    &extractor,
                    Some(document),
                )?;
                if let Some(failed) = report.failed.first() {
                    return Err(Error::Extractor {
                        extractor: extractor.name,
                        message: format!(
                            "could not read document `{}`: {}",
                            failed.key, failed.reason
                        ),
                    });
                }
                inner.document_mentions(document, report.run)?
            }
        };
        let mut records = Vec::with_capacity(mentions.len());
        for (record, entity_type, start, end) in mentions {
            let placed = inner.place_record(&self.actor, &entity_type, record)?;
            records.push(DocumentRecord {
                record,
                entity_type,
                start,
                end,
                identity: placed.identity,
                review: placed.review,
            });
        }
        Ok(AddDocumentReport { document, records })
    }

    /// As [`Engine::extract`]. The load run that stores what the extractor
    /// found is started by the same caller.
    pub fn extract(&self, options: &ExtractOptions) -> Result<ExtractReport> {
        let _turn = lock(&self.engine.resolving);
        let inner = self.engine.lock();
        let manifest = inner.manifest.as_deref().ok_or(Error::NoManifest)?;
        let extractor = choose_extractor(manifest, Some(&options.extractor))?;
        let (_inner, report) =
            self.engine
                .extract_locked(inner, &self.actor, options, &extractor, None)?;
        Ok(report)
    }

    /// As [`Engine::begin_run`].
    pub fn begin_run(&self, strategy: &StrategyRef, scope: &Scope) -> Result<RunId> {
        self.engine
            .lock()
            .begin_match(&self.actor, strategy, scope, RunStatus::Open, None)
    }

    /// As [`Engine::resolve`]. The reconcile run of each match run is
    /// started by the same caller.
    pub fn resolve(&self, options: &ResolveOptions) -> Result<ResolveReport> {
        let _turn = lock(&self.engine.resolving);
        let plan = self.engine.lock().plan(options)?;
        // Before anything is started: user code needs the project's files,
        // and a file that is not the one the manifest was compiled from is
        // never run.
        let user_code = self.engine.user_code();
        for (_, chosen) in &plan {
            if let Chosen::UserCode(strategy) = chosen {
                user_code
                    .as_deref()
                    .ok_or_else(|| Error::NoUserCode(strategy.name.clone()))?
                    .check(&strategy.into())?;
            }
        }
        let then = if options.leave_for_review {
            Disposition::LeaveForReview
        } else {
            Disposition::Commit
        };

        let mut report = ResolveReport::default();
        for (entity_type, chosen) in plan {
            let mut inner = self.engine.lock();
            if !options.rematch && !inner.has_work(&entity_type)? {
                continue;
            }
            let scope = Scope {
                entity_type,
                rematch: options.rematch,
            };
            let resolution = match chosen {
                Chosen::Rules => {
                    let run = inner.begin_match(
                        &self.actor,
                        &StrategyRef::rules(),
                        &scope,
                        RunStatus::Running,
                        None,
                    )?;
                    inner.in_run(run, |inner| {
                        inner.judge_with_rules(run)?;
                        inner.conclude(&self.actor, run, &Policy::default(), then)
                    })?
                }
                Chosen::UserCode(strategy) => {
                    let user_code = user_code
                        .clone()
                        .ok_or_else(|| Error::NoUserCode(strategy.name.clone()))?;
                    let reference = StrategyRef {
                        name: strategy.name.clone(),
                        version: Some(strategy.hash.clone()),
                    };
                    let run = inner.begin_match(
                        &self.actor,
                        &reference,
                        &scope,
                        RunStatus::Running,
                        None,
                    )?;
                    let mut task =
                        inner.in_run(run, |inner| inner.write_task(run, &strategy, &user_code))?;
                    // The user code runs with the writer let go. The run
                    // stays `running`, and what it was given stays as it
                    // was written, whatever is committed meanwhile.
                    drop(inner);
                    let executed = task.execute();
                    let mut inner = self.engine.lock();
                    let resolution = inner.in_run(run, |inner| {
                        executed?;
                        inner
                            .take_judgements(run, &task)
                            .map_err(|error| task.rejected(error))?;
                        inner.conclude(&self.actor, run, &Policy::default(), then)
                    })?;
                    task.remove();
                    resolution
                }
            };
            report.resolved.push(resolution.1);
        }
        Ok(report)
    }

    /// As [`Engine::reconcile`]. The caller starts the reconcile run,
    /// whoever started the match run.
    pub fn reconcile(&self, run: RunId, policy: &Policy) -> Result<Draft> {
        let (draft, _) =
            self.engine
                .lock()
                .reconcile(&self.actor, run, policy, Disposition::LeaveForReview)?;
        Ok(draft)
    }

    /// As [`Engine::reconcile_with`].
    pub fn reconcile_with(
        &self,
        run: RunId,
        policy: &Policy,
        then: Disposition,
    ) -> Result<TypeResolution> {
        let (_, report) = self
            .engine
            .lock()
            .reconcile(&self.actor, run, policy, then)?;
        Ok(report)
    }

    /// As [`Engine::apply_recompute`]. The runs are started by the caller;
    /// the changeset is the engine's, because the rules decided it.
    pub fn apply_recompute(
        &self,
        entity_type: Option<&EntityType>,
        plan: &str,
        reason: Option<String>,
    ) -> Result<RecomputeApplied> {
        let _turn = lock(&self.engine.resolving);
        self.engine
            .lock()
            .apply_recompute(&self.actor, entity_type, plan, reason)
    }
}
