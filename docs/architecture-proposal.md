# Spool architecture proposal: initial prototype

A minimal Rust-first design for the first Spool prototype, grounded in the [landscape survey](landscape-survey.md). Dated 2026-11-13. Nothing here is built. Where the text says something was tested or measured, that was a throwaway local experiment with DuckDB's Rust bindings, not a benchmark.

Milestone numbers refer to Spool's initial roadmap: (1) a Rust engine on DuckDB, (2) TypeScript and Python bindings, (3) the identity lifecycle across successive batches, (4) interchangeable resolution strategies, (5) extraction from unstructured documents, (6) sandbox synchronisation with a shared store, and (7) developer tooling.

## Summary of conclusions

1. **Split the system into identity state and evidence.** Identity state (identities, which observation belongs to which identity, manual constraints, the log of changes) is small, typed and transactional, and its rules live in a pure Rust crate. Evidence (observations, candidate pairs, match judgements) is large and analytical, and lives in DuckDB tables that strategies query with SQL. Only identity state needs to be independent of DuckDB.
2. **Replace a single "proposal" concept with two sharper things.** Strategies emit *judgements*: pairwise evidence with a verdict of match, no match or unsure. A framework-owned *reconciler* turns judgements plus current state into a *draft changeset*. Extensions never write identity operations, which makes Spool's central invariant, that extensions may propose identity changes but only the framework commits them, enforceable by construction.
3. **Identity IDs are minted, never derived.** A UUIDv7 is assigned once, is never reused, and a merged ID redirects to its survivor permanently. By default the matcher can attach new observations to existing identities and create new ones, and can only *suggest* merging or splitting existing ones. That is what makes the design goal that a better matching model must not change existing IDs hold.
4. **History is a hybrid.** An append-only changeset log is the source of truth. Current-state tables are a projection written in the same transaction and rebuildable from the log. Every operation carries its own precondition, which is the hook sandbox sync needs later.
5. **The matcher contract is a table shape, not a trait.** Observations in, `(left, right, verdict, score, detail)` out. Blocking, scoring, Splink, an LLM and a human reviewer all fit it. Clustering happens only in the reconciler.
6. **Bindings are thin and the engine never calls into Python or JavaScript.** The host language drives a loop: pull candidates as Arrow, compute, push judgements as Arrow. Small things cross as typed objects. DuckDB relations never cross the boundary.
7. **Start with two crates.** `spool-core` (no DuckDB, no Arrow) and `spool` (engine, DuckDB store, built-in strategy, metrics). Add the binding crates in Milestone 2 and split further only when a second implementation of something exists.
8. **Milestone 1 must ingest two batches.** A single batch cannot show that IDs persist, which is the only thing that separates Spool from Splink. Changesets also have to exist in Milestone 1 because they are the write path; only their transport belongs in Milestone 6.
9. **Entity types are declared, never built in.** Spool defines no company, person or any other type, and no two users have to agree on what a company looks like. A user declares a type and its feature columns, and the engine generates that type's feature table from the declaration. Companies appear in this document as the worked example and as the first benchmark fixture, nowhere in the engine.

## Where this departs from the initial design goals

Spool's initial design goals and roadmap are the starting point for this proposal. It keeps most of them and departs from the following.

| Initial goal or assumption | Position | Why |
| --- | --- | --- |
| Three core abstractions: observations, proposals and changesets | Keep observations and changesets. Split proposals into judgements (evidence from strategies) and draft changesets (intent from the reconciler). | "Proposal" mixes what a matcher saw with what should change. A matcher that proposes a merge has to know identity state and operation semantics, which is the coupling Spool's design goals set out to avoid. nomenklatura already works this way ([survey, section 4](landscape-survey.md#nomenklatura-and-opensanctions)). |
| An eventual layout of seven crates: core, DuckDB store, resolution, benchmarks, Node bindings, Python bindings and CLI | Two crates for Milestone 1, four after Milestone 2. | The initial plan treats these as logical boundaries and allows starting with fewer crates. The built-in strategy is SQL run through the store and the benchmark needs both, so `spool-duckdb`, `spool-resolution` and `spool-bench` would have no independent boundary yet. |
| Core identity semantics are independent of any particular database | Apply it to identity state only. Let evidence handling be openly DuckDB-specific. | Abstracting the analytical engine means a SQL dialect layer. Splink carries `sqlglot` and per-backend code for this. A shared Postgres store needs the log and the projection, not candidate generation. |
| Milestone 1 creates identities; Milestone 3 preserves them across batches | Move "second batch keeps its IDs" into Milestone 1. | Otherwise Milestone 1 validates nothing that Splink plus a UUID column would not. |
| Milestone 6 defines identity changesets | Define the changeset format and preconditions in Milestone 1. Milestone 6 adds transport and conflict policy. | Changesets are the only way state changes. Retrofitting preconditions and stable observation IDs after data exists is a format migration. |
| Milestone 2 bindings expose the core functionality | Include submitting judgements from Python and TypeScript in Milestone 2. | Calling `resolve()` from another language proves packaging. The extension path is the part of the language architecture that can fail. |
| DuckDB relations are one of the cross-language data representations | Relations stay inside the engine. SQL text goes in and Arrow comes out. | A relation is a handle into one process's DuckDB instance. A second process cannot open the file at all while the engine holds it, even read-only (tested). |
| The core owns canonical entity representations | Core owns *which* observations form an identity. Canonical attributes are derived views over evidence. | Storing a golden record as identity state makes every attribute rule an identity operation and works against the goal that evidence stays traceable. Relationships and temporal attributes are derived the same way. |
| Persistent identity is an unoccupied space in open source | It is partly occupied. | nomenklatura, GoldenMatch and Zingg Enterprise each do part of it. The gap is narrower and is described in the [survey](landscape-survey.md#7-borrow-avoid-and-the-gap). |

## The model

Five concepts. The first two are evidence, the last three are identity state.

- **Observation.** One immutable record of something a source said about one entity: a CRM row, or a company mention extracted from a deck. Its ID is derived from its content, so ingesting the same thing twice is a no-op. A changed source row is a new observation that names the one it supersedes.
- **Judgement.** A strategy's statement about two observations: `match`, `no_match` or `unsure`, with an optional score and explanation. Judgements are kept, are attributed to a run, and never change state by themselves.
- **Identity.** A minted ID with an entity type and a status: active, or merged into another identity. It has no attributes of its own.
- **Constraint.** A human or agent decision that two observations must or must not share an identity. Constraints are state: they are changed by changesets and enforced on every later changeset.
- **Changeset.** An ordered list of operations applied atomically, with an author, a reason and the evidence run it came from. It is the only way identity state changes.

An entity type is a name the user declares, such as `company`, `person` or `product`. Spool ships none. To identity state a type is only that name. What an entity of that type looks like is an evidence-side question, answered by the type's declaration in section 1.

Invariants the core enforces on every commit:

1. An observation belongs to at most one identity. Belonging to none is legal and means the system abstained.
2. Observations are assigned only to active identities, and all members of an identity share its entity type.
3. An identity ID is never deleted or reused. A merged identity points at an active one.
4. No committed state violates a constraint.
5. The current-state tables equal the result of replaying the log.

Spool's central invariant, that extensions may propose identity changes but only the framework commits them, becomes: **extensions supply evidence; the framework decides and commits.** A user or agent may also author a changeset directly, for example a manual split. It goes through the same validation.

## 1. DuckDB representation of observations, candidates and identity state

All tables live in a `spool` schema so a user's own tables can share the file. The column types used below or available to feature columns (`UUID`, `JSON`, `STRUCT`, `LIST`, `MAP`) and the `uuidv7()` function are all in the stock DuckDB library (tested).

Evidence:

```sql
CREATE TABLE spool.observation (
  observation_id UUID        NOT NULL, -- first 128 bits of sha256(source_id, source_key, canonical payload)
  entity_type    VARCHAR     NOT NULL, -- a declared type, for example 'company'
  source_id      VARCHAR     NOT NULL, -- 'crm', 'pitchbook', 'deck:acme-seed.pdf'
  source_key     VARCHAR,              -- row key in the source; NULL for extracted mentions
  supersedes     UUID,                 -- previous version of the same (source_id, source_key)
  payload        JSON        NOT NULL, -- attributes exactly as received
  evidence       JSON,                 -- locator: uri, page, character span, extractor and version
  observed_at    TIMESTAMPTZ,          -- when the source says it held, if known
  ingested_at    TIMESTAMPTZ NOT NULL,
  ingest_run     BIGINT      NOT NULL
);

CREATE TABLE spool.entity_type (          -- declared by the user; the engine ships none
  entity_type      VARCHAR     NOT NULL,  -- any name: 'company', 'person', 'product', ...
  version          INTEGER     NOT NULL,  -- increases when the declaration changes
  declaration      JSON        NOT NULL,  -- feature columns: name, DuckDB type, SQL expression
  declaration_hash VARCHAR     NOT NULL,
  declared_at      TIMESTAMPTZ NOT NULL
);

-- Generated by the engine, one per declared type, as spool.features_<entity_type>.
-- Derived from payload, rebuildable, and what matchers read. Only the first and last
-- columns are fixed. The rest come from the declaration; these come from the example below.
CREATE TABLE spool.features_company (
  observation_id      UUID    NOT NULL,
  name_norm           VARCHAR,
  domain              VARCHAR,
  country             VARCHAR,
  registry_id         VARCHAR,
  declaration_version INTEGER NOT NULL  -- the declaration version that produced the row
);

CREATE TABLE spool.run (
  run_id BIGINT NOT NULL, kind VARCHAR NOT NULL,   -- 'ingest' | 'match' | 'reconcile' | 'bench'
  strategy VARCHAR, strategy_version VARCHAR, config JSON, config_hash VARCHAR,
  engine_version VARCHAR NOT NULL, started_at TIMESTAMPTZ NOT NULL, finished_at TIMESTAMPTZ
);

CREATE TABLE spool.candidate (            -- per run, disposable
  run_id BIGINT NOT NULL, left_id UUID NOT NULL, right_id UUID NOT NULL, rule VARCHAR
);

CREATE TABLE spool.judgement (            -- kept; cited by changesets through run_id
  run_id   BIGINT  NOT NULL,
  left_id  UUID    NOT NULL,
  right_id UUID    NOT NULL,             -- left_id < right_id
  verdict  VARCHAR NOT NULL,             -- 'match' | 'no_match' | 'unsure'
  score    DOUBLE,                       -- strategy-local scale, never compared across strategies
  detail   JSON                          -- per-field comparison levels, rule name, LLM rationale
);
```

`spool.features_company` above is not part of Spool. It is what the engine generates from this declaration, which is one user's idea of a company:

```toml
entity_type = "company"
features = [
  { name = "name_norm",   type = "VARCHAR", sql = "lower(trim(payload->>'name'))" },
  { name = "domain",      type = "VARCHAR", sql = "lower(payload->>'domain')" },
  { name = "country",     type = "VARCHAR", sql = "upper(payload->>'country')" },
  { name = "registry_id", type = "VARCHAR", sql = "payload->>'registry_id'" },
]
```

Another user's `company` can have different columns, and a deployment that resolves people, products or papers declares those types and never has a `company` at all. The declaration is shown as TOML, but it is one of the typed objects of section 5, so an SDK passes it as a plain object.

Identity state:

```sql
CREATE TABLE spool.changeset (            -- the log; source of truth
  seq          BIGINT      NOT NULL,      -- dense, assigned at commit; this store's total order
  changeset_id UUID        NOT NULL,      -- minted by the author; idempotency key
  base_seq     BIGINT      NOT NULL,      -- log position the author had seen
  actor        VARCHAR     NOT NULL,      -- 'engine' | 'user:<id>' | 'agent:<id>'
  reason       VARCHAR,
  run_id       BIGINT,                    -- reconcile run that drafted it, if any
  content_hash VARCHAR     NOT NULL,      -- hash of the canonical encoding of its operations
  committed_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE spool.changeset_op (         -- one row per operation, columnar so apply is set-based
  seq BIGINT NOT NULL, op_index INTEGER NOT NULL,
  kind VARCHAR NOT NULL,                  -- 'create_identity' | 'assign' | 'merge' | 'constrain' | 'unconstrain' | 'reinstate'
  identity_id UUID,                       -- created identity / assign target / merge survivor
  from_identity_id UUID,                  -- assign source / merged-away identity
  observation_id UUID, other_observation_id UUID,
  entity_type VARCHAR, constraint_kind VARCHAR   -- 'must_link' | 'cannot_link'
);

CREATE TABLE spool.identity (             -- projection
  identity_id UUID NOT NULL, entity_type VARCHAR NOT NULL,
  created_seq BIGINT NOT NULL,
  merged_into UUID, merged_seq BIGINT     -- always points at an active identity (chains are flattened)
);

CREATE TABLE spool.membership (           -- projection with history; current rows have to_seq IS NULL
  observation_id UUID NOT NULL, identity_id UUID NOT NULL,
  from_seq BIGINT NOT NULL, to_seq BIGINT
);

CREATE TABLE spool.constraint_link (      -- projection with history
  observation_a UUID NOT NULL, observation_b UUID NOT NULL, kind VARCHAR NOT NULL,
  from_seq BIGINT NOT NULL, to_seq BIGINT
);

CREATE TABLE spool.meta (key VARCHAR NOT NULL, value VARCHAR NOT NULL);  -- format_version
```

Design points:

- **Raw payload and typed features are separate.** The payload is never rewritten, so a better normaliser can be rerun over history. Matchers read only the feature table.
- **Feature tables are generated from a declaration, never hand-written.** A declaration lists feature columns, each with a name, a DuckDB type and a SQL expression over the observation's `payload`. The engine creates `spool.features_<entity_type>` from it and fills it with one `INSERT ... SELECT` per ingest. No engine code names a feature column, and `ingest` rejects a type that has not been declared.
- **Changing a declaration changes no identity.** Declaring a changed definition for an existing type increases its version and rebuilds the feature table from the stored payloads. Identity state refers to observations and to the type's name, never to feature columns, so every identity ID stays in place. A match run records the declaration version it read, so its judgements stay traceable.
- **Typed columns, not one generic table.** A single features table holding JSON for every type would need no generation. It would also move normalisation into every matching query and give up the typed columns that blocking joins and similarity functions work on.
- **Operations are rows, not a JSON blob.** A first load is one changeset with as many `assign` rows as observations, applied with `INSERT ... SELECT`. When measured, a one-million-operation changeset wrote its log rows and its projection rows in one transaction in 0.4 s.
- **Membership keeps intervals.** "What was this identity at log position N" is a filter, and undoing a merge can read which observations it moved.
- **No primary keys or foreign keys on the large tables at first.** Uniqueness is guaranteed by the Rust validator and by anti-join inserts, and checked by a `verify` routine in tests. DuckDB constraints did behave correctly when tested (including update and delete-then-insert of a keyed row in one transaction), but index maintenance on bulk loads is a cost I have not measured, and the identity rules must not depend on the database.
- **Public read surface is views.** `spool.identity_current`, `spool.membership_current` and a generated per-type entity view are documented and stable; base tables are not.

Tradeoffs of declared types:

- A SQL expression cannot express every normaliser. Reducing a URL to its registrable domain needs the public suffix list, for example. Feature columns computed by the host language and pushed as Arrow cover that case, and are deferred until the bindings exist.
- Expressions must be deterministic, or the table is not rebuildable. The engine cannot check that in general.
- Declared names become SQL identifiers, so the engine restricts them to a fixed pattern. Expressions are SQL that the engine runs. That is acceptable in a sandbox its user owns, and it means a shared store must not accept declarations from a sandbox unreviewed, which is a question for sync.
- The Arrow schema of feature rows differs per type, so an SDK cannot offer static types for them until it generates those from a declaration.

Tradeoff: two write targets per commit (log and projection) and no database-level safety net. Mitigation is a standing test that the projection equals a replay of the log. A simpler log-versus-projection comparison over a million rows took 0.05 s when measured.

**Decide now:** the evidence versus identity-state split, content-derived observation IDs, entity types declared at run time with versioned declarations and generated feature tables, operation rows, interval membership, a format version from the first commit. **Defer:** indexes and keys, host-computed feature columns, rebuilding a feature table after its declaration changes, retention of old candidates and judgements, how a retracted or deleted source record is represented.

## 2. Separating Rust identity semantics from DuckDB persistence

`spool-core` depends on `serde`, `uuid`, `sha2` and `thiserror`, and on neither `duckdb` nor `arrow`. The compiler then enforces the boundary. It contains the types, the validator, the reconciler, operation inversion, replay, and an in-memory state used as the reference model in tests.

```rust
pub struct ObservationId(u128);   // content-derived, computed only by the engine
pub struct IdentityId(Uuid);      // minted UUIDv7 from an injected generator
pub struct Seq(u64);

pub enum Verdict { Match, NoMatch, Unsure }
pub enum ConstraintKind { MustLink, CannotLink }
pub enum IdentityStatus { Active, MergedInto(IdentityId) }

pub enum Op {
    CreateIdentity { identity: IdentityId, entity_type: EntityType },
    /// Attach (from = None), move, or detach (to = None). `from` is the precondition.
    Assign { observation: ObservationId, from: Option<IdentityId>, to: Option<IdentityId> },
    /// Moves every current member of `from` to `into` and redirects `from`.
    Merge { from: IdentityId, into: IdentityId },
    Constrain   { kind: ConstraintKind, a: ObservationId, b: ObservationId },
    Unconstrain { kind: ConstraintKind, a: ObservationId, b: ObservationId },
    /// Makes a merged identity active again; used only to undo a merge.
    Reinstate { identity: IdentityId },
}

pub struct Changeset {
    pub id: ChangesetId, pub base: Seq, pub actor: Actor,
    pub reason: Option<String>, pub run: Option<RunId>, pub ops: Vec<Op>,
}

/// Read access the validator needs. Batch-shaped so a SQL store answers with one query per call.
pub trait IdentityState {
    fn identities(&self, ids: &[IdentityId]) -> Result<Vec<Option<IdentityRecord>>>;
    fn assignments(&self, obs: &[ObservationId]) -> Result<Vec<Option<IdentityId>>>;
    fn members(&self, ids: &[IdentityId]) -> Result<Vec<(IdentityId, ObservationId)>>;
    fn constraints_on(&self, obs: &[ObservationId]) -> Result<Vec<Constraint>>;
}

/// The only way to obtain a `Validated`, and a store accepts nothing else.
pub fn validate(state: &impl IdentityState, cs: Changeset) -> Result<Validated, Vec<Rejection>>;

pub fn reconcile(evidence: &EvidenceSummary, state: &impl IdentityState, policy: &Policy,
                 ids: &mut impl IdGenerator) -> Draft;   // Draft = Changeset + review items
```

The seam between the two halves is **SQL prepares, Rust decides**. DuckDB does the heavy join of judgements against current membership and returns a compact summary per new observation (which identities it matched, how many match and no-match judgements, the best score). The reconciler is a pure function over that summary. Core never sees SQL, and DuckDB never decides what a merge means.

The DuckDB store in the `spool` crate implements `IdentityState` and one write: apply a `Validated` changeset by inserting log rows and updating projections in a single transaction. One conformance suite runs against both the in-memory state and the DuckDB store, and property tests compare random operation sequences between the two. A future Postgres shared store is a third implementation that must pass the same suite.

Split is deliberately not an operation. It is `CreateIdentity` plus `Assign` moves, usually with a `cannot_link` constraint so the matcher does not undo it, in one changeset. The original ID stays with whichever side the author leaves in place; the default policy keeps it with the larger side, as OpenSanctions does.

Tradeoff: validating in Rust means a changeset must fit in memory, which caps one changeset at roughly millions of operations. Larger loads are chunked into several changesets. I prefer that to moving validation into SQL, which would put identity rules in the database.

**Decide now:** the dependency-free core crate, validated-changeset type, injected ID generator and clock (required for reproducible tests), operation set. **Defer:** implementing `Merge`, `Constrain` and `Reinstate` (Milestone 3), a Postgres store, any trait over evidence storage.

## 3. Event log, versioned state, or hybrid

**Hybrid, with the log as the source of truth.**

| Option | Gives | Costs |
| --- | --- | --- |
| Event log only | One truth, full audit, natural sync unit | Every read replays or needs a cache, which is a projection by another name |
| Versioned state only (interval rows) | Cheap time travel | No grouping of changes, no author or reason, no unit to sync or undo, intent is lost |
| Hybrid | Fast reads, audit, undo, sync unit | Two writes per commit and the risk they diverge |

Divergence is contained three ways: both writes share one transaction (tested: a rollback leaves both empty), the projection can be rebuilt from the log at any time, and replay equality is a standing test.

Each operation is its own precondition: `Assign.from` must equal the observation's current identity, `Merge` needs both identities active, `CreateIdentity` needs an unused ID. Commit checks these against current state and does not require `base_seq` to be the latest. Two sandboxes that touch different identities therefore both commit cleanly against a shared store, and a stale changeset is rejected with the exact operation that conflicted. That is optimistic concurrency at the granularity of an identity, and it is all Milestone 6 needs from the format. It does not catch two sandboxes that each create an identity for the same new company: both commit and leave a duplicate. Detecting that is conflict policy and is deferred.

Undo is a new changeset computed by the framework from the log and the membership intervals: the inverse of a merge reinstates the merged ID and moves its former members back, so anyone who stored the old ID gets the right entity again. History is never rewritten.

DuckDB as the log store has one measured cost: small transactions took about 5 ms each (five statements, durable file), so roughly 200 commits a second. That is fine for batch reconciliation and interactive corrections, and not suited to per-record streaming. Batching operations into fewer changesets is the answer for now.

**Decide now:** hybrid, per-operation preconditions, permanent redirects, undo as a forward changeset. **Defer:** snapshots and log compaction, sync transport and conflict policy, hash-chaining the log for tamper evidence.

## 4. Minimal data contracts that keep matchers interchangeable

Three relation shapes, each an Arrow schema:

| Relation | Columns | Produced by |
| --- | --- | --- |
| Features | `observation_id` plus the columns the entity type's declaration defines | Framework, from the declaration |
| Candidates | `left_id`, `right_id`, optional `rule` | Strategy, or the built-in blocker |
| Judgements | `left_id`, `right_id`, `verdict`, optional `score`, optional `detail` | Strategy |

Plus one rule about scope: a run is told which observations are new, and it must judge new-versus-existing and new-versus-new pairs only. The engine rejects judgements that name unknown observations.

Candidates and judgements have the same shape for every entity type. Features do not: their columns come from the type's declaration. A strategy is therefore written against a declaration's columns, and nothing in the engine or the contract fixes what those are.

A strategy is anything that fills the judgements relation. That covers a SQL statement, a Rust function, a Python model, an LLM call and a human reviewer. Blocking and scoring can be one step or two; the contract does not care. In Milestone 1 the single built-in strategy is a plain Rust function that runs SQL rules supplied as configuration (section 8). There is no `Matcher` trait until a second native strategy exists.

What the contract leaves out, on purpose:

- **Comparable scores.** A Splink match weight, a logistic probability and an LLM's "yes" are not on one scale. The verdict is the common currency and each strategy applies its own threshold.
- **Clusters.** A strategy that natively produces groups (dedupe, in-context LLM clustering) has its output expanded to pairwise `match` judgements by its adapter. Transitive closure is the reconciler's decision.
- **Identity IDs.** Strategies see observations. They can read current membership as an input feature and cannot write it.

The reconciler's default policy for Milestone 1 is conservative:

1. A new observation with `match` judgements to members of exactly one identity, and no `no_match` to that identity, is assigned to it.
2. New observations that match only each other form a new identity, unless the group contains a `no_match` pair or a `cannot_link` constraint, in which case it is left unassigned and flagged.
3. A new observation that matches members of two or more identities is left unassigned, and a "possible merge" review item is recorded.
4. Existing identities are never merged or split by judgements alone.

Rule 4 is what protects IDs from model upgrades: rerunning a better matcher over old data yields a list of suggested changes, not changes. The cost is that this greedy, incremental policy depends on arrival order, which is why order sensitivity is a benchmark metric in section 9.

Tradeoff: pairwise judgements fit entity-centric strategies awkwardly. An LLM asked "which of these five existing companies is this?" answers about an identity. For now its adapter emits a judgement against a member observation and records the reasoning in `detail`. Whether to add observation-to-identity judgements is an open question.

**Decide now:** the three relations, verdict as the contract, clustering only in the reconciler, no automatic merges of existing identities by default. **Defer:** a strategy trait, policy plug-ins, entity-centric judgements, calibrated probabilities.

## 5. Exposing the engine through napi-rs and PyO3

One Rust `Engine` type owns the database file and is the entire API. Bindings convert arguments and contain no domain logic.

```rust
impl Engine {
    pub fn open(path: &Path, options: Options) -> Result<Engine>;
    pub fn declare_entity_type(&self, declaration: &EntityTypeDeclaration) -> Result<DeclarationVersion>;
    pub fn ingest(&self, source: &str, entity_type: &str, rows: impl RecordBatchReader) -> Result<IngestReport>;
    pub fn begin_run(&self, strategy: StrategyRef, scope: Scope) -> Result<RunId>;
    pub fn candidates(&self, run: RunId) -> Result<impl RecordBatchReader>;   // pairs with both sides' features
    pub fn submit_judgements(&self, run: RunId, rows: impl RecordBatchReader) -> Result<u64>;
    pub fn reconcile(&self, run: RunId, policy: &Policy) -> Result<Draft>;
    pub fn commit(&self, changeset: Changeset) -> Result<Committed, Rejected>;
    pub fn resolve(&self, options: ResolveOptions) -> Result<ResolveReport>; // built-in strategy, default policy
    pub fn identity_of(&self, observation: ObservationId) -> Result<Option<IdentityId>>;
    pub fn current_id(&self, id: IdentityId) -> Result<IdentityId>;          // follows redirects
    pub fn query(&self, sql: &str) -> Result<impl RecordBatchReader>;        // read-only, against the views
}
```

- **Small values** (entity type declarations, changesets, drafts, reports, single lookups, configuration) cross as plain objects defined once with `serde`. napi-rs generates the TypeScript declarations; Python gets dataclass-like wrappers. The same serialised form is the log's exchange format and later the sync format, described by one versioned JSON Schema.
- **Bulk data** crosses as Arrow. In Python, `pyo3-arrow` 0.21 implements the Arrow PyCapsule interface, so polars, pandas, pyarrow and DuckDB relations are accepted without Spool depending on pyarrow (from its documentation; not tested). In Node, the plan is an Arrow IPC stream in a `Buffer`, read with `apache-arrow`. That is one copy and is also not tested.
- **IDs cross as strings** in the first version. DuckDB exports `UUID` columns to Arrow as strings by default (tested), which is also what JavaScript and pandas users expect.
- **Blocking work stays off the event loop and releases the GIL.** Every engine call that touches DuckDB is asynchronous in Node and releases the interpreter lock in Python.
- **Parity is tested by hash.** With a seeded ID generator, the same fixture must produce the same changeset `content_hash` from Rust, Node and Python. Only Rust computes observation IDs and content hashes.

Three packaging facts to plan around:

- `duckdb` 1.20000.2, `arrow` 60 and `napi` 3 require Rust 1.88, and `uuid` 1.29 requires 1.89. `pyo3-arrow` 0.21 pins `arrow` 60 and `pyo3` 0.30, while `arrow-pyarrow` 60 still wants `pyo3` 0.29. These crates must be upgraded as a set, pinned at the workspace root.
- The `duckdb` crate links a prebuilt shared library by default (12 s clean build when measured, but a 96 MB library that must ship beside the binary) or compiles DuckDB from source with `bundled` (about 6 min 40 s on 4 cores when measured; I did not get as far as linking or running that build). Use the prebuilt library for development and decide the release linking in Milestone 2.
- Bun's documentation says most Node-API modules work. napi-rs's WebAssembly target does not load in Bun or Deno. Test Bun in CI against the native addon and do not plan on WebAssembly.

Tradeoff: an engine-shaped API is less idiomatic than hand-designed SDKs. The TypeScript and Python packages add the idiomatic layer (accepting arrays of objects or data frames, async iteration) in their own language, on top of the same calls.

**Decide now:** one engine API, serde objects plus Arrow as the only two crossing forms, hash-based parity tests. **Defer:** release linking of DuckDB, Windows on ARM, WebAssembly, the idiomatic SDK surface beyond a walking skeleton.

## 6. Python and TypeScript extensions without embedded runtimes

The host language owns the loop, and the engine is called, never calling back:

```python
run = spool.begin_run(strategy="my-matcher@1")
for pairs in spool.candidates(run):          # Arrow batches: left_id, right_id, left_*, right_*
    spool.submit_judgements(run, my_model.score(pairs))
draft = spool.reconcile(run)
spool.commit(draft)
```

There are four places an extension plugs in. The first is a declaration, and the other three are "produce a relation":

1. **Entity types.** A declaration is a plain object whose feature expressions are SQL text, so it is written the same way from all three languages.
2. **Observations.** Extractors and connectors run in the host language and call `ingest`. An LLM extraction pipeline is an upstream producer; the engine does not know it exists.
3. **Candidates.** Either SQL text handed to the engine, which is portable across all three languages, or pairs computed by the host, such as nearest neighbours from an embedding index.
4. **Judgements.** Anything, as in the loop above.

Reconciliation policy is configuration in the first versions. Someone who needs custom logic can author a changeset directly and it will be validated like any other.

Consequences to accept:

- The Rust CLI can run only native and SQL strategies. Benchmarking a Python matcher means starting the run from Python with the same benchmark specification. This is the price of not embedding runtimes, and I think it is right for now.
- Only one process can have a sandbox file open. A studio or a second tool has to talk to the engine process and cannot read the file beside it. DuckDB 2.0's Quack protocol is the likely route when that matters.
- A user who opens the same file with the Python `duckdb` package inside the engine's process may bypass the file lock, because two copies of the library share one process. I have not tested this. `Engine.query` exists so nobody needs to.

**Decide now:** host-driven loop, no callbacks from the engine, SQL text as the portable way to push logic down. **Defer:** an out-of-process worker protocol (Arrow over stdio) for CLI-driven polyglot runs, policy plug-ins, multi-process access.

## 7. Arrow, DuckDB relations, or typed objects

| Use | Form | Reason |
| --- | --- | --- |
| Entity type declarations, changesets, drafts, review items, reports, configuration, single lookups | Typed objects | Small, structured, and they need schemas and good error messages |
| Ingesting observations, pulling candidates, pushing judgements, exports, benchmark truth | Arrow record batch streams | Columnar and streamable; zero-copy into Python, one copy into Node |
| Blocking, comparison, aggregation, metrics | DuckDB SQL inside the engine | DuckDB already does it; string similarity functions are built in |
| Datasets at rest, benchmark fixtures, later sync bundles | Parquet | Portable, and `COPY ... TO` works in the stock library |
| Ad hoc inspection | SQL text in, Arrow out, against documented views | Gives SQL users everything without leaking a connection |

The rule of thumb for SDK authors: accept plain objects for convenience up to some thousands of rows, convert them once in Rust, and document the Arrow path for anything larger. Simple examples never mention Arrow or DuckDB.

A local test confirmed the internal half of this: an Arrow batch appended into a table, a blocking join with `jaro_winkler_similarity` read back as Arrow, and an in-memory Arrow batch joined against a stored table without first loading it into a table. That last one is how submitted judgements get validated against known observations.

**Decide now:** relations never cross the API. **Defer:** binary UUID columns in Arrow, a zero-copy path for Node.

## 8. Integrating existing algorithms without leaking their assumptions

Every external algorithm sits behind an adapter that ends at the judgements relation. Three things are stopped at that boundary:

| Leak | Example | Stopped by |
| --- | --- | --- |
| The tool's entity IDs | Splink `cluster_id`, Zingg `Z_CLUSTER`, dedupe cluster numbers | Adapters return pairs only; identity comes from the reconciler |
| The tool's score scale | Match weight, probability, similarity | The adapter applies the tool's own threshold and reports a verdict; the raw score goes in `score` and `detail` |
| Transitivity | Connected components inside the tool | The adapter never calls the tool's clustering step, or expands its clusters back into pairs |

How specific tools would attach:

- **Splink.** A Python adapter exports features as Arrow, calls `predict` (or `predict_between` for new-versus-existing, which is marked experimental), and maps match weights to verdicts with the per-comparison levels in `detail`. Its model file is an opaque artefact referenced from `run.config`. It is the best candidate for the second strategy in Milestone 4 because it exercises the Python extension path and gives a credible baseline.
- **LLM matchers.** Verdict plus rationale, with `unsure` used freely. Selection-style prompts become one `match` and several `no_match` judgements.
- **dedupe.** Use its pair scores; ignore its clustering.
- **Zingg.** AGPL and Spark, so out of process only: read a Parquet export, write pairs. Low priority.

Training data, model state and thresholds belong to the strategy. The framework records the configuration hash in `run` so any judgement can be traced to the exact model that made it.

The built-in strategy for Milestone 1 is deliberately plain, and it knows nothing about any entity type. It is a rule runner. Its configuration is three sets of SQL predicates over a left row `l` and a right row `r` of the type's feature table:

- `block` predicates generate the candidate pairs.
- A candidate for which any `no_match` predicate holds is judged `no_match`.
- Otherwise, a candidate for which any `match` predicate holds is judged `match`.

Other candidates get no judgement. The names of the predicates that held go in `detail`. For the example company declaration in section 1, a rule set could be:

```toml
block = [
  "l.domain = r.domain",
  "l.registry_id = r.registry_id",
  "l.name_norm[:4] = r.name_norm[:4]",
]

[no_match]
different_registry_id = "l.registry_id <> r.registry_id"

[match]
same_domain = "l.domain = r.domain"
similar_name_same_country = "jaro_winkler_similarity(l.name_norm, r.name_norm) >= 0.92 AND l.country = r.country"
```

A person type has its own columns and its own rules, and runs through the same code. Running SQL predicates keeps to Spool's non-goal of writing custom Rust implementations of algorithms DuckDB already handles efficiently. A native Fellegi-Sunter model is a later choice, not a prerequisite.

**Decide now:** adapters end at judgements; the built-in strategy takes its rules as configuration and contains none. **Defer:** which adapters to ship, and whether Spool ever implements a probabilistic model natively.

## 9. Benchmarking resolution quality from the beginning

The benchmark runs the ordinary path (ingest, match, reconcile, commit) and scores the committed state against a truth relation `(observation_id, true_entity)`. Nothing is special-cased, so a Rust, Python or TypeScript strategy is scored identically.

A benchmark is a **scenario**: an ordered list of steps (ingest this batch, apply this correction, switch to this strategy) with truth at each step. A single-batch dataset is a one-step scenario. The specification is a data file, not code.

Resolution metrics, per step:

- Blocking: pair completeness and reduction ratio.
- Pairs: precision and recall, reported separately. F1 is shown but is not the headline, for the reasons in the [survey](landscape-survey.md#5-benchmark-datasets-and-evaluation-practice).
- Clusters: B-cubed precision and recall, the count of false merges (identities containing more than one true entity) and false splits (true entities spread over more than one identity), and the share of observations left unassigned.

Identity metrics, across steps. These are the ones no public benchmark has:

| Metric | Definition |
| --- | --- |
| ID stability | Of the observations present before and after a step, the share whose identity ID is unchanged. Reported strictly and after following redirects. |
| Churn | Identities created, merged and observations reassigned per thousand new observations |
| Order sensitivity | Agreement between final partitions when the same batches arrive in different orders |
| Upgrade stability | After switching strategy on the same data: IDs retained, and suggested changes versus applied changes |
| Split recovery | After a scripted wrong merge and a manual split, whether later batches keep the two sides apart |
| Override persistence | Share of constraints still honoured after further batches and a strategy change |

Datasets for day one: Splink's `fake_1000` as a smoke test, `historical_50k` and FEBRL as mid-sized checks, and a small hand-built company fixture committed to the repository that contains the hard cases of company resolution (aliases, legal versus trading names, domain changes, subsidiaries, stealth companies), split into two batches. Each fixture carries its own entity type declaration and rules. The three public datasets are person records and the fixture is company records, so the benchmark runs two unrelated entity types through one engine from the start. The public datasets are downloaded and checksummed at benchmark time, because I have not confirmed their redistribution terms.

Results are written as JSON. CI compares them with a committed baseline and fails on a regression, with false merges and ID stability treated as the strictest gates.

For a real company benchmark, the [GLEIF register](landscape-survey.md#5-benchmark-datasets-and-evaluation-practice) is the best public source I found: CC0, LEI as ground truth, real former and trading names, parent links for hard negatives, and 6,521 records flagged as duplicates of another LEI. Building a scenario generator on it is Milestone 3 work.

Metrics are implemented in Spool. The existing `er-evaluation` package is AGPL.

**Decide now:** truth relation, scenario format, false merges and ID stability as first-class metrics, baseline comparison in CI. **Defer:** the GLEIF scenario generator, sampling-based estimation for unlabeled data, extraction and end-to-end suites, cost and latency reporting.

## 10. The smallest useful end-to-end implementation

Milestone 1 is a Rust library, a test suite, and a thin command that runs one scenario.

```text
spool/
├── Cargo.toml                 # workspace; edition 2024; rust-version 1.89
├── crates/
│   ├── spool-core/            # IDs, Op, Changeset, validate, reconcile, in-memory state
│   └── spool/                 # Engine, DuckDB store, ingest, built-in SQL strategy, metrics; `spool` binary
└── benchmarks/
    ├── datasets.toml          # name, url, sha256, licence
    └── identity/companies-small/   # type declaration, rules, two CSV batches, truth, scenario file
```

Milestone 2 adds `crates/spool-node`, `crates/spool-python`, `packages/sdk` and `python/spool`. `spool-duckdb` is split out when a second store exists, `spool-resolution` when a second native strategy exists, and `spool-cli` when the command needs dependencies the library should not carry.

What it does:

1. Open a database file and create the schema with a format version.
2. Declare the fixture's entity type from its declaration file, which generates the feature table.
3. Ingest batch one from CSV: content-hashed observations and their feature rows.
4. Run the built-in SQL strategy with the fixture's rules to produce candidates and judgements.
5. Reconcile into a draft changeset, validate it, commit it, and print the identities.
6. Ingest batch two, which contains exact re-sends, new records for known companies, and new companies. Repeat steps 4 and 5.
7. Report pair precision and recall, B-cubed, false merges and ID stability against the fixture's truth.

Exit criteria, each a test:

- The `fake_1000` person data runs through the same binary as the company fixture, with only the declaration and the rules differing.
- Re-sent rows create no observations and no operations.
- Every identity ID from batch one still exists after batch two, with the same members plus any additions.
- A subsidiary with a different registry identifier is not attached to its parent.
- An observation matching two identities is left unassigned with a review item.
- Rebuilding the projection from the log reproduces the state exactly.
- The in-memory state and the DuckDB store pass the same conformance suite.
- Two runs with the same seed produce the same changeset content hashes.
- A changeset with a stale precondition is rejected and names the conflicting operation.

In scope for the types but not implemented: `Merge`, `Constrain`, `Unconstrain`, `Reinstate`. Out of scope: bindings, host-computed feature columns, changing a declaration after data exists, superseding observations, probabilistic or LLM matching, extraction, relationships, canonical attribute views, sync, the studio.

Suggested reshaping of the later milestones: Milestone 2 is a walking skeleton of the bindings that includes submitting judgements and builds wheels and addons in CI, and it should not block Milestone 3. Milestone 3 implements merge, split, constraints, undo and superseding observations, plus the identity benchmark. Milestone 4 adds the Splink adapter as the second strategy.

## Decisions at a glance

| Decide now | Defer |
| --- | --- |
| Evidence versus identity-state split; `spool-core` without DuckDB or Arrow | A Postgres store; any abstraction over evidence storage |
| Entity types declared by the user; feature tables generated from the declaration; rules as configuration; no built-in types | Host-computed feature columns; rebuilding after a declaration changes; SDK types generated from a declaration |
| Judgements and draft changesets in place of proposals | Entity-centric judgements; calibrated probabilities |
| Minted UUIDv7 identity IDs, permanent redirects, content-derived observation IDs | Public ID formatting; binary IDs in Arrow |
| Changeset log as truth, projection in the same transaction, per-operation preconditions | Sync transport, conflict policy, compaction, hash chaining |
| Operation set: create, assign, merge, constrain, unconstrain, reinstate | Implementing all but create and assign until Milestone 3 |
| Default policy never merges or splits existing identities automatically | Policy plug-ins; automatic merges above a threshold |
| One engine API; serde objects and Arrow as the only crossing forms; no callbacks into host languages | Out-of-process strategy protocol; multi-process access |
| Scenario-based benchmark with identity metrics and a CI baseline | GLEIF scenario generator; extraction and end-to-end suites |
| Two crates; injected ID generator and clock; format version in the file | Further crate splits; release linking of DuckDB; indexes and keys |
