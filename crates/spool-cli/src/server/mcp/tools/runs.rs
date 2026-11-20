//! Tools for runs: reading one, and starting a match.

use schemars::JsonSchema;
use serde::Deserialize;
use spool::StrategyRef;
use spool::core::{EntityType, RunId};

use super::{Declared, Effect, declare};
use crate::api::MatchRequest;
use crate::server::App;
use crate::server::error::{ApiError, ApiResult};
use crate::server::mcp::limits::beginning;

/// The longest `get_run` waits for a run that is still in progress. A
/// client gives up on a call that takes much longer.
const MAX_RUN_WAIT_MS: u64 = 60_000;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneRun {
    /// The `run` of the run: a number.
    run: u64,
    /// If the run is still in progress, wait up to this many milliseconds
    /// for it to finish. Not at all unless given, 60000 at most.
    #[serde(default)]
    wait_ms: Option<u64>,
}

pub(super) fn get_run() -> Declared {
    declare(
        "get_run",
        "Get a run",
        Effect::Reads,
        "One run by its number: whether it is `running`, `succeeded` or `failed`, who \
         started it, and what it reported. A load reports in `load` how many rows or documents were \
         new; an extraction reports in `extraction` the documents it read and the \
         records it found; a match run reports in `resolution` how many records it \
         examined, the pairs it judged, the identities it created, and what it left \
         for review. Use this after `load_rows`, `run_extraction` or `run_match` \
         answered with a run that was still `running`, with `wait_ms` to wait for it.",
        |app, _, asked: OneRun| async move {
            let wait_ms = asked.wait_ms.map(|wait| wait.min(MAX_RUN_WAIT_MS));
            app.read_run(RunId(asked.run), wait_ms).await
        },
    )
    .answered_whole()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunMatch {
    /// The entity type whose records are matched.
    entity_type: EntityType,
    /// Match every record of the type again, not only those no run has
    /// examined. Records that have an identity keep it: where the rules
    /// now disagree with the identities, the run's `resolution` lists
    /// `suggestions` and changes nothing. Records that have none, such as
    /// those of a match run that was undone, are placed. False unless
    /// given.
    #[serde(default)]
    rematch: Option<bool>,
    /// The name of one of the project's strategies for the type, which
    /// `describe_store` lists. Left out, the project's SQL rules judge.
    #[serde(default)]
    strategy: Option<String>,
}

pub(super) fn run_match() -> Declared {
    declare(
        "run_match",
        "Run a match",
        Effect::Changes,
        "Match the records of one entity type that no run has examined, and commit the \
         result: each record joins the identity it matches or gets a new one, and \
         identities that existed keep their IDs. A record that matches more than one \
         identity, or that the evidence contradicts itself about, is left as a review \
         item. The change is one changeset in the log, which `undo_changeset` takes \
         back. Returns the run, whose `resolution` has the counts; `run` is null when \
         there was nothing to match. If its `status` is still `running`, read it with \
         `get_run`.",
        |app, caller, asked: RunMatch| async move {
            let strategy = match asked.strategy {
                Some(name) => Some(project_strategy(&app, &asked.entity_type, name)?),
                None => None,
            };
            let request = MatchRequest {
                entity_type: asked.entity_type,
                strategy,
                // A draft would wait for an approval no tool gives.
                commit: None,
                rematch: asked.rematch,
                wait_ms: None,
            };
            Ok(app.start_match(caller, request).await?.run)
        },
    )
}

/// The strategy a match run is asked for by name, which has to be one
/// the server runs. The API takes any other name as that of a strategy
/// the caller runs, and starts a run that waits for its judgements;
/// no tool sends any, so such a run would wait for good.
fn project_strategy(app: &App, entity_type: &EntityType, name: String) -> ApiResult<StrategyRef> {
    let manifest = app.engine.manifest();
    let known = name == StrategyRef::RULES
        || manifest
            .as_deref()
            .and_then(|manifest| manifest.entity_type(entity_type.as_str()))
            .is_some_and(|entry| entry.strategy(&name).is_some());
    match known {
        true => Ok(StrategyRef {
            name,
            version: None,
        }),
        false => Err(ApiError::invalid(format!(
            "the project has no strategy `{}` for `{entity_type}`: `describe_store` lists \
             the strategies of each entity type, and `rules` is always one",
            beginning(&name)
        ))),
    }
}
