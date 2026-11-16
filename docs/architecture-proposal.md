# Spool architecture proposal: initial prototype

A minimal Rust-first design for the first Spool prototype, grounded in the [landscape survey](landscape-survey.md). Dated 2026-11-13, revised 2026-11-16. Nothing here is built. Where the text says something was tested or measured, that was a throwaway local experiment with DuckDB 2.0.0, through its Rust bindings or its Python package, not a benchmark.

Spool's initial roadmap had seven milestones: (1) a Rust engine on DuckDB, (2) TypeScript and Python bindings, (3) the identity lifecycle across successive batches, (4) interchangeable resolution strategies, (5) extraction from unstructured documents, (6) sandbox synchronisation with a shared store, and (7) developer tooling. This proposal changes that roadmap. Unless the text says "initial", "Milestone N" refers to the proposed roadmap at the end of section 10.

## Summary of conclusions

1. **Split the system into identity state and evidence.** Identity state (identities, which observation belongs to which identity, manual constraints, the log of changes) is small, typed and transactional, and its rules live in a pure Rust crate. Evidence (observations, candidate pairs, match judgements) is large and analytical, and lives in DuckDB tables that strategies query with SQL. Only identity state needs to be independent of DuckDB.
2. **Replace a single "proposal" concept with two sharper things.** Strategies emit *judgements*: pairwise evidence with a verdict of match, no match or unsure. A framework-owned *reconciler* turns judgements plus current state into a *draft changeset*. Extensions never write identity operations, which makes Spool's central invariant, that extensions may propose identity changes but only the framework commits them, enforceable by construction.
3. **Identity IDs are minted, never derived.** A UUIDv7 is assigned once, is never reused, and a merged ID redirects to its survivor permanently. By default the matcher can attach new observations to existing identities and create new ones, and can only *suggest* merging or splitting existing ones. That is what makes the design goal that a better matching model must not change existing IDs hold.
4. **History is a hybrid.** An append-only changeset log is the source of truth. Current-state tables are a projection written in the same transaction and rebuildable from the log. Every operation carries its own precondition, so a draft that waited for review is checked against the state it is committed to.
5. **The matcher contract is a table shape, not a trait.** Observations in, `(left, right, verdict, score, detail)` out. Blocking, scoring, Splink, an LLM and a human reviewer all fit it. Clustering happens only in the reconciler.
6. **Spool runs as one server, and clients are thin.** The engine is a Rust library, and `spool serve` wraps it in an HTTP API. The server owns the only store, so nothing is synchronised between copies. A client sends JSON for small things and Arrow for bulk, and contains no engine code, so there are no native bindings to build.
7. **A Spool project is a directory.** Entity types, features and rules are files, and names come from paths. `spool build` compiles the directory into a manifest and writes a Dockerfile. The result is one container and one volume.
8. **User code runs as child processes, never inside the engine.** Python and TypeScript files in the project run through `uv` and `bun`, and exchange Parquet files with the server. They produce evidence only. The workers are a bounded pool inside the one service.
9. **Entity types are declared, never built in.** Spool defines no company, person or any other type, and no two users have to agree on what a company looks like. Companies appear in this document as the worked example and as the first benchmark fixture, nowhere in the engine.
10. **Start with two crates.** `spool-core` (no DuckDB, no Arrow) and `spool` (engine, DuckDB store, project discovery, built-in strategy, metrics). Add the server in Milestone 2 and split further only when a second implementation of something exists.
11. **Milestone 1 must ingest two batches.** A single batch cannot show that IDs persist, which is the only thing that separates Spool from Splink. Changesets also have to exist in Milestone 1 because they are the write path.

## Where this departs from the initial design goals

Spool's initial design goals and roadmap are the starting point for this proposal. It keeps most of them and departs from the following.

| Initial goal or assumption | Position | Why |
| --- | --- | --- |
| Three core abstractions: observations, proposals and changesets | Keep observations and changesets. Split proposals into judgements (evidence from strategies) and draft changesets (intent from the reconciler). | "Proposal" mixes what a matcher saw with what should change. A matcher that proposes a merge has to know identity state and operation semantics, which is the coupling Spool's design goals set out to avoid. nomenklatura already works this way ([survey, section 4](landscape-survey.md#nomenklatura-and-opensanctions)). |
| Local-first and sandbox-native: each sandbox resolves entities locally and synchronises changesets to a shared store (initial Milestone 6) | One server owns the store and does the work. Applications and agents call it. Nothing is synchronised between copies. | Matching a new record needs the existing evidence, so every sandbox would need a copy of it. While one process has a DuckDB file open, no other process can open it. One writer removes sync transport, conflict policy and duplicates created by two copies at once. Local development stays: `spool dev` is the same server on localhost. Section 5 has the full argument. |
| TypeScript and Python are first-class interfaces to the engine through napi-rs and PyO3 bindings (initial Milestone 2) | No native bindings. A TypeScript client, and later a Python client, call the server's API. Python and TypeScript user code runs as child processes of the server. | With one server, a caller needs no engine in its own process. Bindings need native packages per platform and a matched set of crate versions: `pyo3-arrow` 0.21 pins `arrow` 60 and `pyo3` 0.30, while `arrow-pyarrow` 60 still wants `pyo3` 0.29. A client is a small package with no native code. |
| The Rust engine does not run Python or JavaScript; the language SDKs orchestrate extensions written in their language | The engine still embeds no runtime. The server starts user code as child processes and exchanges files with them. | A server has no host language to drive the loop. Child processes keep every runtime out of the engine, and one file protocol serves any language (section 6). |
| An eventual layout of seven crates: core, DuckDB store, resolution, benchmarks, Node bindings, Python bindings and CLI | Two crates for Milestone 1, three after Milestone 2. | The initial plan treats these as logical boundaries and allows starting with fewer crates. The built-in strategy is SQL run through the store and the benchmark needs both, so `spool-duckdb`, `spool-resolution` and `spool-bench` would have no independent boundary yet. |
| Core identity semantics are independent of any particular database | Apply it to identity state only. Let evidence handling be openly DuckDB-specific. | Abstracting the analytical engine means a SQL dialect layer. Splink carries `sqlglot` and per-backend code for this. A second store for identity state would need the log and the projection, not candidate generation. |
| Initial Milestone 1 creates identities; initial Milestone 3 preserves them across batches | Move "second batch keeps its IDs" into Milestone 1. | Otherwise Milestone 1 validates nothing that Splink plus a UUID column would not. |
| Initial Milestone 6 defines identity changesets | Define the changeset format and preconditions in Milestone 1. | Changesets are the only way state changes. Retrofitting preconditions and stable observation IDs after data exists is a format migration. |
| DuckDB relations are one of the cross-language data representations | Relations stay inside the engine. SQL text goes in and Arrow comes out. | A relation is a handle into one process's DuckDB instance. A second process cannot open the file at all while the engine holds it, even read-only (tested). |
| The core owns canonical entity representations | Core owns *which* observations form an identity. Canonical attributes are derived views over evidence. | Storing a golden record as identity state makes every attribute rule an identity operation and works against the goal that evidence stays traceable. Relationships and temporal attributes are derived the same way. |
| Persistent identity is an unoccupied space in open source | It is partly occupied. | nomenklatura, GoldenMatch and Zingg Enterprise each do part of it. The gap is narrower and is described in the [survey](landscape-survey.md#7-borrow-avoid-and-the-gap). |

## The model

Five concepts. The first two are evidence, the last three are identity state.

- **Observation.** One immutable record of something a source said about one entity: a CRM row, or a company mention extracted from a PDF report. Its ID is derived from its source, its key within that source and its content, so ingesting the same thing twice is a no-op. The key is a row key, or the locator of an extracted mention, so two mentions at different places in one document are two observations. A changed source row is a new observation that names the one it supersedes.
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

## The project directory

A Spool project is a directory in a git repository. It defines the entity types, how they are matched, and how the server is built. The conventions follow eve, a framework in which an agent is a directory of files ([agent files](https://eve.dev/docs/reference/agent-files), [deployment](https://eve.dev/docs/guides/deployment)):

- **The path is the name.** `entities/company/` declares the entity type `company`, and `rules/match/same_domain.sql` is the rule `same_domain`. Nothing is registered by hand.
- **Data files first.** SQL for features and rules, TOML for settings, Markdown for instructions. Code is used only where data cannot say it.
- **A file replaces the default at the same path.** A project without `policy.toml` gets the default reconciliation policy.
- **A build step compiles the directory into one artefact**, and a command lists what was discovered.

```text
my-project/
├── spool.toml                    # optional project settings
├── entities/
│   └── company/                  # the directory name is the entity type
│       ├── features.sql          # one query; its columns are the feature table (section 1)
│       ├── rules/
│       │   ├── block/*.sql       # candidate rules, one predicate per file (section 8)
│       │   ├── match/*.sql
│       │   └── no_match/*.sql
│       ├── policy.toml           # replaces the default reconciliation policy
│       ├── strategies/           # user code: splink.py, llm.ts (Milestone 4, section 6)
│       ├── extract.md            # instructions for LLM extraction (Milestone 5)
│       └── review.md             # guidance for whoever reviews this type's drafts (later)
├── benchmarks/
│   └── companies-small/          # a scenario; the path is its name (section 9)
├── Dockerfile                    # written by `spool build`, then owned by the project
└── .spool/                       # compiled manifest and the development database; not committed
```

A project with no `entities/company/` has no company type. A project that resolves people, products or papers has those directories.

| Command | What it does |
| --- | --- |
| `spool init` | Scaffolds the directory. |
| `spool dev` | Runs the server on localhost against a development database under `.spool/`, and applies file changes as they are saved. |
| `spool info` | Lists the discovered entity types, rules and strategies, with diagnostics. |
| `spool build` | Validates the project and compiles it into a manifest. Every SQL file is prepared against DuckDB, so a mistake fails the build, not a run. Writes a `Dockerfile` if the project has none. |
| `spool apply` | Applies a manifest to a store: declares entity types and rebuilds the feature tables whose declaration changed. |
| `spool serve` | Runs the server from a manifest and a store location. An empty store is set up from the manifest. |
| `spool bench` | Runs benchmark scenarios in-process (section 9). |
| `spool schema` | Prints the JSON Schema of the API's values (section 5). |

Three rules keep deployment simple:

1. **The directory is build-time input.** The engine reads only the manifest. The manifest has a content hash that is recorded with every run, so any judgement can be traced to the project version that produced it.
2. **A project of data files needs only the `spool` binary.** Discovery and the build are in Rust, so no Node or Python is involved. Only a project with user code adds `uv` or `bun` to its image.
3. **State is never in the image.** The image holds the binary, the manifest, and any user code with its dependencies. The store is a file on a volume, and its location is configuration.

The generated Dockerfile starts from the Spool base image, copies the project, compiles the manifest, declares a volume for the store at `/data` and runs `spool serve` as a user that is not root. When user code exists (Milestone 4) it will also install that code's dependencies from their lockfiles. It belongs to the project and can be edited: `spool build` writes it once and never again. The result is one container and one volume, run wherever containers run.

The base image is built from the `Dockerfile` at the root of this repository. It holds the `spool` binary in `/usr/local/bin` and the DuckDB shared library in `/usr/local/lib`, where the binary looks for it. The generated Dockerfile names `ghcr.io/alexpatow/spool:<version>`, and `spool build --base-image` names another. No image is published yet, so for now the base image is built locally under that name.

Applying a changed definition to an existing store is an explicit step, never a side effect of a container starting. It rebuilds feature tables, which can take time. It cannot change an identity (section 1). `spool serve` refuses to start on a store whose applied manifest differs from its own, and says which `spool apply` to run. A store that has never had a manifest applied is the one exception: there is nothing to rebuild and no identity to protect, so `serve` applies its manifest and starts. A first deploy is then one container start, with no separate step against an empty volume.

Tradeoff: the file layout becomes an interface that has to be documented and kept stable, and the model cannot be changed through the API. Changing an entity type means a build and a deploy. `spool dev` covers fast iteration, and `spool info` shows what the conventions resolved to.

**Decide now:** a project directory with path-derived names, a manifest as the engine's only input, discovery and build in the Rust binary, state outside the image, an explicit apply step. **Defer:** packages of entity types that a project can mount, the Markdown instruction files, settings in `spool.toml` beyond the minimum.

## 1. DuckDB representation of observations, candidates and identity state

All tables live in a `spool` schema so a user's own tables can share the file. The column types used below or available to feature columns (`UUID`, `JSON`, `STRUCT`, `LIST`, `MAP`) and the `uuidv7()` function are all in the stock DuckDB library (tested).

Evidence:

```sql
CREATE TABLE spool.observation (
  observation_id UUID        NOT NULL, -- first 128 bits of sha256(source_id, source_key, canonical payload)
  entity_type    VARCHAR     NOT NULL, -- a declared type, for example 'company'
  source_id      VARCHAR     NOT NULL, -- 'crm', 'registry', 'report:acme-annual-2025.pdf'
  source_key     VARCHAR,              -- key within the source: a row key, or an extracted mention's locator
  supersedes     UUID,                 -- previous version of the same (source_id, source_key)
  payload        JSON        NOT NULL, -- attributes exactly as received
  evidence       JSON,                 -- locator: uri, page, character span, extractor and version
  observed_at    TIMESTAMPTZ,          -- when the source says it held, if known
  ingested_at    TIMESTAMPTZ NOT NULL,
  ingest_run     BIGINT      NOT NULL
);

CREATE TABLE spool.entity_type (          -- declared by the project; the engine ships none
  entity_type      VARCHAR     NOT NULL,  -- any name: 'company', 'person', 'product', ...
  version          INTEGER     NOT NULL,  -- increases when the declaration changes
  declaration      JSON        NOT NULL,  -- the features query and the columns inferred from it
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

CREATE TABLE spool.run (                  -- also the task record a client reads (section 5)
  run_id BIGINT NOT NULL, kind VARCHAR NOT NULL,   -- 'ingest' | 'match' | 'reconcile'
  status VARCHAR NOT NULL,                         -- 'running' | 'open' | 'drafted' | 'succeeded' | 'failed'
  strategy VARCHAR, strategy_version VARCHAR, config JSON, config_hash VARCHAR,
  engine_version VARCHAR NOT NULL, started_at TIMESTAMPTZ NOT NULL, finished_at TIMESTAMPTZ,
  error VARCHAR,                                   -- failed: why
  result JSON                                      -- what the run reported: an ingest's counts, a match run's resolution
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

CREATE TABLE spool.review_item (          -- what a reconcile run would not decide; kept
  review_id       BIGINT  NOT NULL,       -- dense, assigned by the engine
  run_id          BIGINT  NOT NULL,       -- reconcile run that raised it
  entity_type     VARCHAR NOT NULL,
  kind            VARCHAR NOT NULL,       -- 'possible_merge' | 'conflict'
  observation_ids UUID[]  NOT NULL,       -- the observations left without an identity
  identity_ids    UUID[]  NOT NULL,       -- the identities they matched; empty when the conflict is among the observations
  status          VARCHAR NOT NULL,       -- 'open' | 'resolved' | 'superseded'
  closed_seq      BIGINT,                 -- resolved: the changeset after which every observation had an identity
  closed_run      BIGINT                  -- superseded: the reconcile run that raised it again in another form
);

CREATE TABLE spool.draft (                -- a changeset a reconcile run drafted and left for review; kept
  changeset_id  UUID    NOT NULL,         -- the draft's handle, and the ID it is committed under
  run_id        BIGINT  NOT NULL,         -- reconcile run that drafted it
  match_run     BIGINT  NOT NULL,         -- match run whose judgements it rests on
  entity_type   VARCHAR NOT NULL,
  base_seq      BIGINT  NOT NULL,         -- log position it was drafted against
  actor VARCHAR NOT NULL, reason VARCHAR,
  status        VARCHAR NOT NULL,         -- 'open' | 'committed' | 'rejected' | 'superseded'
  closed_seq    BIGINT,                   -- committed: its log position
  closed_run    BIGINT,                   -- superseded: the reconcile run that drafted the type again
  closed_at TIMESTAMPTZ, closed_reason VARCHAR     -- rejected: when, and why if the reviewer said
);
-- spool.draft_op holds a draft's operations in the shape of spool.changeset_op below.
```

`spool.features_company` above is not part of Spool. It is what the engine generates from this file, which is one project's idea of a company:

```sql
-- entities/company/features.sql
SELECT
  observation_id,
  lower(trim(payload->>'name')) AS name_norm,
  lower(payload->>'domain')     AS domain,
  upper(payload->>'country')    AS country,
  payload->>'registry_id'       AS registry_id
FROM observation
```

`observation` in this file is the engine's view of the type's observations. The query must return `observation_id` and at most one row per observation, and the engine checks both. DuckDB infers the column names and types from the query, so the file is the whole declaration (tested). Another project's `company` can have different columns.

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
- **An extracted mention's key is its locator.** `source_key` holds the mention's position in its document, such as the page and character span. Two identical mentions at different positions are then two observations, and each keeps its own `evidence`. Repeating the same extraction produces the same IDs and is a no-op. `ingest` rejects an observation whose `evidence` has a locator and whose `source_key` is empty. The cost is that a document which names one entity many times yields that many observations, which all match each other.
- **Feature tables are generated from a declaration, never hand-written.** A declaration is one SQL query over the type's observations. The engine creates `spool.features_<entity_type>` from its result and fills it with one `INSERT ... SELECT` per ingest. No engine code names a feature column, and `ingest` rejects a type that has not been declared.
- **Changing a declaration changes no identity.** Applying a changed query for an existing type increases its version and rebuilds the feature table from the stored payloads. Identity state refers to observations and to the type's name, never to feature columns, so every identity ID stays in place. A match run records the declaration version it read, so its judgements stay traceable.
- **Typed columns, not one generic table.** A single features table holding JSON for every type would need no generation. It would also move normalisation into every matching query and give up the typed columns that blocking joins and similarity functions work on.
- **Operations are rows, not a JSON blob.** A first load is one changeset with as many `assign` rows as observations, applied with `INSERT ... SELECT`. When measured, a one-million-operation changeset wrote its log rows and its projection rows in one transaction in 0.4 s.
- **Membership keeps intervals.** "What was this identity at log position N" is a filter, and undoing a merge can read which observations it moved.
- **No primary keys or foreign keys on the large tables at first.** Uniqueness is guaranteed by the Rust validator and by anti-join inserts, and checked by a `verify` routine in tests. DuckDB constraints did behave correctly when tested (including update and delete-then-insert of a keyed row in one transaction), but index maintenance on bulk loads is a cost that has not been measured, and the identity rules must not depend on the database.
- **Review items are rows, and they are evidence.** `spool.review_item` holds what a reconcile run would not decide, so the question outlives the call that raised it. The table is not identity state: it is not in the log, no changeset mentions it, and replaying the log does not need it. The engine keeps `status` in step with membership inside the transaction that commits a changeset. Section 4 says when an item opens and closes.
- **A draft left for review is a row too.** `spool.draft` holds a changeset that a reconcile run drafted and nobody has committed yet, with its operations in `spool.draft_op`. Like a review item it is not identity state: the log does not mention it until it is committed, and then under the same changeset ID. A match run whose draft waits has the status `drafted`, not `succeeded`. What is new to a run is counted from the last run of the type that succeeded, so the observations of a draft that is rejected, replaced or never decided are examined again by the next run. A type has at most one open draft: drafting it again closes the earlier one as superseded.
- **A run that the engine was working on when it stopped is marked failed.** A run is `running` only while a call is inside the engine. Opening a store marks any run it finds `running` as `failed`, with an error that says it was interrupted, so a client reading the run sees an end.
- **Public read surface is views.** `spool.identity_current`, `spool.membership_current`, `spool.review_open` and a generated per-type entity view are documented and stable; base tables are not.

Tradeoffs of declared types:

- A SQL query cannot express every normaliser. Reducing a URL to its registrable domain needs the public suffix list, for example. Feature columns computed by user code (section 6) cover that case and come with Milestone 4.
- The query must be deterministic, or the table is not rebuildable. The engine cannot check that in general.
- The type's name becomes part of a table name, so the engine restricts directory names to a fixed pattern. The query is SQL that the engine runs. It comes only from the project directory, which is built into the server image; the API accepts no declarations.
- Feature columns differ per type, so a client cannot offer static types for feature rows until it generates them from a manifest.

Tradeoff: two write targets per commit (log and projection) and no database-level safety net. Mitigation is a standing test that the projection equals a replay of the log. A simpler log-versus-projection comparison over a million rows took 0.05 s when measured.

**Decide now:** the evidence versus identity-state split, content-derived observation IDs, extracted mentions keyed by their locator, entity types declared by the project with versioned declarations and generated feature tables, operation rows, interval membership, review items kept in the store, a format version from the first commit. **Defer:** indexes and keys, feature columns computed by user code, rebuilding a feature table after its declaration changes (Milestone 2), retention of old candidates and judgements, how a retracted or deleted source record is represented, the locator format for extracted mentions (Milestone 5), a separate table of locators so that repeated mentions share one observation.

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
    /// Rejected if either identity's membership changed after the changeset's `base`.
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

The DuckDB store in the `spool` crate implements `IdentityState` and one write: apply a `Validated` changeset by inserting log rows and updating projections in a single transaction. One conformance suite runs against both the in-memory state and the DuckDB store, and property tests compare random operation sequences between the two. A second persistent store for identity state, such as Postgres, would be a third implementation that must pass the same suite.

Split is deliberately not an operation. It is `CreateIdentity` plus `Assign` moves, usually with a `cannot_link` constraint so the matcher does not undo it, in one changeset. The original ID stays with whichever side the author leaves in place; the default policy keeps it with the larger side, as OpenSanctions does.

Tradeoff: validating in Rust means a changeset must fit in memory, which caps one changeset at roughly millions of operations. Larger loads are chunked into several changesets. That is preferable to moving validation into SQL, which would put identity rules in the database.

**Decide now:** the dependency-free core crate, validated-changeset type, injected ID generator and clock (required for reproducible tests), operation set. **Defer:** implementing `Merge`, `Constrain` and `Reinstate` (Milestone 3), a Postgres store, any trait over evidence storage.

## 3. Event log, versioned state, or hybrid

**Hybrid, with the log as the source of truth.**

| Option | Gives | Costs |
| --- | --- | --- |
| Event log only | One truth, full audit, a natural unit of review and undo | Every read replays or needs a cache, which is a projection by another name |
| Versioned state only (interval rows) | Cheap time travel | No grouping of changes, no author or reason, no unit to review or undo, intent is lost |
| Hybrid | Fast reads, audit, review, undo | Two writes per commit and the risk they diverge |

Divergence is contained three ways: both writes share one transaction (tested: a rollback leaves both empty), the projection can be rebuilt from the log at any time, and replay equality is a standing test.

Each operation is its own precondition. `Assign.from` must equal the observation's current identity. `CreateIdentity` needs an unused ID. `Merge` needs both identities active and their membership unchanged since the changeset's `base_seq`, because a merge moves every member and its author saw only the members that existed then. The membership intervals answer that check without a new field. Commit checks these against current state and does not require `base_seq` to be the latest.

One server and one writer do not remove the need for this. A draft can wait for review while other runs commit, and two clients can author changesets at the same time. Drafts that touch different identities all commit cleanly, in whatever order they are approved. A stale one is rejected with the exact operation that conflicted.

Undo is a new changeset computed by the framework from the log and the membership intervals: the inverse of a merge reinstates the merged ID and moves its former members back, so anyone who stored the old ID gets the right entity again. History is never rewritten.

DuckDB as the log store has one measured cost: small transactions took about 5 ms each (five statements, durable file), so roughly 200 commits a second. That is fine for batch reconciliation and interactive corrections, and not suited to per-record streaming. Batching operations into fewer changesets is the answer for now.

**Decide now:** hybrid, per-operation preconditions, permanent redirects, undo as a forward changeset. **Defer:** snapshots and log compaction, hash-chaining the log for tamper evidence. **Not planned:** synchronising changesets between stores.

## 4. Minimal data contracts that keep matchers interchangeable

Three relation shapes, each an Arrow schema:

| Relation | Columns | Produced by |
| --- | --- | --- |
| Features | `observation_id` plus the columns the entity type's declaration defines | Framework, from the declaration |
| Candidates | `left_id`, `right_id`, optional `rule` | Strategy, or the built-in blocker |
| Judgements | `left_id`, `right_id`, `verdict`, optional `score`, optional `detail` | Strategy |

Plus one rule about scope: a run is told which observations are new, and it must judge new-versus-existing and new-versus-new pairs only. New means ingested since the last match run of the type that succeeded, plus the observations of open review items that still have no identity (below). When the engine matches, reconciles and commits in one call, it records the match run as succeeded only after the commit, so a call that is cut short leaves nothing behind: the next one examines the same observations again. The engine rejects judgements that name unknown observations.

Candidates and judgements have the same shape for every entity type. Features do not: their columns come from the type's declaration. A strategy is therefore written against a declaration's columns, and nothing in the engine or the contract fixes what those are.

A strategy is anything that fills the judgements relation. That covers a SQL statement, a Rust function, a Python model, an LLM call and a human reviewer. Blocking and scoring can be one step or two; the contract does not care. In Milestone 1 the single built-in strategy is a plain Rust function that runs the SQL rules in the project directory (section 8). There is no `Matcher` trait until a second native strategy exists.

What the contract leaves out, on purpose:

- **Comparable scores.** A Splink match weight, a logistic probability and an LLM's "yes" are not on one scale. The verdict is the common currency and each strategy applies its own threshold.
- **Clusters.** A strategy that natively produces groups (dedupe, in-context LLM clustering) has its output expanded to pairwise `match` judgements by its adapter. Transitive closure is the reconciler's decision.
- **Identity IDs.** Strategies see observations. They can read current membership as an input feature and cannot write it.

The reconciler's default policy for Milestone 1 is conservative:

1. A new observation with `match` judgements to members of exactly one identity, and no `no_match` to that identity, is assigned to it. Two new observations that would join the same identity this way, with a `no_match` between them, are both left unassigned and flagged: joining would put two things that were judged different into one identity.
2. New observations that match only each other form a new identity, unless the group contains a `no_match` pair or a `cannot_link` constraint, in which case it is left unassigned and flagged.
3. A new observation that matches members of two or more identities is left unassigned, and a "possible merge" review item is recorded.
4. Existing identities are never merged or split by judgements alone.

Rule 4 is what protects IDs from model upgrades: rerunning a better matcher over old data yields a list of suggested changes, not changes. The cost is that this greedy, incremental policy depends on arrival order, which is why order sensitivity is a benchmark metric in section 9.

Whenever the policy leaves observations unassigned, the reconcile run records a review item in `spool.review_item` (section 1). Its kind is `possible_merge` when the observations match members of two or more identities, and `conflict` when a `no_match` contradicts their matches. The item names the observations, the identities they matched and the run that raised it. It is evidence, like a judgement: it changes no identity, and nothing is committed on its account.

A review item is open while any of its observations has no identity. It closes in one of two ways:

- **Resolved.** A changeset gives the last of its observations an identity, and the item records that log position. Any changeset does this, whether a reviewer wrote it or a later run drafted it.
- **Superseded.** A later run examines the observations and raises an item that differs, for example because they now match a third identity. The old item records that run.

The observations of open review items stay in scope. Every later match run of their entity type examines them again, alongside what was newly ingested. An item is therefore reconsidered whenever state or rules may have changed: once a reviewer has moved the members of one identity into the other, or a rule has been corrected, the observation matches one identity and is assigned. A run that reaches the same conclusion leaves the item as it is. Assigning an observation that had no identity changes no existing ID, so this does not weaken rule 4.

This has two costs. A type with open review items gets a run even when nothing was ingested. And an observation under review that matches a later arrival holds that arrival back with it, exactly as it would have if the two had arrived together.

Tradeoff: pairwise judgements fit entity-centric strategies awkwardly. An LLM asked "which of these five existing companies is this?" answers about an identity. For now its adapter emits a judgement against a member observation and records the reasoning in `detail`. Whether to add observation-to-identity judgements is an open question.

**Decide now:** the three relations, verdict as the contract, clustering only in the reconciler, no automatic merges of existing identities by default, review items that persist and are examined again by later runs. **Defer:** a strategy trait, policy plug-ins, entity-centric judgements, calibrated probabilities, closing a review item without giving its observations an identity.

## 5. Exposing the engine: one server and thin clients

The engine is a Rust library. One `Engine` type owns the database file and is the entire API:

```rust
impl Engine {
    pub fn open(path: &Path, options: Options) -> Result<Engine>;
    pub fn apply(&self, manifest: &Manifest) -> Result<ApplyReport>;         // declares entity types, generates feature tables
    pub fn ingest(&self, source: &str, entity_type: &str, rows: impl RecordBatchReader) -> Result<IngestReport>;
    pub fn begin_run(&self, strategy: StrategyRef, scope: Scope) -> Result<RunId>;
    pub fn candidates(&self, run: RunId) -> Result<impl RecordBatchReader>;   // pairs with both sides' features
    pub fn submit_judgements(&self, run: RunId, rows: impl RecordBatchReader) -> Result<u64>;
    pub fn reconcile_with(&self, run: RunId, policy: &Policy, then: Disposition) -> Result<TypeResolution>; // commit, or leave for review
    pub fn commit(&self, changeset: Changeset) -> Result<Committed, Rejected>;
    pub fn approve_draft(&self, draft: ChangesetId) -> Result<Committed, Rejected>;
    pub fn reject_draft(&self, draft: ChangesetId, reason: Option<&str>) -> Result<DraftSummary>;
    pub fn resolve(&self, options: ResolveOptions) -> Result<ResolveReport>; // built-in strategy, default policy
    pub fn run(&self, run: RunId) -> Result<RunRecord>;                       // the task record
    pub fn identity_of(&self, observation: ObservationId) -> Result<Option<IdentityId>>;
    pub fn current_id(&self, id: IdentityId) -> Result<IdentityId>;          // follows redirects
    pub fn query(&self, sql: &str) -> Result<impl RecordBatchReader>;        // read-only, against the views
}
```

The listing is the core. The library also has typed reads for everything the API returns: runs, drafts, review items, identities with their members and history, observations, the log, and a summary of the store.

`spool serve` wraps that type in an HTTP API. The server is the only process that opens the store. Applications and agents reach it through a client, and Spool does not run inside the caller's process or sandbox. Tests and benchmarks use the library directly, with no server.

Why one server, and not an engine in each caller:

- **Matching needs the existing evidence.** Resolving a new record means comparing it with what is already known. An engine in each caller needs a copy of that evidence, and choosing which part to copy is the blocking problem again.
- **The server can be sized for the work.** A caller such as an agent sandbox may be small and short-lived. A network call to a larger machine is the better trade for analytical joins.
- **A DuckDB file has one owner.** While the engine has the file open, no other process can open it, even read-only (tested). One server that owns the file fits that.
- **One writer has nothing to synchronise.** Sync transport, conflict policy, and two copies each creating an identity for the same new entity all disappear.

Every path is under `/v1` and needs the bearer token, except `GET /healthz`. Small values are JSON. `spool schema` prints one JSON Schema for all of them, a copy is committed at `schema/spool-api.v1.schema.json`, and a test fails when the two differ.

| Call | What it does | Response |
| --- | --- | --- |
| `GET /v1/info` | What the store holds: the manifest hash, the head of the log, and counts per entity type | JSON |
| `POST /v1/ingest` | Adds rows as observations. The body is JSON with the rows as objects, or an Arrow stream with the other fields in the query string | The ingest run |
| `POST /v1/runs` | Matches the new observations of one entity type. With no strategy, the server runs the rules and reconciles, and `commit` says whether the draft is committed (the default) or left for review. With a strategy of the caller's own, the run opens and waits for judgements | The match run, or `null` when nothing was new |
| `GET /v1/runs`, `GET /v1/runs/{run}` | Lists runs; reads one, optionally waiting for it to end | Runs |
| `GET /v1/runs/{run}/candidates` | The candidate pairs of an open run with both sides' features | Arrow, or JSON when the `Accept` header asks for it |
| `POST /v1/runs/{run}/judgements` | Stores judgements, as Arrow or JSON | A count |
| `POST /v1/runs/{run}/reconcile` | Turns the run's judgements into a changeset and commits it, or leaves it as a draft with `commit: false` | The resolution: counts, review items, and the commit or the draft's ID |
| `POST /v1/runs/{run}/cancel` | Gives up an open run | The run |
| `GET /v1/drafts`, `GET /v1/drafts/{draft}` | Lists drafts; reads one with its changeset | JSON |
| `POST /v1/drafts/{draft}/approve`, `.../reject` | Commits a draft, or closes it | The commit, or the operations that were rejected; the draft |
| `GET /v1/changesets`, `POST /v1/changesets` | Reads the log in pages; commits a changeset the caller wrote | The log; the commit, or the operations that were rejected |
| `POST /v1/ids` | Mints identity and changeset IDs for a caller that writes its own changeset | JSON |
| `GET /v1/identities`, `GET /v1/identities/{id}`, `.../history` | Lists identities in pages; reads one with its members, following redirects; the changesets that touched it | JSON |
| `GET /v1/observations`, `GET /v1/observations/{id}` | Finds observations by source and key, or those with no identity; reads one | JSON |
| `POST /v1/observations/identities` | The identities of many observations in one call | JSON |
| `GET /v1/review` | Review items, typed | JSON |

- **A call that starts work says how long it will wait.** `POST /v1/ingest` and `POST /v1/runs` take `wait_ms`, 30 seconds if left out. If the run finishes within that time, the response is `200` with the finished run. Otherwise it is `202` with the run as far as it has got, and the client reads `GET /v1/runs/{run}?wait_ms=...` until its status is no longer `running`. The work goes on whether or not anyone waits. The run ID is `spool.run.run_id`, so tasks add no new concept. One limit: a call that finds the writer busy has no run to report until its own work starts, so it can wait longer than it asked to.
- **A run has one of five statuses.** `running` while the engine works on it, `open` while it waits for a caller's judgements, `drafted` while its draft waits for review, and then `succeeded` or `failed`.
- **A run either commits its draft or leaves it for review**, as the caller asks. A draft left for review is how an agent experiments without changing shared identities. The draft is stored on the server and named by its changeset ID, so whoever reviews it need not be whoever started the run. Approving it validates it like any changeset: if state moved under it, the response names the operations that no longer hold and the draft stays open. Section 1 says why a draft nobody commits does not strand its observations.
- **Small values are JSON**, defined once with `serde` and described by one versioned JSON Schema. Client types are generated from it. The same serialised form is the log's exchange format. A request with a field the server does not know is refused, so a misspelt `commit` cannot commit by mistake.
- **Bulk data is Arrow**, as IPC streams in request and response bodies. Clients also accept plain objects for convenience, up to some thousands of rows. Rows sent as JSON become the same observations as the same rows sent as Arrow: a number with no fractional part is stored as an integer whichever way it arrived, so an observation's ID does not depend on the encoding or on the other rows in its batch. A JSON integer that its column cannot hold exactly is refused, not rounded: one of 2^53 or more beside numbers with a fraction, or one past the signed 64-bit range beside a negative one. A JSON row with no values is still a row.
- **IDs cross as strings** in the first version. DuckDB exports `UUID` columns to Arrow as strings by default (tested), which is also what JavaScript and pandas users expect.
- **Errors have one shape.** Every response that is not a success is `{"error": {"code", "message"}}`, with `rejections` when a changeset broke a rule. The codes are `unauthorized`, `invalid`, `not_found`, `conflict`, `rejected`, `too_large` and `internal`.
- **The API is tested by hash.** With a seeded ID generator, the same fixture must produce the same changeset `content_hash` through the library, through the server with Arrow and with JSON bodies, and through the TypeScript client. Only Rust computes observation IDs and content hashes.

A client is a small package that speaks this API. The first is TypeScript, and a Python client is the same amount of work. No client contains engine code, so there are no native packages to build per platform. An agent built with a framework such as eve reaches Spool the same way: a tool in the agent calls the client.

Scope of the server: one store, one writer and one tenant. Access is a bearer token. It scales up, not out. Tenancy, user management and provisioning compute are out of scope, which keeps Spool short of a hosted control plane. Inside the server, reads run concurrently and commits are serialised, so the log's order is the order of commit. Reads use their own connections to the same DuckDB instance, so reading a run or looking up an identity does not wait for an ingest or a resolve in progress. Each read is one transaction, so it sees one committed state however many queries it takes.

Two packaging facts to plan around:

- `duckdb` 1.20000.2 and `arrow` 60 require Rust 1.88, and `uuid` 1.29 requires 1.89. Pin them at the workspace root and upgrade `duckdb` and `arrow` together.
- The `duckdb` crate links a prebuilt shared library by default (12 s clean build when measured, but a 96 MB library that must ship beside the binary) or compiles DuckDB from source with `bundled` (about 6 min 40 s on 4 cores when measured; that build was not linked or run). Decided in Milestone 2: the binary links the prebuilt shared library, in development and in the image. It finds the library through its own run path: beside itself, then in `../lib`, then where the build downloaded it. So a development build starts without `cargo run`, and the image puts the library in `/usr/local/lib`. The static build is not offered yet. The engine uses DuckDB's JSON and ICU extensions, which the prebuilt library includes and the `bundled` feature does not: a static build needs the crate's CMake build with both extensions, and has not been built or run.

Tradeoff: a server has to be run, backed up and secured, where a library needed none of that. Every call crosses the network, which is small next to matching work and costly for per-record lookups, so lookups need batch forms. The library remains for Rust callers and for tests.

**Decide now:** the engine as a library with the server as a wrapper, JSON and Arrow as the only two wire forms, the run ID as the task handle, one tenant and one writer, no native bindings. **Defer:** authentication beyond a bearer token, streaming a run's progress, a queue so that a call never waits for the writer before it has a run ID, a Python client, a statically linked DuckDB, a published base image, Windows on ARM.

## 6. Python and TypeScript user code without embedded runtimes

The engine embeds no Python or JavaScript runtime and never calls into one. When a project contains user code, the server runs it as a child process and exchanges files with it.

A user-code strategy is a file under `strategies/`. `entities/company/strategies/splink.py` is the strategy `splink` for companies. A `.py` file runs with `uv run` and a `.ts` file with `bun run`. Each tool is a single binary that runs the file and resolves its dependencies ([uv](https://docs.astral.sh/uv/guides/scripts/), [bun](https://bun.com/docs/runtime/typescript); from their documentation, not tested). `spool build` installs the dependencies into the image, so nothing is downloaded at run time.

The protocol is a directory of files per task:

```text
$SPOOL_TASK_DIR/
├── task.json             # run ID, entity type, strategy name, declaration version
├── candidates.parquet    # written by the server: left_id, right_id, both sides' features
└── judgements.parquet    # written by the user code: left_id, right_id, verdict, score, detail
```

The server writes the inputs, starts the process with `SPOOL_TASK_DIR` set, waits, and reads the output. The exit code is the status and standard error is the log. IDs are strings in these files. Rust starts a command and never knows which language it is.

The exchange was tested with DuckDB: the server process wrote the candidates file, a second process read it and wrote judgements, and the server read them back and found the row that named an unknown observation. The second process could not open the database file while the server held it.

A small helper per language hides the files:

```python
# entities/company/strategies/splink.py
from spool import strategy

@strategy
def judge(candidates):      # Arrow table: left_id, right_id, left_*, right_*
    ...                     # any Python: Splink, a classifier, an LLM call
    return judgements       # left_id, right_id, verdict, optional score and detail
```

Four rules keep this contained:

1. **Workers stay inside the service.** They are a bounded pool of child processes, each with a timeout. Spool provisions no compute, holds no cloud credentials and starts no containers.
2. **Tasks produce evidence only.** A task returns judgements, and later observations or feature columns. It never returns identity changes. The server alone reconciles and commits, in one transaction, so user code that crashes or hangs can fail a run and cannot corrupt an identity.
3. **Only code built into the image runs.** The API takes data and the name of a strategy. It never takes code, so a caller cannot make the server run something new.
4. **No workflow engine.** The pipeline is short and fixed: ingest, features, candidates, judgements, reconcile, commit. Each stage's output is stored per run, so after a restart a run resumes from its last finished stage. `spool.run` is the task record.

There are four places an extension plugs in:

1. **Entity types.** Files in the project directory.
2. **Observations.** Clients post them through the API. An LLM extraction pipeline that runs in the caller is an upstream producer; the server does not know it exists. From Milestone 5, an extractor can also be user code in the project.
3. **Candidates.** The SQL files under `rules/block/`, or pairs a user-code strategy computes itself, such as nearest neighbours from an embedding index.
4. **Judgements.** SQL rules, a user-code strategy, or a client that pulls a run's candidates and submits judgements through the API. The last is how a review interface or an external system takes part.

Reconciliation policy is configuration in the first versions. Someone who needs custom logic can author a changeset directly and it will be validated like any other.

Consequences to accept:

- An image with user code carries `uv` or `bun` and the code's dependencies. A project of data files stays one binary.
- User code is the operator's own code. It runs with the service's permissions, and Spool does not sandbox it.
- Files cost a write and a read per task. That suits batches and is wrong for per-record calls, which stay with SQL rules.
- A strategy that does its own blocking, as Splink does, needs feature rows and not candidate pairs. Whether the server writes those per task is settled in Milestone 4.
- Only the server opens the store. A studio or any other tool goes through the API.

**Decide now:** child processes and no embedded runtime, Parquet files as the task protocol, tasks that produce evidence only, no code accepted through the API, workers inside the one service. **Defer:** separate worker containers that pull tasks over the same protocol, a streaming protocol, per-task resource limits beyond a timeout, policy plug-ins.

## 7. JSON, Arrow, Parquet or DuckDB relations

| Use | Form | Reason |
| --- | --- | --- |
| Changesets, drafts, review items, reports, run status, single lookups | JSON objects over the API | Small, structured, and they need schemas and good error messages |
| Ingesting many observations, pulling candidates, submitting judgements through the API | Arrow IPC streams | Columnar and streamable |
| The server to user code and back | Parquet files in a task directory | Any language reads them, and a failed task leaves its inputs behind to inspect |
| Blocking, comparison, aggregation, metrics | DuckDB SQL inside the engine | DuckDB already does it; string similarity functions are built in |
| The project's definition | Files in the project directory, compiled to a manifest | Reviewable in git, validated at build |
| Datasets at rest, benchmark fixtures, exports | Parquet | Portable, and `COPY ... TO` works in the stock library |

The rule of thumb for client authors: accept plain objects for convenience up to some thousands of rows, convert them once, and document the Arrow path for anything larger. Simple examples never mention Arrow or DuckDB.

Testing confirmed the internal half of this: an Arrow batch appended into a table, a blocking join with `jaro_winkler_similarity` read back as Arrow, and an in-memory Arrow batch joined against a stored table without first loading it into a table. That last one is how submitted judgements get validated against known observations.

Ad hoc SQL through the API is deferred. `Engine.query` is read-only, but a read-only DuckDB connection can still read files on the server's disk through functions such as `read_csv` (tested). An endpoint that takes SQL from callers needs DuckDB's external-access controls first, and those have not been tested alongside the Parquet task files.

**Decide now:** relations never cross the API. **Defer:** binary UUID columns in Arrow, an ad hoc SQL endpoint.

## 8. Integrating existing algorithms without leaking their assumptions

Every external algorithm sits behind an adapter that ends at the judgements relation. Three things are stopped at that boundary:

| Leak | Example | Stopped by |
| --- | --- | --- |
| The tool's entity IDs | Splink `cluster_id`, Zingg `Z_CLUSTER`, dedupe cluster numbers | Adapters return pairs only; identity comes from the reconciler |
| The tool's score scale | Match weight, probability, similarity | The adapter applies the tool's own threshold and reports a verdict; the raw score goes in `score` and `detail` |
| Transitivity | Connected components inside the tool | The adapter never calls the tool's clustering step, or expands its clusters back into pairs |

How specific tools would attach:

- **Splink.** A Python strategy file reads features from Parquet, calls `predict` (or `predict_between` for new-versus-existing, which is marked experimental), and maps match weights to verdicts with the per-comparison levels in `detail`. Its model file is an opaque artefact in the project. It is the best candidate for the second strategy in Milestone 4 because it exercises the user-code path and gives a credible baseline.
- **LLM matchers.** Verdict plus rationale, with `unsure` used freely. Selection-style prompts become one `match` and several `no_match` judgements.
- **dedupe.** Use its pair scores; ignore its clustering.
- **Zingg.** AGPL and Spark, so out of process only: read a Parquet export, write pairs. Low priority.

Training data, model state and thresholds belong to the strategy. The framework records the configuration hash in `run` so any judgement can be traced to the exact model that made it.

The built-in strategy for Milestone 1 is deliberately plain, and it knows nothing about any entity type. It is a rule runner. Its rules are SQL predicates over a left row `l` and a right row `r` of the type's feature table, one predicate per file:

- `rules/block/` predicates generate the candidate pairs.
- A candidate for which any `rules/no_match/` predicate holds is judged `no_match`.
- Otherwise, a candidate for which any `rules/match/` predicate holds is judged `match`.

Other candidates get no judgement. The names of the predicates that held go in `detail`, and a predicate's name is its file name. For the example company declaration in section 1, a rule set could be:

```text
entities/company/rules/
├── block/
│   ├── same_domain.sql                # l.domain = r.domain
│   ├── same_registry_id.sql           # l.registry_id = r.registry_id
│   └── name_prefix.sql                # l.name_norm[:4] = r.name_norm[:4]
├── no_match/
│   └── different_registry_id.sql      # l.registry_id <> r.registry_id
└── match/
    ├── same_domain.sql                # l.domain = r.domain
    └── similar_name_same_country.sql  # jaro_winkler_similarity(l.name_norm, r.name_norm) >= 0.92 AND l.country = r.country
```

A person type has its own columns and its own rules, and runs through the same code. Running SQL predicates keeps to Spool's non-goal of writing custom Rust implementations of algorithms DuckDB already handles efficiently. A native Fellegi-Sunter model is a later choice, not a prerequisite.

**Decide now:** adapters end at judgements; the built-in strategy reads its rules from the project and contains none. **Defer:** which adapters to ship, and whether Spool ever implements a probabilistic model natively.

## 9. Benchmarking resolution quality from the beginning

The benchmark runs the ordinary path (ingest, match, reconcile, commit) and scores the committed state against a truth relation `(observation_id, true_entity)`. Nothing is special-cased, so a Rust, Python or TypeScript strategy is scored identically. `spool bench` uses the engine in-process, with no server, and starts user-code strategies the same way the server does.

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

Datasets for day one: Splink's `fake_1000` as a smoke test, `historical_50k` and FEBRL as mid-sized checks, and a small hand-built company fixture committed to the repository that contains the hard cases of company resolution (aliases, legal versus trading names, domain changes, subsidiaries, records with few attributes), split into two batches. Each fixture is a project directory with its own entity type and rules. The three public datasets are person records and the fixture is company records, so the benchmark runs two unrelated entity types through one engine from the start. The public datasets are downloaded and checksummed at benchmark time, because their redistribution terms have not been confirmed.

Results are written as JSON. CI compares them with a committed baseline and fails on a regression, with false merges and ID stability treated as the strictest gates.

For a real company benchmark, the [GLEIF register](landscape-survey.md#5-benchmark-datasets-and-evaluation-practice) is the best public source the survey found: CC0, LEI as ground truth, real former and trading names, parent links for hard negatives, and 6,521 records flagged as duplicates of another LEI. Building a scenario generator on it is Milestone 3 work.

Metrics are implemented in Spool. The existing `er-evaluation` package is AGPL.

**Decide now:** truth relation, scenario format, false merges and ID stability as first-class metrics, baseline comparison in CI. **Defer:** the GLEIF scenario generator, sampling-based estimation for unlabeled data, extraction and end-to-end suites, cost and latency reporting.

## 10. The smallest useful end-to-end implementation

Milestone 1 is a Rust library, a test suite, and a thin command that runs one scenario. It has no server.

```text
spool/
├── Cargo.toml                 # workspace; edition 2024; rust-version 1.89
├── crates/
│   ├── spool-core/            # IDs, Op, Changeset, validate, reconcile, in-memory state
│   └── spool/                 # Engine, DuckDB store, project discovery, ingest, rule runner, metrics; `spool` binary
└── benchmarks/
    ├── datasets.toml          # name, url, sha256, licence
    └── identity/companies-small/   # a project directory, two CSV batches, truth, scenario file
```

Milestone 2 adds `crates/spool-cli`, which takes over the binary and holds the HTTP server and the `init`, `dev`, `build`, `apply` and `serve` commands, so the library carries no HTTP stack. It also adds `packages/client`, the TypeScript client, `schema/`, the JSON Schema its types are generated from, and the `Dockerfile` of the base image. Its exit criteria are listed after Milestone 1's. `spool-duckdb` is split out when a second store exists, and `spool-resolution` when a second native strategy exists.

What it does:

1. Open a database file and create the schema with a format version.
2. Read the fixture's project directory and apply it, which generates the feature table.
3. Ingest batch one from CSV: content-hashed observations and their feature rows.
4. Run the built-in SQL strategy with the fixture's rules to produce candidates and judgements.
5. Reconcile into a draft changeset, validate it, commit it, and print the identities.
6. Ingest batch two, which contains exact re-sends, new records for known companies, and new companies. Repeat steps 4 and 5.
7. Report pair precision and recall, B-cubed, false merges and ID stability against the fixture's truth.

Exit criteria, each a test:

- The `fake_1000` person data runs through the same binary as the company fixture, with only the project directory differing.
- Re-sent rows create no observations and no operations.
- Every identity ID from batch one still exists after batch two, with the same members plus any additions.
- A subsidiary with a different registry identifier is not attached to its parent.
- Two records that a rule says are different are not attached to the same identity, even when each of them matches it.
- An observation matching two identities is left unassigned with a review item.
- A review item is still in the store after the store is reopened, and its observation is examined again by the next run.
- A resolve that is cut short at any step is completed by the next one.
- Rebuilding the projection from the log reproduces the state exactly.
- The in-memory state and the DuckDB store pass the same conformance suite.
- Two runs with the same seed produce the same changeset content hashes.
- A changeset with a stale precondition is rejected and names the conflicting operation.

Milestone 2's exit criteria, each a test in `crates/spool-cli/tests` unless it says otherwise:

- With the same seed, the company fixture produces the same changeset content hashes through the library, through the server with Arrow bodies and with JSON bodies, and through the TypeScript client (`packages/client/test`).
- Every call under `/v1` is refused without the bearer token, and `serve` does not start without one.
- `serve` sets up an empty store, starts again on a store its manifest was applied to, and refuses a store that was applied with another manifest until `spool apply` has run.
- A call that outlasts its wait answers `202` with the run, the run is read until it ends, and reads are answered while it is in progress.
- A run leaves its draft for review when asked. The draft is listed, read, approved or rejected, and approving one that went stale names the operations at fault.
- A draft that is rejected, replaced or never decided does not strand its observations: the next run examines them again.
- A caller reads a run's candidates and submits judgements, as Arrow and as JSON, and reconciles the run.
- Identities, members, history, observations and typed review items are read through the API.
- `init` scaffolds a project that `info` lists and `build` compiles; `build` writes the Dockerfile once.
- `apply` rebuilds the feature table of a changed declaration and changes no identity; `dev` does the same as files are saved, and keeps the last version that compiled.
- A run cut short by a killed server is recorded as failed when the store is next opened.
- The binary starts with no library path set.
- The committed JSON Schema is the one the server speaks, and the client's generated types are those of the schema (`packages/client`).

In scope for the types but not implemented: `Merge`, `Constrain`, `Unconstrain`, `Reinstate`. Out of scope after Milestone 2: user code, superseding observations, probabilistic or LLM matching, extraction, relationships, canonical attribute views, the studio.

Proposed roadmap:

1. **Engine.** The Rust library and command described above: a project directory with SQL features and rules, two batches, the changeset log, the benchmark.
2. **Server and client.** `spool serve` and the API, runs as tasks, `init`, `dev`, `build` and `apply`, the generated Dockerfile, the TypeScript client.
3. **Identity lifecycle.** Merge, split, constraints, undo and superseding observations, plus the identity benchmark.
4. **User code.** The child-process protocol, Python first and then TypeScript, with Splink as the second strategy.
5. **Extraction.** Observations from unstructured documents.
6. **Developer tooling.** Inspection and a studio, both through the API.

Milestones 2 and 3 do not depend on each other. Sandbox synchronisation, initial Milestone 6, is not in this roadmap.

## Decisions at a glance

| Decide now | Defer |
| --- | --- |
| Evidence versus identity-state split; `spool-core` without DuckDB or Arrow | A Postgres store; any abstraction over evidence storage |
| One server that owns the store; thin clients; one tenant, one writer; no native bindings | Richer authentication; a Python client; a published base image |
| A project directory with path-derived names, compiled to a manifest; state outside the image | Mountable packages of entity types; Markdown instruction files |
| Entity types declared by the project; feature tables generated from a query; rules as files; no built-in types | Feature columns from user code; client types generated from a manifest |
| User code as child processes over Parquet files; evidence only; workers inside the service | Separate worker containers; a streaming protocol; per-task resource limits |
| Judgements and draft changesets in place of proposals | Entity-centric judgements; calibrated probabilities |
| Review items kept in the store and examined again by later runs | Closing a review item without giving its observations an identity |
| Minted UUIDv7 identity IDs, permanent redirects, content-derived observation IDs | Public ID formatting; binary IDs in Arrow |
| Changeset log as truth, projection in the same transaction, per-operation preconditions | Compaction, hash chaining |
| Operation set: create, assign, merge, constrain, unconstrain, reinstate | Implementing all but create and assign until Milestone 3 |
| Default policy never merges or splits existing identities automatically | Policy plug-ins; automatic merges above a threshold |
| JSON and Arrow as the only wire forms; the run ID as the task handle; drafts stored on the server | Streaming run progress; an ad hoc SQL endpoint |
| Scenario-based benchmark with identity metrics and a CI baseline | GLEIF scenario generator; extraction and end-to-end suites |
| Two library crates and one for the command and server; injected ID generator and clock; format version in the file; the prebuilt DuckDB library found by run path | Further crate splits; a statically linked DuckDB; indexes and keys |
