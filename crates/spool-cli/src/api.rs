//! The JSON the server speaks: request bodies, response bodies and errors.
//!
//! Most values are the engine's own types, serialised as the engine defines
//! them. The types here are the envelopes around them. Everything is
//! described by one JSON Schema, [`schema`], from which client types are
//! generated.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use spool::core::{
    Actor, Changeset, ChangesetId, Committed, ConstraintRecord, EntityType, IdentityId, LogEntry,
    ObservationId, Op, Rejection, RunId, Seq, Verdict,
};
use spool::{
    Authored, DraftRecord, DraftSummary, HistoryEntry, IdentitySummary, IdentityView, MintedIds,
    ObservationRecord, ReviewRecord, RunRecord, SplitReport, StoreInfo, StrategyRef,
    TypeResolution,
};

/// The version of the API, which is the first segment of every path.
pub const VERSION: &str = "v1";

/// The media type of an Arrow IPC stream.
pub const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";

/// `POST /v1/ingest` with a JSON body. With an Arrow body, the same fields
/// other than `rows` are query parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IngestRequest {
    /// The name of the source the rows come from.
    pub source: String,
    pub entity_type: String,
    /// The column that holds each row's key within the source.
    #[serde(default)]
    pub key: Option<String>,
    /// A column of JSON text that says where each row was found.
    #[serde(default)]
    pub evidence: Option<String>,
    /// One object per row. A column holds text, booleans or numbers.
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// The query parameters of `POST /v1/ingest` with an Arrow body.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestParams {
    pub source: String,
    pub entity_type: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub evidence: Option<String>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// `POST /v1/runs`: match the new observations of one entity type.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartRunRequest {
    pub entity_type: EntityType,
    /// Who judges the candidates. Left out, or named `rules`, the server
    /// runs the project's SQL rules and reconciles. Any other name starts a
    /// run that waits for the caller's judgements.
    #[serde(default)]
    pub strategy: Option<StrategyRef>,
    /// With the built-in strategy: commit the draft (the default), or store
    /// it for review. A run that waits for judgements is told this when it
    /// is reconciled.
    #[serde(default)]
    pub commit: Option<bool>,
    /// How long the call waits for the run, in milliseconds. If the run is
    /// still in progress then, the response has status 202 and the run as
    /// far as it has got.
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// A run, as the response of a call that started one or read one. `run` is
/// `null` when there was nothing to do, so no run was started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunResponse {
    pub run: Option<RunRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunList {
    pub runs: Vec<RunRecord>,
}

/// `POST /v1/runs/{run}/judgements` with a JSON body.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JudgementsRequest {
    pub judgements: Vec<Judgement>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Judgement {
    pub left_id: ObservationId,
    pub right_id: ObservationId,
    pub verdict: Verdict,
    /// On the strategy's own scale. Scores are never compared across
    /// strategies.
    #[serde(default)]
    pub score: Option<f64>,
    /// Anything the strategy wants kept with the judgement.
    #[serde(default)]
    pub detail: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JudgementsResponse {
    pub stored: u64,
}

/// `POST /v1/runs/{run}/reconcile`. The body may be left out.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileRequest {
    /// Commit the draft (the default), or store it for review.
    #[serde(default)]
    pub commit: Option<bool>,
}

/// `GET /v1/runs/{run}/candidates` when JSON is asked for. Each candidate has
/// `left_id`, `right_id`, and every feature column prefixed with `left_` and
/// with `right_`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct CandidateList {
    pub candidates: Vec<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DraftList {
    pub drafts: Vec<DraftSummary>,
}

/// `POST /v1/drafts/{draft}/reject`. The body may be left out.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RejectDraftRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

/// `POST /v1/changesets`: a changeset to validate and commit.
///
/// A caller that has read the state it is changing says so with `base`: a
/// merge is rejected if the members of either identity changed after it. A
/// caller that leaves `base` out is taken to have read the head of the log.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    /// The changeset's ID. Left out, the server mints one. A caller that may
    /// send the same changeset twice mints the ID itself, so that the second
    /// one is refused.
    #[serde(default)]
    pub id: Option<ChangesetId>,
    /// The log position the author had read. Left out, the head of the log.
    #[serde(default)]
    pub base: Option<Seq>,
    pub actor: Actor,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub run: Option<RunId>,
    pub ops: Vec<Op>,
}

/// `POST /v1/changesets/{seq}/undo`: take back the changeset at a log
/// position with a new changeset.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoRequest {
    pub actor: Actor,
    #[serde(default)]
    pub reason: Option<String>,
    /// Commit the undo (the default), or only validate it and return it.
    #[serde(default)]
    pub commit: Option<bool>,
}

/// `POST /v1/identities/{identity}/split`: divide an identity in two.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitRequest {
    /// The observations on one side. The rest of the identity's members are
    /// the other side. The larger side keeps the ID, the side not named here
    /// on a tie.
    pub observations: Vec<ObservationId>,
    /// Record a `cannot_link` constraint between the two sides (the
    /// default), so that nothing puts them back together unnoticed.
    #[serde(default)]
    pub keep_apart: Option<bool>,
    pub actor: Actor,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConstraintList {
    pub constraints: Vec<ConstraintRecord>,
}

/// `POST /v1/review/{id}/dismiss`: close a review item without placing its
/// observations.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DismissRequest {
    pub actor: Actor,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `GET /v1/changesets`: a page of the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangesetList {
    pub changesets: Vec<LogEntry>,
    /// The position of the last committed changeset.
    pub head: Seq,
}

/// `POST /v1/ids`: IDs for a caller that writes its own changeset.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MintRequest {
    #[serde(default)]
    pub identities: u32,
    #[serde(default)]
    pub changesets: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IdentityList {
    pub identities: Vec<IdentitySummary>,
    /// Pass as `after` for the next page. `null` on the last page.
    pub next: Option<IdentityId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct History {
    pub history: Vec<HistoryEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationList {
    pub observations: Vec<ObservationRecord>,
    /// Pass as `after` for the next page. `null` on the last page.
    pub next: Option<ObservationId>,
}

/// `POST /v1/observations/identities`: the identities of many observations
/// in one call.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdentitiesOfRequest {
    pub observations: Vec<ObservationId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IdentitiesOfResponse {
    /// One entry per observation asked about, in the same order. `null` for
    /// an observation that has no identity or is not known.
    pub identities: Vec<Option<IdentityId>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewList {
    pub items: Vec<ReviewRecord>,
}

/// `GET /v1/info`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Info {
    /// The version of the API this server speaks.
    pub api: String,
    pub store: StoreInfo,
}

/// The body of every response that is not a success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    /// With `rejected`: the operations of the changeset that broke a rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejections: Option<Vec<Rejection>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// 401: the bearer token is missing or wrong.
    Unauthorized,
    /// 400: the request cannot be read, or names something that is not
    /// declared.
    Invalid,
    /// 404: there is no such run, draft, changeset, identity, observation,
    /// review item or path.
    NotFound,
    /// 409: the thing exists and is not in a state that allows this.
    Conflict,
    /// 409: the changeset broke a rule. `rejections` names the operations.
    Rejected,
    /// 413: the body is larger than the server reads.
    TooLarge,
    /// 500: the server failed.
    Internal,
}

/// Every type that crosses the API, so that one schema describes them all.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct Wire {
    ingest_request: IngestRequest,
    start_run_request: StartRunRequest,
    run_response: RunResponse,
    run_list: RunList,
    judgements_request: JudgementsRequest,
    judgements_response: JudgementsResponse,
    reconcile_request: ReconcileRequest,
    type_resolution: TypeResolution,
    candidate_list: CandidateList,
    draft_list: DraftList,
    draft_record: DraftRecord,
    reject_draft_request: RejectDraftRequest,
    changeset: Changeset,
    commit_request: CommitRequest,
    committed: Committed,
    undo_request: UndoRequest,
    authored: Authored,
    split_request: SplitRequest,
    split_report: SplitReport,
    constraint_list: ConstraintList,
    dismiss_request: DismissRequest,
    review_record: ReviewRecord,
    changeset_list: ChangesetList,
    mint_request: MintRequest,
    minted_ids: MintedIds,
    identity_list: IdentityList,
    identity_view: IdentityView,
    history: History,
    observation_list: ObservationList,
    observation_record: ObservationRecord,
    identities_of_request: IdentitiesOfRequest,
    identities_of_response: IdentitiesOfResponse,
    review_list: ReviewList,
    info: Info,
    error_response: ErrorResponse,
    changeset_id: ChangesetId,
}

/// The JSON Schema of the API: one definition per type, under `$defs`.
pub fn schema() -> serde_json::Value {
    let generated =
        serde_json::to_value(schemars::schema_for!(Wire)).expect("a schema serialises to JSON");
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": format!("https://github.com/alexpatow/spool/schema/spool-api.{VERSION}.schema.json"),
        "title": format!("Spool API {VERSION}"),
        "description": "The JSON values of the Spool HTTP API. Generated from the server's types; do not edit.",
        "$defs": generated["$defs"],
    })
}
