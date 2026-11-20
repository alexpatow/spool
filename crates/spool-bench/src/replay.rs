//! The steps of a scenario run again in a store of their own, to compare
//! with: without the rows that were taken back, or in another order.

use std::collections::{HashMap, HashSet};

use spool::arrow::array::AsArray;
use spool::core::Actor;
use spool::{Engine, Manifest, NOT_RETRACTED, Options, ResolveOptions};

use crate::error::{Error, Result};
use crate::metrics::{OrderSensitivity, Rebuild};
use crate::scenario::Scenario;
use crate::score::{asked, grouping, truth_relation};
use crate::spec::StepSpec;
use crate::truth::Truth;

/// The rows the sources have taken back as things stand: their keys, by
/// entity type and source.
pub(crate) type Gone = HashMap<(String, String), HashSet<String>>;

fn gone(engine: &Engine) -> Result<Gone> {
    let batches = engine.query(&format!(
        "SELECT DISTINCT entity_type, source_id, source_key FROM spool.record
         WHERE source_key IS NOT NULL AND NOT ({NOT_RETRACTED})"
    ))?;
    let mut gone = Gone::new();
    for batch in &batches {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (types, sources, keys) = (column(0), column(1), column(2));
        for row in 0..batch.num_rows() {
            gone.entry((types.value(row).to_owned(), sources.value(row).to_owned()))
                .or_default()
                .insert(keys.value(row).to_owned());
        }
    }
    Ok(gone)
}

impl Scenario {
    /// Runs steps in a store of their own, without the rows in `without`
    /// and without corrections: each batch with the strategy it was written
    /// with, and no rematch. The scenario's own engine says how user code
    /// is run.
    ///
    /// The mentions in `without` are extracted here as they were there, and
    /// are taken back before the step is resolved, so they are records of
    /// this store that no run examines.
    fn replay<'a>(
        &self,
        engine: &Engine,
        manifest: &Manifest,
        steps: impl Iterator<Item = &'a StepSpec>,
        without: &Gone,
    ) -> Result<(Engine, Truth)> {
        let replayed = Engine::open_in_memory(Options::seeded(0))?;
        replayed.apply(manifest)?;
        if let Some(user_code) = engine.user_code() {
            replayed.set_user_code(user_code.as_ref().clone());
        }
        let mut truth = self.truth()?;
        // In an order that does not change from one run to the next.
        let mut gone: Vec<_> = without.iter().collect();
        gone.sort_by_key(|(named, _)| *named);
        for step in steps {
            self.feed(&replayed, step, &mut truth, without)?;
            // A row of a file is left out before it is ingested. A mention
            // is what an extractor finds in a document, and cannot be left
            // out, so it is taken back before anything is matched with it:
            // it gets no identity, and counts for nothing, as a row that
            // was never sent.
            if !step.extract.is_empty() {
                for ((entity_type, source), keys) in &gone {
                    let mut keys: Vec<String> = keys.iter().cloned().collect();
                    keys.sort();
                    replayed.retract(
                        source,
                        entity_type,
                        &keys,
                        Actor::Agent("benchmark".to_owned()),
                        None,
                    )?;
                }
            }
            replayed.resolve(&ResolveOptions::with_strategy(self.strategy(step)))?;
        }
        Ok((replayed, truth))
    }

    /// Compares what `engine` holds after `steps` with the same steps run
    /// in a store that is never sent the rows the sources have taken back.
    pub(crate) fn rebuild(
        &self,
        engine: &Engine,
        manifest: &Manifest,
        steps: &[StepSpec],
    ) -> Result<Rebuild> {
        let gone = gone(engine)?;
        let batches = steps.iter().filter(|step| step.feeds());
        let (rebuilt, truth) = self.replay(engine, manifest, batches, &gone)?;
        let (kept, afresh) = (grouping(engine)?, grouping(&rebuilt)?);
        // Both are in the order of their record IDs, which say what a
        // record holds: the two stores must hold the same rows.
        let same = kept.len() == afresh.len()
            && kept
                .iter()
                .zip(&afresh)
                .all(|((one, _), (other, _))| one == other);
        if !same {
            return Err(Error::Scenario(format!(
                "the store holds {} records its sources have not taken back, and the same steps \
                 without the rows that were taken back give {} records, or others",
                kept.len(),
                afresh.len()
            )));
        }
        let metrics = rebuilt.score(truth_relation(&rebuilt, &truth.entities)?)?;
        let mut rebuild = Rebuild::between(&kept, &afresh, metrics);
        // Of the pairs only the store has together, those a reviewer is
        // asked about.
        let group = |grouping: &'_ [(String, String)]| -> HashMap<String, String> {
            grouping.iter().cloned().collect()
        };
        let (here, there) = (group(&kept), group(&afresh));
        let mut asked_about = HashSet::new();
        for (_, one, other) in asked(engine)? {
            let (one, other) = (one.to_string(), other.to_string());
            let together =
                |groups: &HashMap<String, String>| match (groups.get(&one), groups.get(&other)) {
                    (Some(a), Some(b)) => Some(a == b),
                    _ => None,
                };
            if together(&here) == Some(true) && together(&there) == Some(false) {
                asked_about.insert((one, other));
            }
        }
        rebuild.pairs_only_kept_asked = asked_about.len() as u64;
        Ok(rebuild)
    }

    /// Runs the steps that ingest anything in reverse, without corrections
    /// and without the rows that are taken back at the end, and compares
    /// the grouping that leaves with the one the written order leaves.
    /// `engine` holds the scenario as it was just run.
    pub(crate) fn order_sensitivity(
        &self,
        engine: &Engine,
        manifest: &Manifest,
    ) -> Result<Option<OrderSensitivity>> {
        let batches: Vec<&StepSpec> = self.spec.steps.iter().filter(|step| step.feeds()).collect();
        if batches.len() < 2 {
            return Ok(None);
        }
        let gone = gone(engine)?;
        // This measures the order of arrival alone.
        let replay = |steps: &mut dyn Iterator<Item = &StepSpec>| -> Result<_> {
            let (replayed, _) = self.replay(engine, manifest, steps, &gone)?;
            grouping(&replayed)
        };
        // The engine holds the written order as it is unless the scenario
        // did more than ingest and resolve.
        let corrected =
            self.spec.steps.iter().any(|step| {
                !step.corrections.is_empty() || step.rematch || !step.retract.is_empty()
            });
        let written = if corrected {
            replay(&mut batches.iter().copied())?
        } else {
            grouping(engine)?
        };
        let reversed = replay(&mut batches.iter().rev().copied())?;
        Ok(Some(OrderSensitivity::between(&written, &reversed)))
    }
}
