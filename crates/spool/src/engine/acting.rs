//! The engine in someone's name: who a run is recorded as started by.

use duckdb::arrow::record_batch::RecordBatchReader;
use spool_core::{Actor, Draft, Policy, RunId};

use super::{
    AddRecordReport, Disposition, Engine, ResolveOptions, ResolveReport, RunStatus, Scope,
    StrategyRef, TypeResolution,
};
use crate::documents::{ExtractOptions, ExtractReport, NewDocument};
use crate::error::Result;
use crate::load::LoadReport;
use crate::recompute::RecomputeApplied;
use spool_core::EntityType;

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
        self.engine
            .add_record_by(&self.actor, source, entity_type, row, replace)
    }

    /// As [`Engine::add_documents`].
    pub fn add_documents(&self, source: &str, documents: &[NewDocument]) -> Result<LoadReport> {
        self.engine
            .lock()
            .add_documents(&self.actor, source, documents)
    }

    /// As [`Engine::extract`]. The load run that stores what the extractor
    /// found is started by the same caller.
    pub fn extract(&self, options: &ExtractOptions) -> Result<ExtractReport> {
        self.engine.extract_by(&self.actor, options)
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
        self.engine.resolve_by(&self.actor, options)
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
        self.engine
            .apply_recompute_by(&self.actor, entity_type, plan, reason)
    }
}
