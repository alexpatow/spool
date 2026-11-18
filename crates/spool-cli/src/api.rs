//! The JSON the server speaks: request bodies, response bodies and errors.
//!
//! Most values are the engine's own types, serialised as the engine defines
//! them. The types here are the envelopes around them. Everything is
//! described by one JSON Schema, [`schema`], from which client types are
//! generated.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use spool::core::{
    Changeset, ChangesetId, Committed, ConstraintEntry, DocumentId, EntityType, IdentityId,
    LogEntry, Op, RecordId, Rejection, RunId, Seq, Verdict,
};
use spool::{
    Authored, DocumentEntry, DraftEntry, DraftSummary, Explanation, HistoryEntry, IdentitySummary,
    IdentityView, MintedIds, NewDocument, RecordEntry, ReviewCase, ReviewEntry, RunEntry,
    SplitReport, StoreInfo, StrategyRef, TypeResolution,
};

/// The version of the API, which is the first segment of every path.
pub const VERSION: &str = "v1";

/// The media type of an Arrow IPC stream.
pub const ARROW_STREAM: &str = "application/vnd.apache.arrow.stream";

/// The media type of CSV: a first line that names the columns, then a line
/// for each row.
pub const CSV: &str = "text/csv";

/// The media type of CSV with tabs between the fields.
pub const TSV: &str = "text/tab-separated-values";

/// `POST /v1/ingest` with a JSON body. With an Arrow or a CSV body, the
/// same fields other than `rows` are query parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IngestRequest {
    /// The name of the source the rows come from, which the project
    /// declares in `sources/<name>.yaml`.
    pub source: String,
    pub entity_type: String,
    /// One object per row, with the columns the source declares. A column
    /// holds text, booleans or numbers.
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// `POST /v1/retract`: take back rows a source no longer holds. The
/// response is the retract run, whose `retraction` says what it did.
///
/// Every version of each row stays as evidence and leaves its identity, by
/// a changeset recorded under the caller. No run examines the records
/// after that, until an ingest sends a row under the same key again.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetractRequest {
    /// The name of the source that held the rows.
    pub source: String,
    pub entity_type: String,
    /// The keys of the rows, as the source's key column gave them. A key
    /// the source has no row under is counted as unknown and changes
    /// nothing.
    pub keys: Vec<String>,
    /// Why, for the log. It is the reason of the changeset that detaches
    /// the records.
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// The query parameters of `POST /v1/ingest` with an Arrow body, or with a
/// CSV body: `content-type: text/csv`, or `text/tab-separated-values` for
/// tabs between the fields. The first line of CSV names the columns, every
/// value is text, and an empty field is a missing value.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestParams {
    pub source: String,
    pub entity_type: String,
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// `POST /v1/documents`: store documents from one source, to be read by an
/// extractor. The response is the ingest run that stored them.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentsRequest {
    /// The name of the source the documents come from. The records
    /// extracted from them have the same source.
    pub source: String,
    pub documents: Vec<NewDocument>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocumentList {
    /// Documents without their text.
    pub documents: Vec<DocumentEntry>,
    /// Pass as `after` for the next page. `null` on the last page.
    pub next: Option<DocumentId>,
}

/// `POST /v1/extractions`: run one of the project's extractors over the
/// documents it has not read. The response is the extract run. What it
/// found is records, which `POST /v1/runs` then matches.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtractRequest {
    /// The name of one of the project's extractors; `GET /v1/info` lists
    /// them. The server runs no code it was not built with.
    pub extractor: String,
    /// Read only the documents of this source.
    #[serde(default)]
    pub source: Option<String>,
    /// Read the documents this extractor has read before as well.
    #[serde(default)]
    pub again: Option<bool>,
    /// Read at most this many documents, oldest first.
    #[serde(default)]
    pub limit: Option<u64>,
    /// How long the call waits for the run, in milliseconds. If the run is
    /// still in progress then, the response has status 202 and the run as
    /// far as it has got.
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// `POST /v1/runs`: match the new records of one entity type.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartRunRequest {
    pub entity_type: EntityType,
    /// Who judges. Left out, or named `rules`, the server runs the
    /// project's SQL rules and reconciles. The name of one of the project's
    /// user-code strategies runs that code on the server and reconciles;
    /// `GET /v1/info` lists them. Any other name starts a run that waits
    /// for the caller's judgements: the server runs no code it was not
    /// built with, whatever is named here.
    ///
    /// A `version` given with a project strategy must be the hash the
    /// project was built with.
    #[serde(default)]
    pub strategy: Option<StrategyRef>,
    /// With a strategy the server runs: commit the draft (the default), or
    /// store it for review. A run that waits for judgements is told this
    /// when it is reconciled.
    #[serde(default)]
    pub commit: Option<bool>,
    /// Match every record of the type again, not only the new ones.
    /// Records that have an identity keep it: where the strategy
    /// disagrees with the identities as they are, the run's `resolution`
    /// lists `suggestions`.
    #[serde(default)]
    pub rematch: Option<bool>,
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
    pub run: Option<RunEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunList {
    pub runs: Vec<RunEntry>,
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
    pub left_id: RecordId,
    pub right_id: RecordId,
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

/// `POST /v1/changesets`: a changeset to validate and commit. Its author is
/// whoever sends it: the person signed in, or the agent whose token it is.
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
    /// The records on one side. The rest of the identity's members are
    /// the other side. The larger side keeps the ID, the side not named here
    /// on a tie.
    pub records: Vec<RecordId>,
    /// Record a `cannot_link` constraint between the two sides (the
    /// default), so that nothing puts them back together unnoticed.
    #[serde(default)]
    pub keep_apart: Option<bool>,
    /// The log position the identity was read at. With it, the split is
    /// refused if the identity's members changed after that: which side
    /// moves depends on all of them. Without it, the split is made of the
    /// members the identity has now.
    #[serde(default)]
    pub base: Option<Seq>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConstraintList {
    pub constraints: Vec<ConstraintEntry>,
}

/// `POST /v1/review/{id}/dismiss`: close a review item without placing its
/// records.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DismissRequest {
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
pub struct RecordList {
    pub records: Vec<RecordEntry>,
    /// Pass as `after` for the next page. `null` on the last page.
    pub next: Option<RecordId>,
}

/// `POST /v1/records/identities`: the identities of many records
/// in one call.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdentitiesOfRequest {
    pub records: Vec<RecordId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IdentitiesOfResponse {
    /// One entry per record asked about, in the same order. `null` for
    /// a record that has no identity or is not known.
    pub identities: Vec<Option<IdentityId>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewList {
    pub items: Vec<ReviewEntry>,
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
    /// 401: nobody is signed in and there is no token, or the username,
    /// password or token is wrong.
    Unauthorized,
    /// 403: the caller is known and may not do this: only an admin manages
    /// people, and a write must come from the server's own pages.
    Forbidden,
    /// 429: too many wrong passwords for this username, or more sign-ins
    /// at once than the server checks. Wait and try again.
    TooManyAttempts,
    /// 400: the request cannot be read, or names something that is not
    /// declared.
    Invalid,
    /// 404: there is no such run, draft, changeset, identity, record,
    /// document, review item or path.
    NotFound,
    /// 409: the thing exists and is not in a state that allows this.
    Conflict,
    /// 409: the changeset broke a rule. `rejections` names the operations.
    Rejected,
    /// 413: the body is larger than the server reads.
    TooLarge,
    /// 500: the project's user code failed, did not finish in time, or
    /// wrote judgements that were refused. The run is recorded as failed
    /// and nothing changed.
    StrategyFailed,
    /// 500: the project's extractor failed, did not finish in time, or
    /// wrote mentions that were refused. The run is recorded as failed and
    /// nothing it found was stored.
    ExtractorFailed,
    /// 500: the server failed.
    Internal,
}

/// Someone with an account on this server. The log names a person by `id`,
/// as `user:<id>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Person {
    pub id: String,
    pub username: String,
    pub name: String,
    /// May add people and change their accounts.
    pub admin: bool,
    /// May sign in. An account that is turned off keeps its name in the log.
    pub active: bool,
    /// The address of the Google account they sign in with, where the
    /// server signs people in with Google. `null` for someone who signs in
    /// with a password only.
    pub email: Option<String>,
    /// Has a password to sign in with. Someone who has none signs in with
    /// Google.
    pub password: bool,
}

/// `GET /auth/session`: who the caller is signed in as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionState {
    pub person: Option<Person>,
    /// No admin can sign in yet. The server printed a link when it started
    /// that makes the first account.
    pub setup: bool,
    /// A development server: nobody signs in, and whoever opens it is the
    /// `dev` account.
    pub dev: bool,
    /// The server signs people in with Google: `GET /auth/google` in a
    /// browser starts it.
    pub google: bool,
}

/// `POST /auth/sign-in`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignInRequest {
    pub username: String,
    pub password: String,
}

/// `POST /auth/setup`: make the first account, an admin, with the code from
/// the link the server printed.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupRequest {
    pub code: String,
    pub username: String,
    pub name: String,
    pub password: String,
}

/// `POST /auth/password`: change the caller's own password.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordRequest {
    pub current: String,
    pub new: String,
}

/// `POST /v1/people`: add a person. Only an admin may. They sign in with
/// the password, with Google by the email, or with either: one of the two is
/// needed, and the email alone only where the server signs people in with
/// Google.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddPersonRequest {
    pub username: String,
    pub name: String,
    #[serde(default)]
    pub password: Option<String>,
    /// The address of the Google account they sign in with.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub admin: Option<bool>,
}

/// `POST /v1/people/{id}`: change a person. Only an admin may. A field left
/// out stays as it is. A new password, or turning the account off, signs
/// the person out everywhere. Another email signs them out wherever they
/// signed in with Google.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangePersonRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// The address of the Google account they sign in with. An empty one
    /// takes it away, for someone who has a password.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub admin: Option<bool>,
    #[serde(default)]
    pub active: Option<bool>,
}

/// `GET /v1/people`: everyone with an account, in the order they were added.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PersonList {
    pub people: Vec<Person>,
}

/// A token: what a caller that is not a person presents, as
/// `Authorization: Bearer <token>`. The log names what it changed by the
/// token's name, as `agent:<name>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Token {
    pub id: String,
    pub name: String,
    pub created_at: String,
    /// The ID of the person who made it. `null` when it was made on the
    /// server's machine with `spool token add`.
    pub created_by: Option<String>,
    /// When it was last presented, to the minute. `null` if it never was.
    pub last_used_at: Option<String>,
}

/// `POST /v1/tokens`: make a token. Only an admin may.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTokenRequest {
    /// What its changes are recorded under, as `agent:<name>`: 1 to 64
    /// letters, digits, `.`, `_` or `-`. No other token has it.
    pub name: String,
}

/// A token that was just made. This is the only time the server says what
/// the token is: it keeps a hash of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddedToken {
    pub token: Token,
    /// What the caller presents.
    pub secret: String,
}

/// `GET /v1/tokens`: every token, in the order they were made. Only an
/// admin may read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenList {
    pub tokens: Vec<Token>,
}

/// Every type that crosses the API, so that one schema describes them all.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct Wire {
    ingest_request: IngestRequest,
    retract_request: RetractRequest,
    documents_request: DocumentsRequest,
    document_list: DocumentList,
    document_entry: DocumentEntry,
    extract_request: ExtractRequest,
    start_run_request: StartRunRequest,
    run_response: RunResponse,
    run_list: RunList,
    judgements_request: JudgementsRequest,
    judgements_response: JudgementsResponse,
    reconcile_request: ReconcileRequest,
    type_resolution: TypeResolution,
    candidate_list: CandidateList,
    draft_list: DraftList,
    draft_entry: DraftEntry,
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
    review_entry: ReviewEntry,
    review_case: ReviewCase,
    changeset_list: ChangesetList,
    mint_request: MintRequest,
    minted_ids: MintedIds,
    identity_list: IdentityList,
    identity_view: IdentityView,
    history: History,
    record_list: RecordList,
    record_from_row: RecordEntry,
    identities_of_request: IdentitiesOfRequest,
    identities_of_response: IdentitiesOfResponse,
    review_list: ReviewList,
    explanation: Explanation,
    info: Info,
    error_response: ErrorResponse,
    changeset_id: ChangesetId,
    session_state: SessionState,
    sign_in_request: SignInRequest,
    setup_request: SetupRequest,
    password_request: PasswordRequest,
    add_person_request: AddPersonRequest,
    change_person_request: ChangePersonRequest,
    person_list: PersonList,
    add_token_request: AddTokenRequest,
    added_token: AddedToken,
    token_list: TokenList,
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
