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
8. **User code runs as child processes, never inside the engine.** Python and TypeScript files in the project run through `uv` and `bun`, and exchange Parquet files with the server. They produce evidence only. They run inside the one service, one at a time, each with a timeout.
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
│       ├── strategies/           # user code: splink.py, llm.ts, and a directory per strategy for its files (section 6)
│       ├── extract.md            # what an extractor looks for when it looks for this type (section 6)
│       └── review.md             # guidance for whoever reviews this type's drafts (later)
├── extractors/                   # user code that reads documents: names.py, llm.ts, and a directory per extractor for its files (section 6)
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
| `spool info` | Lists the discovered entity types, rules, strategies and extractors, with diagnostics. |
| `spool build` | Validates the project and compiles it into a manifest. Every SQL file is prepared against DuckDB, so a mistake fails the build, not a run. Writes a `Dockerfile` if the project has none. With `--install` it also starts each user-code strategy and extractor once with nothing to do, which fetches what it depends on. |
| `spool apply` | Applies a manifest to a store: declares entity types and rebuilds the feature tables whose declaration changed. |
| `spool serve` | Runs the server from a manifest and a store location. An empty store is set up from the manifest. |
| `spool bench` | Runs benchmark scenarios in-process (section 9). |
| `spool schema` | Prints the JSON Schema of the API's values (section 5). |

Three rules keep deployment simple:

1. **The directory is build-time input.** The engine reads only the manifest, and the strategy and extractor files the manifest names. The manifest has a content hash that is recorded with every run, so any judgement can be traced to the project version that produced it. It records the hash of each strategy and extractor file, and the engine runs a file only while it still has that hash.
2. **A project of data files needs only the `spool` binary.** Discovery and the build are in Rust, so no Node or Python is involved. Only a project with user code adds `uv` or `bun` to its image.
3. **State is never in the image.** The image holds the binary, the manifest, and any user code with its dependencies. The store is a file on a volume, and its location is configuration.

The generated Dockerfile starts from the Spool base image, copies the project, compiles the manifest, declares a volume for the store at `/data` and runs `spool serve` as a user that is not root. For a project of data files the image holds the binary and the manifest. For a project with user code it also holds the project's files, `uv` or `bun` (whichever the project's strategies need, copied from their published images at a fixed version), and what the strategies depend on, which `spool build --install` fetches while the image is built. `uv` is then told to stay offline, so a run does not wait for the network. This was tested with images built on a local machine: a container with no network ran Splink for the person project, and the Python and TypeScript strategies of the company fixture, as the user `spool`, and committed the changesets the same scenarios commit outside a container. The Dockerfile belongs to the project and can be edited: `spool build` writes it once and never again. The result is one container and one volume, run wherever containers run.

The base image is built from the `Dockerfile` at the root of this repository. It holds the `spool` binary in `/usr/local/bin` and the DuckDB shared library in `/usr/local/lib`, where the binary looks for it. The generated Dockerfile names `ghcr.io/alexpatow/spool:<version>`, and `spool build --base-image` names another. No image is published yet, so for now the base image is built locally under that name.

Applying a changed definition to an existing store is an explicit step, never a side effect of a container starting. It rebuilds feature tables, which can take time. It cannot change an identity (section 1). `spool serve` refuses to start on a store whose applied manifest differs from its own, and says which `spool apply` to run. A store that has never had a manifest applied is the one exception: there is nothing to rebuild and no identity to protect, so `serve` applies its manifest and starts. A first deploy is then one container start, with no separate step against an empty volume.

Tradeoff: the file layout becomes an interface that has to be documented and kept stable, and the model cannot be changed through the API. Changing an entity type means a build and a deploy. `spool dev` covers fast iteration, and `spool info` shows what the conventions resolved to.

**Decide now:** a project directory with path-derived names, a manifest as the engine's only input, discovery and build in the Rust binary, state outside the image, an explicit apply step. **Defer:** packages of entity types that a project can mount, `review.md`, settings in `spool.toml` beyond the minimum.

## 1. DuckDB representation of observations, candidates and identity state

All tables live in a `spool` schema so a user's own tables can share the file. The column types used below or available to feature columns (`UUID`, `JSON`, `STRUCT`, `LIST`, `MAP`) and the `uuidv7()` function are all in the stock DuckDB library (tested).

Evidence:

```sql
CREATE TABLE spool.observation (
  observation_id UUID        NOT NULL, -- first 128 bits of sha256(source_id, source_key, canonical payload)
  entity_type    VARCHAR     NOT NULL, -- a declared type, for example 'company'
  source_id      VARCHAR     NOT NULL, -- 'crm', 'registry', 'report:acme-annual-2025.pdf'
  source_key     VARCHAR,              -- key within the source: a row key, or an extracted mention's locator
  supersedes     UUID,                 -- previous version of the same (entity_type, source_id, source_key)
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
  status          VARCHAR NOT NULL,       -- 'open' | 'resolved' | 'superseded' | 'dismissed'
  closed_seq      BIGINT,                 -- resolved: the changeset after which every observation had an identity
  closed_run      BIGINT,                 -- superseded: the reconcile run that raised it again in another form
  closed_by VARCHAR, closed_at TIMESTAMPTZ, closed_reason VARCHAR   -- dismissed: who closed it, when, and why if they said
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
  merged_into UUID, merged_seq BIGINT,    -- always points at an active identity (chains are flattened)
  merge_target UUID                       -- the identity the merge named, which may itself have been merged since
);

CREATE TABLE spool.membership (           -- projection with history; current rows have to_seq IS NULL
  observation_id UUID NOT NULL, identity_id UUID NOT NULL,
  from_seq BIGINT NOT NULL, to_seq BIGINT
);

CREATE TABLE spool.constraint_link (      -- projection with history
  observation_a UUID NOT NULL, observation_b UUID NOT NULL, kind VARCHAR NOT NULL,
  from_seq BIGINT NOT NULL, to_seq BIGINT
);

CREATE TABLE spool.document (
  document_id    UUID        NOT NULL, -- first 128 bits of a sha256 over (source_id, source_key, text)
  source_id      VARCHAR     NOT NULL,
  source_key     VARCHAR     NOT NULL, -- the document's key within its source: a path, a URL, a message ID
  supersedes     UUID,                 -- the version of the same (source_id, source_key) before this one
  uri            VARCHAR,              -- where the source keeps it, if it said
  text           VARCHAR     NOT NULL,
  ingested_at    TIMESTAMPTZ NOT NULL,
  ingest_run     BIGINT      NOT NULL,
);

CREATE TABLE spool.extraction (     -- one row each time an extractor read a document
  document_id       UUID    NOT NULL,
  extractor         VARCHAR NOT NULL,
  extractor_version VARCHAR NOT NULL, -- the hash of the extractor's files
  run_id            BIGINT  NOT NULL, -- the extract run
  mentions          BIGINT  NOT NULL, -- how many mentions it found there
);

CREATE TABLE spool.meta (key VARCHAR NOT NULL, value VARCHAR NOT NULL);  -- format_version
```

Design points:

- **Raw payload and typed features are separate.** The payload is never rewritten, so a better normaliser can be rerun over history. Matchers read only the feature table.
- **An extracted mention's key is its locator.** `source_key` holds the mention's place in its document: `<document_id>#<entity_type>:<start>-<end>`, where `start` and `end` count Unicode code points and `end` is the position after the last one. Two identical mentions at different places are then two observations, and each keeps its own `evidence`. Repeating the same extraction produces the same IDs and is a no-op. The cost is that a document which names one entity many times yields that many observations, which all match each other.
- **The locator names a version of the document, not its key.** A document is text under a source and a key, and its ID is derived from all three, so a changed text is another document that names the one it supersedes. Had the locator named the key, a mention at characters 10 to 20 of the new text would supersede whatever was at 10 to 20 of the old one, and a run treats a version and the one before it as a match (below). With the document's ID in it, a locator never spans two versions. What follows is a limit: the mentions found in an earlier version stay as observations, and members of their identities, after the document has changed.
- **Documents are text.** Turning a PDF or a web page into text is the caller's work, and a place in a document is a place in that text. Pages and bounding boxes are not recorded.
- **Feature tables are generated from a declaration, never hand-written.** A declaration is one SQL query over the type's observations. The engine creates `spool.features_<entity_type>` from its result and fills it with one `INSERT ... SELECT` per ingest. No engine code names a feature column, and `ingest` rejects a type that has not been declared.
- **Changing a declaration changes no identity.** Applying a changed query for an existing type increases its version and rebuilds the feature table from the stored payloads. Identity state refers to observations and to the type's name, never to feature columns, so every identity ID stays in place. A match run records the declaration version it read, so its judgements stay traceable.
- **Typed columns, not one generic table.** A single features table holding JSON for every type would need no generation. It would also move normalisation into every matching query and give up the typed columns that blocking joins and similarity functions work on.
- **Operations are rows, not a JSON blob.** A first load is one changeset with as many `assign` rows as observations, applied with `INSERT ... SELECT`. When measured, a one-million-operation changeset wrote its log rows and its projection rows in one transaction in 0.4 s.
- **Membership keeps intervals.** "What was this identity at log position N" is a filter, and undoing a merge can read which observations it moved.
- **A merged identity keeps both pointers.** `merged_into` is the active identity that stands for it now, so a lookup follows any chain of merges in one step. `merge_target` is the identity its own merge named. Undoing a merge in the middle of a chain needs the second: the identities that were merged into the reinstated one point at it again, and no further.
- **A changed row supersedes the one before.** At ingest, a keyed row whose content is new names the latest version of the same entity type, source and key in `supersedes`. Versions sent in one call follow each other in row order. The earlier version is kept, stays a member of its identity and can still be matched. A run treats a version and the one it supersedes as a match whatever the rules say, so a row that changed joins the identity of its earlier version, or is left for review if it now also matches another. One limit: a source that sends content it has sent before under the same key sends a duplicate, which is ignored, so a row that changes from A to B and back to A ends at B.
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

**Decide now:** the evidence versus identity-state split, content-derived observation IDs, extracted mentions keyed by their locator, entity types declared by the project with versioned declarations and generated feature tables, operation rows, interval membership, review items kept in the store, a format version from the first commit. **Defer:** indexes and keys, feature columns computed by user code, rebuilding a feature table after its declaration changes (Milestone 2), retention of old candidates and judgements, how a retracted or deleted source record is represented, a separate table of locators so that repeated mentions share one observation, retiring the mentions of a document version that was superseded, documents that are not text.

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
    fn constraints_of_members(&self, ids: &[IdentityId]) -> Result<Vec<Constraint>>;
    /// The log positions at which each identity's membership last changed: what a merge is checked against.
    fn membership_changed(&self, ids: &[IdentityId]) -> Result<Vec<Seq>>;
    /// What undo reads: the observations that left an identity at a log position,
    /// and the identity a reinstated identity had been merged into.
    fn left_at(&self, identity: IdentityId, seq: Seq) -> Result<Vec<ObservationId>>;
    fn merged_before(&self, identity: IdentityId, seq: Seq) -> Result<Option<IdentityId>>;
}

/// The only way to obtain a `Validated`, and a store accepts nothing else.
pub fn validate(state: &impl IdentityState, cs: Changeset) -> Result<Validated, Vec<Rejection>>;

pub fn reconcile(evidence: &EvidenceSummary, state: &impl IdentityState, policy: &Policy,
                 ids: &mut impl IdGenerator) -> Draft;   // Draft = Changeset + review items
```

The seam between the two halves is **SQL prepares, Rust decides**. DuckDB does the heavy join of judgements against current membership and returns a compact summary per new observation (which identities it matched, how many match and no-match judgements, the best score). The reconciler is a pure function over that summary. Core never sees SQL, and DuckDB never decides what a merge means.

The DuckDB store in the `spool` crate implements `IdentityState` and one write: apply a `Validated` changeset by inserting log rows and updating projections in a single transaction. One conformance suite runs against both the in-memory state and the DuckDB store, and property tests compare random operation sequences between the two. A second persistent store for identity state, such as Postgres, would be a third implementation that must pass the same suite.

Split is deliberately not an operation. It is `CreateIdentity` plus `Assign` moves, usually with a `cannot_link` constraint so the matcher does not undo it, in one changeset. The original ID stays with whichever side the author leaves in place; the default policy keeps it with the larger side, as OpenSanctions does. The engine writes that changeset for a caller who names one side: the larger side keeps the ID, the side that was not named on a tie, and one `cannot_link` between the smallest observation of each side is recorded unless the caller declines it.

What the validator holds a changeset to, beyond each operation's precondition:

- **A merge** needs two different active identities of one entity type whose membership has not changed since the changeset's `base`.
- **An observation moves once per changeset**, whether an `Assign` or a `Merge` moves it, and an identity is merged or reinstated once. Operations are checked in the order written, so an `Assign` out of an identity comes before the `Merge` of that identity. The rule keeps a changeset's meaning independent of how a store applies it, which is set-based.
- **A constraint** names two different observations of one type. A pair has at most one constraint in force, of either kind, and a constraint is recorded or removed once per changeset.
- **The state a changeset leaves breaks no constraint in force.** The two observations of a `must_link` share an identity, and those of a `cannot_link` do not. An observation without an identity breaks nothing. The rejection names the constraint, the two identities, and the last operation that had a part in it. A changeset can therefore move one observation of a `must_link` only by moving the other with it or by removing the constraint first.

Tradeoff: validating in Rust means a changeset must fit in memory, which caps one changeset at roughly millions of operations. Larger loads are chunked into several changesets. That is preferable to moving validation into SQL, which would put identity rules in the database.

**Decide now:** the dependency-free core crate, validated-changeset type, injected ID generator and clock (required for reproducible tests), operation set. **Defer:** a Postgres store, any trait over evidence storage.

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

The inverse reverses the operations last first. An `Assign` is reversed. `Constrain` and `Unconstrain` undo each other. A `Merge` becomes a `Reinstate` and the `Assign`s that move back the observations the merge moved, leaving out any that are no longer in the surviving identity. A `Reinstate` becomes the `Merge` it had undone, so an undo can itself be undone. `CreateIdentity` is not undone: an identity ID is never taken back, and an identity left without members stays. The undo is validated like any other changeset, against current state. If state has moved on so that an operation no longer holds, it is rejected and names that operation, and nothing is undone by halves.

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

Rerunning a matcher over old data is a **rematch**: a run whose scope is every observation of the type, not only the new ones. It is the one kind of run that may judge two observations that both have an identity. The policy is the same, so those judgements move nothing. The run reports them as suggestions: each pair of identities with a `match` between their members is a suggested merge, and each identity with a `no_match` between two of its members is a suggested split, both with the counts of pairs for and against. A reviewer who agrees commits a merge or a split, which is a changeset like any other. Observations that have no identity are placed by a rematch as by any run.

Constraints and superseding versions enter here as evidence that outranks the strategy's. Before the summary is built, the judgement on a pair is replaced, or added where the strategy gave none: a `must_link` in force reads as `match`, a `cannot_link` as `no_match`, and a version and the one it supersedes read as `match`, with a constraint ahead of supersession. The policy is unchanged and needs no rule about constraints. A `cannot_link` against the only identity an observation matches leaves it to form its own. A `cannot_link` against one member of an identity it otherwise matches is a conflict, and is left for review. A `must_link` to a member of an identity places the observation there when nothing else matches. What a run reports as the strategy's judgements are still the strategy's.

Whenever the policy leaves observations unassigned, the reconcile run records a review item in `spool.review_item` (section 1). Its kind is `possible_merge` when the observations match members of two or more identities, and `conflict` when a `no_match` contradicts their matches. The item names the observations, the identities they matched and the run that raised it. It is evidence, like a judgement: it changes no identity, and nothing is committed on its account.

A review item is open while any of its observations has no identity. It closes in one of three ways:

- **Resolved.** A changeset gives the last of its observations an identity, and the item records that log position. Any changeset does this, whether a reviewer wrote it or a later run drafted it.
- **Superseded.** A later run examines the observations and raises an item that differs, for example because they now match a third identity. The old item records that run.
- **Dismissed.** A reviewer closes the item without placing its observations, and the item records who, when and why. The observations stay without an identity and leave the scope of later runs, so the question is not raised again. A changeset can still place them.

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
    pub fn add_documents(&self, source: &str, documents: &[NewDocument]) -> Result<IngestReport>;
    pub fn extract(&self, options: &ExtractOptions) -> Result<ExtractReport>; // runs an extractor of the project over unread documents
    pub fn begin_run(&self, strategy: StrategyRef, scope: Scope) -> Result<RunId>;
    pub fn candidates(&self, run: RunId) -> Result<impl RecordBatchReader>;   // pairs with both sides' features
    pub fn submit_judgements(&self, run: RunId, rows: impl RecordBatchReader) -> Result<u64>;
    pub fn reconcile_with(&self, run: RunId, policy: &Policy, then: Disposition) -> Result<TypeResolution>; // commit, or leave for review
    pub fn commit(&self, changeset: Changeset) -> Result<Committed, Rejected>;
    pub fn approve_draft(&self, draft: ChangesetId) -> Result<Committed, Rejected>;
    pub fn reject_draft(&self, draft: ChangesetId, reason: Option<&str>) -> Result<DraftSummary>;
    pub fn resolve(&self, options: ResolveOptions) -> Result<ResolveReport>; // the rules or a strategy of the project, default policy
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
| `POST /v1/documents` | Stores documents of one source as text, for an extractor to read. A changed text under a key is the document's next version | The ingest run |
| `GET /v1/documents`, `GET /v1/documents/{id}` | Lists documents in pages, without their text; reads one version with its text and each time an extractor read it | JSON |
| `POST /v1/extractions` | Runs one of the project's extractors, which `GET /v1/info` lists, over the latest version of each document it has not read, or with `again` over those it has. What it finds is stored as observations | The extract run |
| `POST /v1/runs` | Matches the new observations of one entity type, or with `rematch` all of them. With no strategy, the server runs the rules and reconciles, and `commit` says whether the draft is committed (the default) or left for review. The name of one of the project's user-code strategies, which `GET /v1/info` lists, runs that instead. With any other name the run opens and waits for the caller's judgements | The match run, or `null` when nothing was new. A rematch lists its suggestions |
| `GET /v1/runs`, `GET /v1/runs/{run}` | Lists runs; reads one, optionally waiting for it to end | Runs |
| `GET /v1/runs/{run}/candidates` | The candidate pairs of an open run with both sides' features | Arrow, or JSON when the `Accept` header asks for it |
| `POST /v1/runs/{run}/judgements` | Stores judgements, as Arrow or JSON | A count |
| `POST /v1/runs/{run}/reconcile` | Turns the run's judgements into a changeset and commits it, or leaves it as a draft with `commit: false` | The resolution: counts, review items, and the commit or the draft's ID |
| `POST /v1/runs/{run}/cancel` | Gives up an open run | The run |
| `GET /v1/drafts`, `GET /v1/drafts/{draft}` | Lists drafts; reads one with its changeset | JSON |
| `POST /v1/drafts/{draft}/approve`, `.../reject` | Commits a draft, or closes it | The commit, or the operations that were rejected; the draft |
| `GET /v1/changesets`, `POST /v1/changesets` | Reads the log in pages; commits a changeset the caller wrote. A merge, a move and a constraint are each a changeset of one operation. The server mints the changeset's ID if the caller sends none, and takes the head of the log as its base | The log; the commit, or the operations that were rejected |
| `POST /v1/changesets/{seq}/undo` | Takes back the changeset at a log position with a new one, or with `commit: false` only returns it | The undo and its commit, or the operations that were rejected |
| `POST /v1/identities/{id}/split` | Divides an identity in two, given the observations of one side | The two identities, what moved, and the commit |
| `GET /v1/constraints` | The constraints in force, or with `removed=true` their history, for one observation or one identity's members | JSON |
| `POST /v1/ids` | Mints identity and changeset IDs for a caller that writes its own changeset | JSON |
| `GET /v1/identities`, `GET /v1/identities/{id}`, `.../history` | Lists identities in pages; reads one with its members, following redirects; the changesets that touched it | JSON |
| `GET /v1/observations`, `GET /v1/observations/{id}` | Finds observations by source and key, those found in one document, or those with no identity; reads one | JSON |
| `POST /v1/observations/identities` | The identities of many observations in one call | JSON |
| `GET /v1/review`, `GET /v1/review/{id}` | Review items, typed; one of them | JSON |
| `POST /v1/review/{id}/dismiss` | Closes a review item without placing its observations | The item |

- **A call that starts work says how long it will wait.** `POST /v1/ingest`, `POST /v1/documents`, `POST /v1/extractions` and `POST /v1/runs` take `wait_ms`, 30 seconds if left out. If the run finishes within that time, the response is `200` with the finished run. Otherwise it is `202` with the run as far as it has got, and the client reads `GET /v1/runs/{run}?wait_ms=...` until its status is no longer `running`. The work goes on whether or not anyone waits. The run ID is `spool.run.run_id`, so tasks add no new concept. One limit: a call that finds the writer busy has no run to report until its own work starts, so it can wait longer than it asked to.
- **A run has one of five statuses.** `running` while the engine works on it, `open` while it waits for a caller's judgements, `drafted` while its draft waits for review, and then `succeeded` or `failed`.
- **A run either commits its draft or leaves it for review**, as the caller asks. A draft left for review is how an agent experiments without changing shared identities. The draft is stored on the server and named by its changeset ID, so whoever reviews it need not be whoever started the run. Approving it validates it like any changeset: if state moved under it, the response names the operations that no longer hold and the draft stays open. Section 1 says why a draft nobody commits does not strand its observations.
- **Small values are JSON**, defined once with `serde` and described by one versioned JSON Schema. Client types are generated from it. The same serialised form is the log's exchange format. A request with a field the server does not know is refused, so a misspelt `commit` cannot commit by mistake.
- **Bulk data is Arrow**, as IPC streams in request and response bodies. Clients also accept plain objects for convenience, up to some thousands of rows. Rows sent as JSON become the same observations as the same rows sent as Arrow: a number with no fractional part is stored as an integer whichever way it arrived, so an observation's ID does not depend on the encoding or on the other rows in its batch. A JSON integer that its column cannot hold exactly is refused, not rounded: one of 2^53 or more beside numbers with a fraction, or one past the signed 64-bit range beside a negative one. A JSON row with no values is still a row.
- **IDs cross as strings** in the first version. DuckDB exports `UUID` columns to Arrow as strings by default (tested), which is also what JavaScript and pandas users expect.
- **Errors have one shape.** Every response that is not a success is `{"error": {"code", "message"}}`, with `rejections` when a changeset broke a rule. The codes are `unauthorized`, `invalid`, `not_found`, `conflict`, `rejected`, `too_large`, `strategy_failed`, `extractor_failed` and `internal`.
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

A user-code strategy is a file under `strategies/`. `entities/company/strategies/splink.py` is the strategy `splink` for companies. A `.py` file runs with `uv run --script` and a `.ts` file with `bun run`. Each tool is a single binary that runs the file and resolves its dependencies: a Python file names its own in a comment at its top ([uv](https://docs.astral.sh/uv/guides/scripts/)), and `bun` fetches what a TypeScript file imports ([bun](https://bun.com/docs/runtime/typescript)). Files that belong to a strategy, such as a model, go in a directory beside it with the same name: `strategies/splink/model.json`. Anything else under `strategies/` is reported by `spool info` and not used.

The manifest records each strategy with its runtime, its path and a SHA-256 of the file and of the files in its directory. That hash is the strategy's version in `spool.run`. Before a run the engine checks that the file on disk still has it, and refuses to run one that changed: the project is built and applied again first. It checks every strategy before it starts the first, and each one again as it is started, since a run over several entity types can start its last strategy long after its first. `spool dev` does that as files are saved.

The protocol is a directory of files per task:

```text
$SPOOL_TASK_DIR/
├── task.json             # protocol version, run ID, entity type, strategy, whether it is a rematch, feature columns
├── candidates.parquet    # written by the engine: left_id, right_id, both sides' features as left_* and right_*
├── features.parquet      # written by the engine: observation_id, is_new, identity_id, the feature columns
├── judgements.parquet    # written by the user code: left_id, right_id, verdict, score, detail
├── stdout.log
└── stderr.log
```

The engine writes the inputs, starts the process in the project directory with `SPOOL_TASK_DIR` set, waits, and reads the output. The exit code is the status and standard error is the log. IDs are strings in these files. Rust starts a command and never knows which language it is. The judgements are stored under the checks a caller's judgements get through the API (section 4), so user code cannot name an unknown observation or, outside a rematch, judge two observations that both have an identity.

`candidates.parquet` holds the pairs the project's block rules found. `features.parquet` holds every observation of the type, which is what a strategy that finds its own pairs needs: Splink blocks for itself, and a nearest-neighbour search has no rules at all. `is_new` marks the observations the run is to place, and `identity_id` says where the others are. Both files are written for every task. A task that fails is kept, with its inputs and what the process printed, and the error names its directory. A task that succeeds is removed.

A small helper per language hides the files. The engine carries both and puts them where the process finds them, so a strategy imports its helper with nothing installed:

```python
# entities/company/strategies/in_python.py
from spool import strategy

@strategy
def judge(candidates):      # pyarrow table: left_id, right_id, left_*, right_*
    ...                     # any Python: Splink, a classifier, an LLM call
    return judgements       # left_id, right_id, verdict, optional score and detail
```

```ts
// entities/company/strategies/in_typescript.ts
import { strategy } from "spool-strategy";

await strategy(async (task) => {
  const pairs = await task.candidates();   // or task.features()
  return pairs.map((pair) => ({ left_id: pair.left_id, right_id: pair.right_id, verdict: "match" }));
});
```

The Python function is given what its parameters are named: `candidates`, `features` or `task`. It may return a list of dicts, a pyarrow table, or a pandas, polars or DuckDB result. The Python helper needs `pyarrow`, which the engine asks `uv` for. The TypeScript helper is built from `packages/strategy` into one file that includes a Parquet reader and writer ([hyparquet](https://github.com/hyparam/hyparquet) and hyparquet-writer, both MIT), and a test fails if the file the engine carries is not the one the package builds. Without `SPOOL_TASK_DIR` both helpers do nothing, so a strategy file can be imported in a test, and can be started ahead of time to fetch its dependencies.

Tested with `uv` 0.13.8 and `bun` 1.4.4: the company fixture's rules written as a Python strategy and as a TypeScript strategy each commit the changesets the SQL rules commit, hash for hash.

While the process runs the engine holds no lock on the store, so reads and other writes are served. One strategy or extractor runs at a time. A process has ten minutes by default (`--task-timeout`), after which it is stopped with everything it started, and its run fails. A run that fails for any reason changes nothing: the next run examines the same observations.

Four rules keep this contained:

1. **Workers stay inside the service.** They are child processes, one at a time, each with a timeout. Spool provisions no compute, holds no cloud credentials and starts no containers.
2. **Tasks produce evidence only.** A task returns judgements or mentions, and later feature columns. It never returns identity changes. The server alone reconciles and commits, in one transaction, so user code that crashes or hangs can fail a run and cannot corrupt an identity.
3. **Only code built into the image runs.** The API takes data and the name of a strategy or an extractor. It never takes code, so a caller cannot make the server run something new. A name the project does not have opens a run that waits for the caller's judgements, and the server runs nothing for it.
4. **No workflow engine.** The pipeline is short and fixed: ingest, features, candidates, judgements, reconcile, commit. Each stage's output is stored per run, so after a restart a run resumes from its last finished stage. `spool.run` is the task record.

### Extractors

An extractor is user code that reads documents and says where they mention entities. It is a file under `extractors/` at the top of the project: `extractors/names.py` is the extractor `names`. It belongs to the project and not to an entity type, because one reading of a document finds the companies and the people in it. It runs as a strategy runs: the same two runtimes, the same directory beside it for its files, the same hash in the manifest, checked before it starts. A name is one extractor, whichever language it is written in.

What an entity type has to say to an extractor is in `entities/<type>/extract.md`: prose that says what counts as a mention and which attributes to give. The engine does not read it. It hands the text to every extractor, and a language model's prompt is the obvious use. Editing it changes the manifest's hash and no feature table.

Documents reach the store through `POST /v1/documents`, and `POST /v1/extractions` names an extractor to run over them. The task is a directory, as for a strategy:

```text
$SPOOL_TASK_DIR/
├── task.json             # protocol version, kind "extract", run ID, extractor, and each entity type's name, instructions and feature columns
├── documents.parquet     # written by the engine: document_id, source, key, uri, text
├── mentions.parquet      # written by the user code: document_id, entity_type, start, end, attributes
├── stdout.log
└── stderr.log
```

```python
# extractors/names.py
import re
from spool import extractor

@extractor
def find(document):          # called once for each document; `documents` asks for all of them at once
    for found in re.finditer(r"[A-Z][a-z]+ (Inc|Ltd)", document.text):
        yield document.mention("company", found.start(), found.end(), name=found.group())
```

```ts
// extractors/names.ts
import { extractor } from "spool-strategy";

await extractor(async (task) =>
  (await task.documents()).flatMap((document) =>
    [...document.text.matchAll(/[A-Z][a-z]+ (Inc|Ltd)/g)].map((found) =>
      document.mention("company", found.index, found.index + found[0].length, { name: found[0] }),
    ),
  ),
);
```

A mention is a place in a document, an entity type and attributes. The engine makes each one an observation of that type: its source is the document's, its key is the locator of section 1, its payload is the attributes, and its evidence is the document's ID, key and URI, the place, the text there, and the extractor with its version. From there it is an observation like any other. The type's `features.sql` reads its payload, and the next match run places it.

- **Places count Unicode code points.** That is what Python's string indices are. JavaScript's count UTF-16 units, so the TypeScript helper's `document.mention` converts, and the same mention written in either language is the same observation, with the same ID. `document.find(quote)` gives the places of a quote, for an extractor that is told what a document mentions and not where.
- **The engine cuts the quoted text itself**, from the document and the place. An extractor cannot attribute to a document words that are not in it.
- **Attributes are flat**: text, numbers, and true or false. A number with no fractional part is stored as an integer, as for rows (section 5).
- **One bad mention refuses them all.** A mention of a type the project does not declare, a place outside the document, a document the task did not hand out, or the same place and type twice fails the run, and nothing is stored. So does a `start` or `end` column that does not hold whole numbers: a position is never rounded to a place the extractor did not name.
- **An extractor reads a document once.** A run reads the latest version of each document that this extractor has not read, whatever version the extractor had then. Reading again after the extractor changed is asked for with `again`: mentions it finds at the same place with the same attributes are duplicates, and with other attributes they supersede. A mention it no longer finds stays.
- **What an extractor found is stored under an ingest run of its own**, started when the mentions are stored, not under the extract run. A match run examines the observations ingested after the last run and before itself. An extractor can work for minutes, and a match run that began meanwhile would otherwise pass over what it found, for good.

The engine has no extractor built in, and calls no model. `benchmarks/extraction/company-news/extractors/llm.py` is an example of one that asks a language model, through the chat-completions API that many services accept and with nothing beyond Python's standard library. It asks for quotes and finds their places itself, and drops a quote the text does not have. It is tested against a stand-in for the API, and has not been measured against a model.

There are four places an extension plugs in:

1. **Entity types.** Files in the project directory.
2. **Observations.** Clients post them through the API. An extraction pipeline that runs in the caller is an upstream producer, and the server does not know it exists: it posts rows whose key is a locator of its own. Or the caller posts documents, and an extractor in the project reads them.
3. **Candidates.** The SQL files under `rules/block/`, or pairs a user-code strategy computes itself, such as nearest neighbours from an embedding index.
4. **Judgements.** SQL rules, a user-code strategy, or a client that pulls a run's candidates and submits judgements through the API. The last is how a review interface or an external system takes part.

Reconciliation policy is configuration in the first versions. Someone who needs custom logic can author a changeset directly and it will be validated like any other.

Consequences to accept:

- An image with user code carries `uv` or `bun` and the code's dependencies. A project of data files stays one binary.
- User code is the operator's own code. It runs with the service's permissions, and Spool does not sandbox it. The one thing withheld is the API's bearer token, which is taken out of the process's environment.
- Files cost a write and a read per task. That suits batches and is wrong for per-record calls, which stay with SQL rules.
- Every task gets both the candidate pairs and the feature rows, though most strategies read one of them. Writing only what a strategy asks for needs the strategy to say so before it runs, and is deferred.
- A Python file named after a library it imports would be found in place of the library, because Python looks first in the directory of the file it runs. The engine turns that off (`PYTHONSAFEPATH`), which needs Python 3.11 or later, so `splink.py` can `import splink`.
- Only the server opens the store. A studio or any other tool goes through the API.

**Decide now:** child processes and no embedded runtime, Parquet files as the task protocol, tasks that produce evidence only, no code accepted through the API, workers inside the one service. **Defer:** more than one user-code process at a time, extraction in batches of documents so that a long run stores what it has found so far, separate worker containers that pull tasks over the same protocol, a streaming protocol, per-task resource limits beyond a timeout, a published package of the TypeScript helper's types, policy plug-ins.

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

- **Splink.** Built in Milestone 4 as the second strategy, for the person project (`benchmarks/resolution/people/entities/person/strategies/splink.py`, with Splink 5.0). It reads the feature rows, scores the new records among themselves with `predict_within` and against the records that have an identity with `predict_between`, and never calls Splink's clustering. A match probability of 0.95 or more is a `match`, one from 0.5 up to that is an `unsure`, which is kept and changes nothing, and below 0.5 it says nothing: a low score is not evidence of a difference, and a `no_match` would be. The probability is the score, and `detail` has the match weight and the level each comparison fell into. The model is a file beside the strategy, written by a training script that is committed with it and reads no truth labels. No comparison in it adjusts for how common a value is, because Splink would count that in the records of each run, and the same pair would then score differently depending on what else had arrived.
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

A benchmark is a **scenario**: an ordered list of steps (ingest this batch, apply this correction, switch to this strategy) with truth at each step. A single-batch dataset is a one-step scenario. The specification is a data file, not code. A scenario names the strategy its steps resolve with, a step may name its own, and `spool bench --strategy` names one for a scenario that names none. A step with `rematch = true` matches everything again, which is how a scenario switches strategy on data it has already resolved.

A step ingests files, applies corrections, and resolves, in that order. A correction is what a reviewer would do, written against observations named by source and key: merge the identities of two observations, put one observation in the identity of another, split observations out of their identity, record a `must_link` or a `cannot_link`, or undo an earlier correction by name. Each is made through the call any caller would use. A step of corrections alone shows what the next run makes of them. Switching strategy within a scenario comes with the second strategy, in Milestone 4.

Resolution metrics, per step:

- Blocking: pair completeness and reduction ratio.
- Pairs: precision and recall, reported separately. F1 is shown but is not the headline, for the reasons in the [survey](landscape-survey.md#5-benchmark-datasets-and-evaluation-practice).
- Clusters: B-cubed precision and recall, the count of false merges (identities containing more than one true entity) and false splits (true entities spread over more than one identity), and the share of observations left unassigned.

Identity metrics, across steps. These are the ones no public benchmark has:

| Metric | Definition |
| --- | --- |
| ID stability | Of the observations present before and after a step, the share whose identity ID is unchanged. Reported strictly and after following redirects. |
| Churn | Identities created, merged and observations reassigned per thousand new observations. The counts compare the state before the step with the state after it. |
| Order sensitivity | Agreement between final partitions when the same batches arrive in different orders: the steps as written against the same steps reversed, both without corrections. Reported as the share of observations grouped with the same observations either way, and as the pairs together in both orders over the pairs together in either. |
| Upgrade stability | After a step that matches everything again with a strategy: of the observations that had an identity, the share whose identity ID is unchanged; the merges and splits the strategy suggested; and the changes to existing identities that were committed during the step. |
| Split recovery | After a scripted wrong merge and a manual split, whether later batches keep the two sides apart: of the splits the scenario made, the share with no identity holding observations of both sides. |
| Override persistence | Share of constraints still honoured after further batches and a strategy change: of the constraints the scenario recorded, those still in force with state that agrees with them. |

The company fixture has a second scenario, `corrections.toml`, for these. Between the two batches a reviewer makes two wrong merges, undoes one, splits the other, merges a pair the rules missed, and at the end places the record that was left for review. With the default policy, split recovery and override persistence can only be complete: the policy never merges identities, and the validator rejects a changeset that breaks a constraint. They become gates when a strategy's suggestions are applied. Order sensitivity is already less than complete on this fixture. Reversing its two batches leaves 32 of 39 observations in the same group, because a record that arrives before the one that would have connected two identities gets an identity of its own, and the policy does not merge afterwards.

Upgrade stability is complete with the default policy, as split recovery is: a rematch commits nothing about an identity that exists. What the metric shows is the other half, how much the new strategy would change. The person project has two scenarios for it. `upgrade.toml` resolves two batches of the committed sample with the rules and then has Splink judge every record again: all 16 records keep their identity, and Splink suggests two merges. One is right, a record the rules left on its own. The other would join two different people who share a surname, a city and most of an e-mail address. `fake-1000-upgrade.toml` does the same on `fake_1000`: 1,000 of 1,000 records keep their identity and 66 merges are suggested.

Resolved from the start with Splink, `fake_1000` has pair recall 0.76 where the rules have 0.72, both with no false merge. On the sample's second batch Splink matches one new record to two identities, which is the wrong merge above arriving as evidence: the record is left for review, and no identity is merged.

Extraction is scored on its own. A step can store a directory of documents and run extractors over them before it resolves, and the scenario's truth can be labelled mentions: a file that says which document mentions which true entity at which place. Two things are then reported:

- **Extraction**: the mentions found against the labelled ones, as precision, recall and F1. A mention is right when a label has the same document, entity type, start and end. Half a name is a wrong mention and a missed one.
- **Resolution**, with the metrics above, over the observations that have a true entity. For a mention that is the label at the same place. A mention the extractor made up has no true entity and is left out, so a bad extractor lowers the first score and not the second.

Running the same documents through an extractor that reads the labels gives resolution with perfect extraction, which is the other half of telling the two apart. `benchmarks/extraction/company-news` does both. It is ten invented news items in two batches, with 25 labelled mentions of ten companies, and the cases that make each half hard: names without their legal form, a capitalised word before a name, a product called "Falcon Ltd edition", accents a wire service dropped, an emoji before a name, a short name, a similar name, and two unrelated companies with one name. Its rule-based extractor finds 21 mentions, 18 of them labelled (precision 0.86, recall 0.72). Resolution over those 18 has pair recall 1.0 and one false merge, the two companies with one name. With the labels as the extractor, resolution over all 25 has pair recall 0.86, one false merge and one false split: "Northwind" on its own does not match "Northwind Traders Ltd", a miss the rule-based extractor hides by not finding the short name at all. The rule-based extractor is written in Python and again in TypeScript, and the two produce the same observations, ID for ID.

Datasets for day one: Splink's `fake_1000` as a smoke test, `historical_50k` and FEBRL as mid-sized checks, and a small hand-built company fixture committed to the repository that contains the hard cases of company resolution (aliases, legal versus trading names, domain changes, subsidiaries, records with few attributes), split into two batches. Each fixture is a project directory with its own entity type and rules. The three public datasets are person records and the fixture is company records, so the benchmark runs two unrelated entity types through one engine from the start. The public datasets are downloaded and checksummed at benchmark time, because their redistribution terms have not been confirmed.

Results are written as JSON. CI compares them with a committed baseline and fails on a regression, with false merges and ID stability treated as the strictest gates.

For a real company benchmark, the [GLEIF register](landscape-survey.md#5-benchmark-datasets-and-evaluation-practice) is the best public source the survey found: CC0, LEI as ground truth, real former and trading names, parent links for hard negatives, and 6,521 records flagged as duplicates of another LEI. A scenario generator on it has not been built.

Metrics are implemented in Spool. The existing `er-evaluation` package is AGPL.

**Decide now:** truth relation, scenario format, false merges and ID stability as first-class metrics, baseline comparison in CI. **Defer:** the GLEIF scenario generator, sampling-based estimation for unlabeled data, a public extraction dataset, partial credit for a mention that overlaps a label, one end-to-end score from documents to entities, cost and latency reporting.

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

Milestone 3 implements the operations that Milestones 1 and 2 only declared: `Merge`, `Constrain`, `Unconstrain` and `Reinstate`. Its exit criteria, each a test:

- The in-memory state and the DuckDB store pass one conformance suite for all six operations, and agree on random changesets of all six and on the undo of each (`spool-core` and `crates/spool/src`).
- A merged identity's ID answers with the identity that stands for it, through any chain of merges.
- A merge written before its identities changed is rejected and names the operation.
- No committed state breaks a constraint. A changeset that would is rejected and names the constraint, and a run leaves for review what a constraint forbids.
- Undoing a merge brings the identity back under its own ID with the members it had, and undoing the undo merges it again.
- A split gives the smaller side a new identity and keeps the sides apart: a later batch puts nothing in both.
- A row that changed at its source supersedes its earlier version and joins its identity.
- A dismissed review item stays closed, and its observations are not examined again.
- Rebuilding the projection from the log reproduces the state after every kind of operation.
- The corrections scenario reports ID stability, churn, split recovery, override persistence and order sensitivity, and gives the same report for the same seed (`crates/spool-cli/tests`).
- Each of these is reached through the API and through the TypeScript client (`crates/spool-cli/tests`, `packages/client/test`).

Milestone 4 adds user code: `crates/spool/src/usercode.rs`, which runs a project's strategy files, the two helpers, `packages/strategy`, which the TypeScript helper is built from, and Splink as a strategy of the person project. Its exit criteria, each a test (those that start `uv` or `bun` run with `--include-ignored`):

- The company fixture's rules written as a Python strategy and as a TypeScript strategy commit the changesets the SQL rules commit, with the same content hashes.
- A strategy that fails, writes nothing, or does not finish in time fails its run, changes nothing, and leaves its task directory to be looked at. The next run examines the same observations.
- Judgements from user code are checked as a caller's are: an unknown observation, or a pair of observations that both have an identity, is refused.
- Only the file the manifest was compiled from runs. A file that changed is refused until the project is built again, and a name the project does not have runs nothing.
- A strategy is started without the server's token, and with paths that do not depend on where the engine was started.
- The engine answers reads and commits writes while a strategy runs.
- A strategy that finds its own pairs is given every observation with its identity.
- A rematch moves no observation that has an identity, and lists the merges and splits the strategy would make.
- Splink resolves the person sample in two batches with no false merge, and leaves for review the record it matches to two identities. The same records give the same changesets.
- Switching from the rules to Splink keeps every identity ID and reports upgrade stability, on the sample and on `fake_1000`.
- On `fake_1000` Splink has higher pair recall than the rules and no false merge.
- A project's strategies are listed by `info` and by the API, run through the server and through the TypeScript client, and a strategy that fails is reported as `strategy_failed` (`crates/spool-cli/tests`, `packages/client/test`).
- `build` writes a Dockerfile that carries the runtimes the project's strategies need, and `build --install` starts each strategy once.
- The TypeScript helper the engine carries is the one `packages/strategy` builds.

Milestone 5 adds documents and extractors: `crates/spool/src/documents.rs` and `crates/spool/src/engine/extract.rs`, the extractor half of the two helpers, and the news benchmark. The store's format version is 5. Its exit criteria, each a test (those that start `uv` or `bun` run with `--include-ignored`):

- A document is stored once, a changed text is its next version, and an extractor reads only the latest version, once.
- Mentions become observations that say where they were found, and the next match run resolves them with the type's rules. Repeating an extraction creates nothing.
- The same extractor written in Python and in TypeScript produces the same observation IDs, with text that holds accents and an emoji.
- An extractor that fails, or returns a mention the engine refuses, fails its run, stores nothing, and leaves its task directory to be looked at.
- A match run that begins while an extractor works does not hide what the extractor finds from the run after it.
- Each entity type's `extract.md` reaches the extractor, and editing it rebuilds no feature table.
- A scenario with labelled mentions reports extraction apart from resolution. The news benchmark gives the numbers of section 9 with its rule-based extractor, and with the labels as the extractor.
- A label that does not fit its document is refused and named.
- The example extractor that asks a language model places what the model quotes, and drops what the text does not have (against a stand-in for the model's API).
- Documents and extractions are reached through the API and the TypeScript client, a project's extractors are listed by `info` and by the API, and one that fails is reported as `extractor_failed` (`crates/spool-cli/tests`, `packages/client/test`).
- `dev` runs an extractor as it was last saved, and `build` writes a Dockerfile that carries what the project's extractors need.

Out of scope after Milestone 5: LLM matching, an extractor built into the engine, documents that are not text, relationships between entities, canonical attribute views, the studio, applying a rematch's suggestions automatically.

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
| A project directory with path-derived names, compiled to a manifest; state outside the image | Mountable packages of entity types; `review.md` |
| Entity types declared by the project; feature tables generated from a query; rules as files; no built-in types | Feature columns from user code; client types generated from a manifest |
| User code as child processes over Parquet files; evidence only; workers inside the service; only files the manifest was compiled from run | More than one user-code process at a time; separate worker containers; a streaming protocol; per-task resource limits |
| Extractors as project-level user code; documents as text; a mention keyed by its document version, entity type and code-point span; mentions stored under their own ingest run; reading again only when asked | An extractor built into the engine; documents that are not text; retiring the mentions of a superseded document; extraction in batches |
| Judgements and draft changesets in place of proposals | Entity-centric judgements; calibrated probabilities |
| Review items kept in the store and examined again by later runs, until a changeset places their observations or a reviewer dismisses them | Reopening a dismissed review item |
| Minted UUIDv7 identity IDs, permanent redirects, content-derived observation IDs | Public ID formatting; binary IDs in Arrow |
| Changeset log as truth, projection in the same transaction, per-operation preconditions | Compaction, hash chaining |
| Operation set: create, assign, merge, constrain, unconstrain, reinstate; split and undo as changesets the engine writes | Constraints that choose between identities during a run; operations on more than two observations |
| Default policy never merges or splits existing identities automatically; a rematch suggests and does not change | Policy plug-ins; automatic merges above a threshold; applying suggestions in bulk |
| JSON and Arrow as the only wire forms; the run ID as the task handle; drafts stored on the server | Streaming run progress; an ad hoc SQL endpoint |
| Scenario-based benchmark with identity metrics and a CI baseline; extraction scored by exact span, apart from resolution | GLEIF scenario generator; a public extraction dataset; partial credit for overlapping spans; one end-to-end score |
| Two library crates and one for the command and server; injected ID generator and clock; format version in the file; the prebuilt DuckDB library found by run path | Further crate splits; a statically linked DuckDB; indexes and keys |
