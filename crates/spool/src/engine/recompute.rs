//! Recomputing: what the identities would be if every record were
//! matched again by the rules as they are now, and making them that.

use duckdb::params;
use serde_json::json;
use spool_core::{
    Actor, Changeset, EntityType, IdentityState, IdentityStatus, IdentityStore, Policy, validate,
};

use super::matching::MatchConfig;
use super::{Engine, Inner, StrategyRef, lock_ignoring_poison};
use crate::drafts;
use crate::error::{Error, Result};
use crate::possible_split;
use crate::project::{EntityTypeManifest, Manifest};
use crate::recompute::{self, AppliedType, RecomputeApplied, RecomputeReport, RecomputeRequest};
use crate::retract;
use crate::review;
use crate::rules;
use crate::schema;
use crate::store;

impl Engine {
    /// What the identities of a type, or of every type, would be if every
    /// record were matched again, all at once, by the rules as they are
    /// now, against the identities there are. Nothing is changed or
    /// recorded to find out, and writes do not wait for it.
    ///
    /// See [`apply_recompute`](Self::apply_recompute) for applying it.
    pub fn recompute(&self, request: &RecomputeRequest) -> Result<RecomputeReport> {
        let manifest = self.manifest().ok_or(Error::NoManifest)?;
        self.read(|conn| {
            let applied = schema::meta(conn, "manifest_hash")?;
            if applied.as_deref() != Some(manifest.hash.as_str()) {
                return Err(Error::ManifestMismatch {
                    applied,
                    manifest: manifest.hash.clone(),
                });
            }
            let detail_of = match request.identity {
                Some(id) => Some(match store::read_identities(conn, &[id])?.remove(0) {
                    None => return Err(Error::UnknownIdentity(id)),
                    Some(entry) => match entry.status {
                        IdentityStatus::Active => id,
                        IdentityStatus::MergedInto(survivor) => survivor,
                    },
                }),
                None => None,
            };
            let entries = recomputed_types(&manifest, request.entity_type.as_ref())?;
            let mut plans = Vec::new();
            let mut types = Vec::new();
            for entry in entries {
                let judgements = recompute::judge_for_preview(conn, &entry.name, &entry.rules)?;
                let outcome = recompute::compute(conn, &entry.name, &judgements)?;
                plans.push(outcome.plan());
                types.push(outcome.report(conn, request, detail_of)?);
            }
            Ok(RecomputeReport {
                plan: recompute::combined_plan(&plans),
                types,
            })
        })
    }

    /// Makes the identities of a type, or of every type, what
    /// [`recompute`](Self::recompute) showed under `plan`. It works the
    /// result out again, and applies nothing unless that is the plan: if
    /// the records, the rules or the identities changed in a way that
    /// changes the result, it is refused with [`Error::StalePlan`].
    ///
    /// For each type, a match run judges every record by the rules and its
    /// judgements are kept as any run's are, and a reconcile run commits
    /// one changeset by the engine, which validates like any other. The
    /// records that wait for review are left in `conflict` items, and an
    /// open draft of the type is superseded.
    pub fn apply_recompute(
        &self,
        entity_type: Option<&EntityType>,
        plan: &str,
        reason: Option<String>,
    ) -> Result<RecomputeApplied> {
        self.acting(Actor::Engine)
            .apply_recompute(entity_type, plan, reason)
    }
}

/// The manifest's entry for the type a recomputation is asked about, or
/// for every type it declares.
fn recomputed_types<'a>(
    manifest: &'a Manifest,
    entity_type: Option<&EntityType>,
) -> Result<Vec<&'a EntityTypeManifest>> {
    match entity_type {
        Some(name) => Ok(vec![
            manifest
                .entity_type(name.as_str())
                .ok_or_else(|| Error::UndeclaredType(name.to_string()))?,
        ]),
        None => Ok(manifest.entity_types.iter().collect()),
    }
}

impl Inner {
    #[expect(clippy::too_many_lines)]
    pub(super) fn apply_recompute(
        &mut self,
        by: &Actor,
        entity_type: Option<&EntityType>,
        plan: &str,
        reason: Option<String>,
    ) -> Result<RecomputeApplied> {
        let manifest = self.manifest.clone().ok_or(Error::NoManifest)?;
        let names: Vec<EntityType> = recomputed_types(&manifest, entity_type)?
            .into_iter()
            .map(|entry| entry.name.clone())
            .collect();
        let mut worked = Vec::new();
        for name in &names {
            let (manifest, entry, version) = self.declared(name.as_str())?;
            let config = MatchConfig {
                entity_type: name.clone(),
                declaration_version: version,
                manifest_hash: manifest.hash.clone(),
                after_run: self.last_batch_match(name)?,
                review_items: review::open_ids(&self.store.conn, name)?,
                record: None,
                rematch: true,
                recompute: true,
                rules: entry.rules.clone(),
            };
            let run = self.start_run("match", by, Some(&StrategyRef::rules()), &json!(config))?;
            let outcome = self.in_run(run, |inner| {
                let tx = inner.store.conn.transaction()?;
                recompute::create_scope(&tx, name, run)?;
                rules::generate_candidates(&tx, name, run, &config.rules.block)?;
                rules::judge_candidates(
                    &tx,
                    name,
                    run,
                    &config.rules.no_match,
                    &config.rules.match_,
                )?;
                tx.commit()?;
                recompute::compute(
                    &inner.store.conn,
                    name,
                    &format!("(SELECT * FROM spool.judgement WHERE run_id = {run})"),
                )
            })?;
            worked.push((run, outcome));
        }
        let plans: Vec<String> = worked.iter().map(|(_, outcome)| outcome.plan()).collect();
        let now = recompute::combined_plan(&plans);
        if now != plan {
            for (run, _) in &worked {
                self.fail_run(
                    *run,
                    &format!("plan {plan} is not what it found, plan {now}"),
                )?;
            }
            return Err(Error::StalePlan {
                plan: plan.to_owned(),
                now,
            });
        }

        let mut applied = RecomputeApplied {
            plan: now.clone(),
            types: Vec::new(),
        };
        for (run, outcome) in worked {
            let entity_type = outcome.entity_type().clone();
            let reconcile_run = self.start_run(
                "reconcile",
                by,
                None,
                &json!({ "match_run": run, "policy": Policy::default(), "recompute": now }),
            )?;
            let report = self.in_run(reconcile_run, |inner| {
                let (ops, identities) = {
                    let mut ids = lock_ignoring_poison(&inner.ids);
                    outcome.operations(recompute::mint(ids.as_mut()))
                };
                let changeset = Changeset {
                    id: lock_ignoring_poison(&inner.ids).changeset_id(),
                    base: inner.store.head()?,
                    actor: Actor::Engine,
                    reason: Some(match &reason {
                        Some(reason) => format!("recompute (plan {now}): {reason}"),
                        None => format!("recompute under the current rules (plan {now})"),
                    }),
                    run: Some(reconcile_run),
                    ops,
                };
                let review = outcome.review(&identities);
                let summary = outcome.summary();
                let finished = inner.store.clock.now_micros();
                let tx = inner.store.conn.transaction()?;
                review::record(&tx, &entity_type, reconcile_run, &review)?;
                drafts::supersede(&tx, &entity_type, reconcile_run)?;
                tx.execute(
                    "UPDATE spool.run SET status = 'succeeded', finished_at = make_timestamptz(?)
                     WHERE run_id = ?",
                    params![finished, reconcile_run.0 as i64],
                )?;
                tx.commit()?;
                let committed = match changeset.ops.is_empty() {
                    true => None,
                    false => {
                        retract::refuse_placing(&inner.store.conn, &changeset.ops)?;
                        let validated = validate(&inner.store, changeset)?;
                        Some(inner.store.apply(validated)?)
                    }
                };
                Ok(AppliedType {
                    entity_type: entity_type.clone(),
                    match_run: run,
                    reconcile_run,
                    summary,
                    committed,
                    review,
                })
            });
            let report = match report {
                Ok(report) => report,
                Err(error) => {
                    self.fail_run(run, &error.to_string())?;
                    return Err(error);
                }
            };
            let finished = self.store.clock.now_micros();
            let tx = self.store.conn.transaction()?;
            tx.execute(
                "UPDATE spool.run
                 SET status = 'succeeded', finished_at = make_timestamptz(?), result = ?::JSON
                 WHERE run_id = ?",
                params![finished, json!(report).to_string(), run.0 as i64],
            )?;
            possible_split::supersede_answered(&tx, &entity_type, reconcile_run, run)?;
            tx.commit()?;
            applied.types.push(report);
        }
        Ok(applied)
    }
}
