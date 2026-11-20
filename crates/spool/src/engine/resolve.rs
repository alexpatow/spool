//! Resolving: each type's new records are matched with a strategy and
//! reconciled, and the draft is committed or left for review.

use serde::{Deserialize, Serialize};
use spool_core::{Actor, EntityType, Policy};

use super::{Disposition, Engine, Inner, Scope, StrategyRef, TypeResolution, lock};
use crate::error::{Error, Result};
use crate::project::{EntityTypeManifest, Strategy};
use crate::retract;
use crate::review;
use crate::rules;
use crate::runs::RunStatus;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolveOptions {
    /// Resolve only this entity type. `None` resolves every declared type.
    pub entity_type: Option<EntityType>,
    /// Store each type's draft for review instead of committing it.
    pub leave_for_review: bool,
    /// The strategy that judges: `rules`, or the name of one of the
    /// project's user-code strategies. `None` is `rules`. Every type that is
    /// resolved must have it.
    pub strategy: Option<String>,
    /// Match every record again, not only the new ones, which is what
    /// a new strategy or a changed rule calls for.
    ///
    /// A rematch changes nothing about a record that has an identity:
    /// judgements never merge or split existing identities. Where the
    /// strategy disagrees with the identities as they are, the run reports
    /// it as [`Suggestions`](super::Suggestions). Records without an identity are placed
    /// as in any run.
    pub rematch: bool,
}

impl ResolveOptions {
    /// Resolves with the named strategy.
    pub fn with_strategy(strategy: impl Into<String>) -> Self {
        Self {
            strategy: Some(strategy.into()),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ResolveReport {
    /// One entry per entity type that had new records.
    pub resolved: Vec<TypeResolution>,
}

/// The strategy a resolve runs for an entity type.
enum Chosen {
    Rules,
    UserCode(Strategy),
}

impl Engine {
    /// Matches new records with a strategy, reconciles with the default
    /// policy, and commits or leaves a draft for review. A type is resolved
    /// when it has newly loaded records or open review items, and
    /// always by a [rematch](ResolveOptions::rematch).
    ///
    /// The strategy is the built-in runner of the project's SQL rules, or
    /// user code the project holds, which runs as a process of its own.
    /// While it runs the engine is free: changesets are committed, rows are
    /// loaded and reads are answered. What the user code hands back is
    /// evidence, reconciled against the state there is when it finishes. A
    /// strategy that fails, or does not finish in time, fails its run and
    /// changes nothing.
    ///
    /// The match run is recorded as succeeded only after the commit. A
    /// resolve that is cut short leaves the run unfinished, and the next
    /// resolve examines the same records again.
    pub fn resolve(&self, options: &ResolveOptions) -> Result<ResolveReport> {
        self.resolve_by(&Actor::Engine, options)
    }

    pub(super) fn resolve_by(&self, by: &Actor, options: &ResolveOptions) -> Result<ResolveReport> {
        let _turn = lock(&self.resolving);
        let plan = self.lock().plan(options)?;
        // Before anything is started: user code needs the project's files,
        // and a file that is not the one the manifest was compiled from is
        // never run.
        let user_code = self.user_code();
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
            let mut inner = self.lock();
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
                        by,
                        &StrategyRef::rules(),
                        &scope,
                        RunStatus::Running,
                        None,
                    )?;
                    inner.in_run(run, |inner| {
                        inner.judge_with_rules(run)?;
                        inner.conclude(by, run, &Policy::default(), then)
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
                    let run =
                        inner.begin_match(by, &reference, &scope, RunStatus::Running, None)?;
                    let mut task =
                        inner.in_run(run, |inner| inner.write_task(run, &strategy, &user_code))?;
                    // The user code runs with the writer let go. The run
                    // stays `running`, and what it was given stays as it
                    // was written, whatever is committed meanwhile.
                    drop(inner);
                    let executed = task.execute();
                    let mut inner = self.lock();
                    let resolution = inner.in_run(run, |inner| {
                        executed?;
                        inner
                            .take_judgements(run, &task)
                            .map_err(|error| task.rejected(error))?;
                        inner.conclude(by, run, &Policy::default(), then)
                    })?;
                    task.remove();
                    resolution
                }
            };
            report.resolved.push(resolution.1);
        }
        Ok(report)
    }
}

impl Inner {
    /// The entity types a resolve covers, each with the strategy it runs.
    /// Nothing is started if a type lacks the strategy that was named.
    fn plan(&self, options: &ResolveOptions) -> Result<Vec<(EntityType, Chosen)>> {
        let manifest = self.manifest.as_deref().ok_or(Error::NoManifest)?;
        let entries: Vec<&EntityTypeManifest> = match &options.entity_type {
            Some(entity_type) => vec![self.declared(entity_type.as_str())?.1],
            None => manifest.entity_types.iter().collect(),
        };
        entries
            .into_iter()
            .map(|entry| {
                let chosen = match options.strategy.as_deref() {
                    None | Some(StrategyRef::RULES) => Chosen::Rules,
                    Some(name) => match entry.strategy(name) {
                        Some(strategy) => Chosen::UserCode(strategy.clone()),
                        None => {
                            return Err(Error::UnknownStrategy {
                                entity_type: entry.name.to_string(),
                                strategy: name.to_owned(),
                                known: entry
                                    .strategies
                                    .iter()
                                    .map(|strategy| strategy.name.clone())
                                    .collect(),
                            });
                        }
                    },
                };
                Ok((entry.name.clone(), chosen))
            })
            .collect()
    }

    /// Whether a type has anything a run would examine: records
    /// loaded since its last match run that succeeded, records that
    /// came back since then after their source had taken them back, or open
    /// review items with records that wait for an identity.
    fn has_work(&self, entity_type: &EntityType) -> Result<bool> {
        let after_run = self.last_batch_match(entity_type)?;
        let loaded = self.count(&format!(
            "SELECT count(*) FROM spool.record
             WHERE entity_type = '{entity_type}'
               AND ((load_run > {after_run} {unmatched}) OR (
                 record_id IN (
                   SELECT record_id FROM spool.retraction WHERE restored_run > {after_run})
                 AND record_id NOT IN (
                   SELECT record_id FROM spool.membership WHERE to_seq IS NULL)))
               AND {} AND {}",
            rules::NOT_DISMISSED,
            retract::NOT_RETRACTED,
            unmatched = rules::not_matched_individually(after_run, None),
        ))?;
        Ok(loaded > 0 || !review::open_ids(&self.store.conn, entity_type)?.is_empty())
    }
}
