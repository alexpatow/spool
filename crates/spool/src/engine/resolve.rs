//! Resolving: each type's new records are matched with a strategy and
//! reconciled, and the draft is committed or left for review.

use serde::{Deserialize, Serialize};
use spool_core::{Actor, EntityType};

use super::{Engine, Inner, StrategyRef, TypeResolution};
use crate::error::{Error, Result};
use crate::project::{EntityTypeManifest, Strategy};
use crate::retract;
use crate::review;
use crate::rules;

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
pub(super) enum Chosen {
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
        self.acting(Actor::Engine).resolve(options)
    }
}

impl Inner {
    /// The entity types a resolve covers, each with the strategy it runs.
    /// Nothing is started if a type lacks the strategy that was named.
    pub(super) fn plan(&self, options: &ResolveOptions) -> Result<Vec<(EntityType, Chosen)>> {
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
    pub(super) fn has_work(&self, entity_type: &EntityType) -> Result<bool> {
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
