# Spool architecture proposal: initial prototype

A minimal Rust-first design for the first Spool prototype, grounded in the [landscape survey](landscape-survey.md). Dated 2026-11-13, revised 2026-11-16. Nothing here is built. Where the text says something was tested or measured, that was a throwaway local experiment with DuckDB 2.0.0, through its Rust bindings or its Python package, not a benchmark.

Spool's initial roadmap had seven milestones: (1) a Rust engine on DuckDB, (2) TypeScript and Python bindings, (3) the identity lifecycle across successive batches, (4) interchangeable resolution strategies, (5) extraction from unstructured documents, (6) sandbox synchronisation with a shared store, and (7) developer tooling. This proposal changes that roadmap. Unless the text says "initial", "Milestone N" refers to the proposed roadmap at the end of section 10.

## Summary of conclusions

1. **Split the system into identity state and evidence.** Identity state (identities, which record belongs to which identity, manual constraints, the log of changes) is small, typed and transactional, and its rules live in a pure Rust crate. Evidence (records, candidate pairs, match judgements) is large and analytical, and lives in DuckDB tables that strategies query with SQL. Only identity state needs to be independent of DuckDB.
2. **Replace a single "proposal" concept with two sharper things.** Strategies emit *judgements*: pairwise evidence with a verdict of match, no match or unsure. A framework-owned *reconciler* turns judgements plus current state into a *draft changeset*. Extensions never write identity operations, which makes Spool's central invariant, that extensions may propose identity changes but only the framework commits them, enforceable by construction.
3. **Identity IDs are minted, never derived.** A UUIDv7 is assigned once, is never reused, and a merged ID redirects to its survivor permanently. By default the matcher can attach new records to existing identities and create new ones, and can only *suggest* merging or splitting existing ones. That is what makes the design goal that a better matching model must not change existing IDs hold.
4. **History is a hybrid.** An append-only changeset log is the source of truth. Current-state tables are a projection written in the same transaction and rebuildable from the log. Every operation carries its own precondition, so a draft that waited for review is checked against the state it is committed to.
5. **The matcher contract is a table shape, not a trait.** Records in, `(left, right, verdict, score, detail)` out. Blocking, scoring, Splink, an LLM and a human reviewer all fit it. Clustering happens only in the reconciler.
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
| Three core abstractions: records, proposals and changesets | Keep records and changesets. Split proposals into judgements (evidence from strategies) and draft changesets (intent from the reconciler). | "Proposal" mixes what a matcher saw with what should change. A matcher that proposes a merge has to know identity state and operation semantics, which is the coupling Spool's design goals set out to avoid. nomenklatura already works this way ([survey, section 4](landscape-survey.md#nomenklatura-and-opensanctions)). |
| Local-first and sandbox-native: each sandbox resolves entities locally and synchronises changesets to a shared store (initial Milestone 6) | One server owns the store and does the work. Applications and agents call it. Nothing is synchronised between copies. | Matching a new record needs the existing evidence, so every sandbox would need a copy of it. While one process has a DuckDB file open, no other process can open it. One writer removes sync transport, conflict policy and duplicates created by two copies at once. Local development stays: `spool dev` is the same server on localhost. Section 5 has the full argument. |
| TypeScript and Python are first-class interfaces to the engine through napi-rs and PyO3 bindings (initial Milestone 2) | No native bindings. A TypeScript client, and later a Python client, call the server's API. Python and TypeScript user code runs as child processes of the server. | With one server, a caller needs no engine in its own process. Bindings need native packages per platform and a matched set of crate versions: `pyo3-arrow` 0.21 pins `arrow` 60 and `pyo3` 0.30, while `arrow-pyarrow` 60 still wants `pyo3` 0.29. A client is a small package with no native code. |
| The Rust engine does not run Python or JavaScript; the language SDKs orchestrate extensions written in their language | The engine still embeds no runtime. The server starts user code as child processes and exchanges files with them. | A server has no host language to drive the loop. Child processes keep every runtime out of the engine, and one file protocol serves any language (section 6). |
| An eventual layout of seven crates: core, DuckDB store, resolution, benchmarks, Node bindings, Python bindings and CLI | Two crates for Milestone 1, three after Milestone 2. | The initial plan treats these as logical boundaries and allows starting with fewer crates. The built-in strategy is SQL run through the store and the benchmark needs both, so `spool-duckdb`, `spool-resolution` and `spool-bench` would have no independent boundary yet. |
| Core identity semantics are independent of any particular database | Apply it to identity state only. Let evidence handling be openly DuckDB-specific. | Abstracting the analytical engine means a SQL dialect layer. Splink carries `sqlglot` and per-backend code for this. A second store for identity state would need the log and the projection, not candidate generation. |
| Initial Milestone 1 creates identities; initial Milestone 3 preserves them across batches | Move "second batch keeps its IDs" into Milestone 1. | Otherwise Milestone 1 validates nothing that Splink plus a UUID column would not. |
| Initial Milestone 6 defines identity changesets | Define the changeset format and preconditions in Milestone 1. | Changesets are the only way state changes. Retrofitting preconditions and stable record IDs after data exists is a format migration. |
| DuckDB relations are one of the cross-language data representations | Relations stay inside the engine. SQL text goes in and Arrow comes out. | A relation is a handle into one process's DuckDB instance. A second process cannot open the file at all while the engine holds it, even read-only (tested). |
| The core owns canonical entity representations | Core owns *which* records form an identity. Canonical attributes are derived views over evidence. | Storing a golden record as identity state makes every attribute rule an identity operation and works against the goal that evidence stays traceable. Relationships and temporal attributes are derived the same way. |
| Persistent identity is an unoccupied space in open source | It is partly occupied. | nomenklatura, GoldenMatch and Zingg Enterprise each do part of it. The gap is narrower and is described in the [survey](landscape-survey.md#7-borrow-avoid-and-the-gap). |

## The model

Five concepts. The first two are evidence, the last three are identity state.

- **Record.** Something a source said about one entity, kept as it arrived and never changed: a CRM row, or a company mention extracted from a PDF report. Its ID is derived from its source, its key within that source and its content, so ingesting the same thing twice is a no-op. The key is a row key, or the locator of an extracted mention, so two mentions at different places in one document are two records. A changed source row is a new record that names the one it supersedes.
- **Judgement.** A strategy's statement about two records: `match`, `no_match` or `unsure`, with an optional score and explanation. Judgements are kept, are attributed to a run, and never change state by themselves.
- **Identity.** A minted ID with an entity type and a status: active, or merged into another identity. It has no attributes of its own.
- **Constraint.** A human or agent decision that two records must or must not share an identity. Constraints are state: they are changed by changesets and enforced on every later changeset.
- **Changeset.** An ordered list of operations applied atomically, with an author, a reason and the evidence run it came from. It is the only way identity state changes.

An entity type is a name the user declares, such as `company`, `person` or `product`. Spool ships none. To identity state a type is only that name. What an entity of that type looks like is an evidence-side question, answered by the type's declaration in section 1.

Invariants the core enforces on every commit:

1. A record belongs to at most one identity. Belonging to none is legal and means the system abstained.
2. Records are assigned only to active identities, and all members of an identity share its entity type.
3. An identity ID is never deleted or reused. A merged identity points at an active one.
4. No committed state violates a constraint.
5. The current-state tables equal the result of replaying the log.

Spool's central invariant, that extensions may propose identity changes but only the framework commits them, becomes: **extensions supply evidence; the framework decides and commits.** A user or agent may also author a changeset directly, for example a manual split. It goes through the same validation.

## The project directory

A Spool project is a directory in a git repository. It defines the entity types, how they are matched, and how the server is built. The conventions follow eve, a framework in which an agent is a directory of files ([agent files](https://eve.dev/docs/reference/agent-files), [deployment](https://eve.dev/docs/guides/deployment)):

- **The path is the name.** `entities/company/` declares the entity type `company`, and `rules/match/same_domain.sql` is the rule `same_domain`. Nothing is registered by hand.
- **Data files first.** YAML for sources and entity types, SQL for transforms and rules, TOML for settings, Markdown for instructions. Code is used only where data cannot say it.
- **A file replaces the default at the same path.** A project without `policy.toml` gets the default reconciliation policy.
- **A build step compiles the directory into one artefact**, and a command lists what was discovered.

```text
my-project/
├── spool.toml                    # optional project settings
├── sources/
│   └── crm.yaml                  # the file name is the source; the columns its rows may have (section 1)
├── transforms/
│   └── company_name_key.sql      # the file name is the transform; one expression over its arguments (section 1)
├── entities/
│   └── company/                  # the directory name is the entity type
│       ├── entity.yaml           # its features, and how each source's columns become them (section 1)
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
| `spool info` | Lists the discovered sources, entity types, rules, strategies and extractors, with diagnostics. |
| `spool build` | Validates the project and compiles it into a manifest. Every YAML file is checked against the files it names and every SQL file is prepared against DuckDB, so a mistake fails the build, not a run. Writes a `Dockerfile` if the project has none. With `--install` it also starts each user-code strategy and extractor once with nothing to do, which fetches what it depends on. |
| `spool apply` | Applies a manifest to a store: declares entity types and rebuilds the feature tables whose declaration changed. |
| `spool serve` | Runs the server from a manifest and a store location. An empty store is set up from the manifest. |
| `spool show` | Shows what a running server holds: runs, identities, records, documents, review items, drafts and the log (section 5). |
| `spool retract` | Tells a running server that a source no longer holds some of its rows, named by their keys (sections 1 and 5). |
| `spool explain` | Says why two records share an identity, or why they do not (section 5). |
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

## 1. DuckDB representation of records, candidates and identity state

All tables live in a `spool` schema so a user's own tables can share the file. The column types used below or available to feature columns (`UUID`, `JSON`, `STRUCT`, `LIST`, `MAP`) and the `uuidv7()` function are all in the stock DuckDB library (tested).

Evidence:

```sql
CREATE TABLE spool.record (
  record_id      UUID        NOT NULL, -- first 128 bits of sha256(source_id, source_key, canonical payload)
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

CREATE TABLE spool.retraction (           -- rows a source took back; kept. One is in force while restored_run IS NULL
  record_id    UUID   NOT NULL,           -- a record the source no longer holds: it stays as evidence and has no identity
  retract_run  BIGINT NOT NULL,           -- the retract run that took it back
  restored_run BIGINT,                    -- the ingest run in which the source sent the row again
  detached_seq BIGINT                     -- the log position of the changeset that took it out of its identity, if it had one
);

CREATE TABLE spool.entity_type (          -- declared by the project; the engine ships none
  entity_type      VARCHAR     NOT NULL,  -- any name: 'company', 'person', 'product', ...
  version          INTEGER     NOT NULL,  -- increases when the declaration changes
  declaration      JSON        NOT NULL,  -- the features, the label, and the query and transforms compiled from entity.yaml
  declaration_hash VARCHAR     NOT NULL,
  declared_at      TIMESTAMPTZ NOT NULL
);

-- Generated by the engine, one per declared type, as spool.features_<entity_type>.
-- Derived from payload, rebuildable, and what matchers read. Only the first and last
-- columns are fixed. The rest come from the declaration; these come from the example below.
CREATE TABLE spool.features_company (
  record_id           UUID    NOT NULL,
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

CREATE TABLE spool.review_item (          -- what the engine would not decide; kept
  review_id       BIGINT  NOT NULL,       -- dense, assigned by the engine
  run_id          BIGINT  NOT NULL,       -- the run that raised it: a reconcile run, or for a possible_split a retract or an ingest run
  entity_type     VARCHAR NOT NULL,
  kind            VARCHAR NOT NULL,       -- 'possible_merge' | 'conflict' | 'possible_split'
  record_ids UUID[]  NOT NULL,       -- the records left without an identity; for a possible_split, the records of its identity
  identity_ids    UUID[]  NOT NULL,       -- the identities they matched; empty when the conflict is among the records; for a possible_split, the one identity
  status          VARCHAR NOT NULL,       -- 'open' | 'resolved' | 'superseded' | 'dismissed'
  closed_seq      BIGINT,                 -- resolved: the changeset after which every record had an identity, or a possible_split had nothing left to ask
  closed_run      BIGINT,                 -- superseded: the reconcile run that raised it again in another form or whose judgements answered it, or the retract run that took back what it asked about
  closed_by VARCHAR, closed_at TIMESTAMPTZ, closed_reason VARCHAR,  -- dismissed: who closed it, when, and why if they said
  groups          UUID[][]                -- possible_split only: its records as the groups that still hold together
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
  closed_run    BIGINT,                   -- superseded: the reconcile run that drafted the type again, or the retract run that took back a record it places
  closed_at TIMESTAMPTZ, closed_reason VARCHAR     -- rejected: when, and why if the reviewer said
);
-- spool.draft_op holds a draft's operations in the shape of spool.changeset_op below.
```

`spool.features_company` above is not part of Spool. It is what the engine generates from three kinds of file, which are one project's idea of a company. Each knows nothing of the others but the last, which is where they meet.

What comes in. One file per source, named for it. It lists the columns a row may have, and names the one that is the row's key and, if rows say where they were found, the one that holds that evidence:

```yaml
# sources/crm.yaml
key: id
columns: [id, name, website, country, city, registry_number]
```

A transform. One file per transform, named for it: `lambda`, the names of its arguments, and a DuckDB expression over them. It works on the values it is given from one record, and cannot `select` from anything:

```sql
-- transforms/registry_id.sql
lambda country, number: upper(country) || '-' || number
```

What a company is compared by, and how each source's columns become that:

```yaml
# entities/company/entity.yaml
label: name
features: [name, name_key, trading_key, domain, country, registry_id]

mapping:
  crm:
    name: name
    name_key: { from: name, transform: company_name_key }
    domain: { from: website, transform: domain_of }
    country: { from: country, transform: upper }
    registry_id: { from: [country, registry_number], transform: registry_id }
  registry:
    name: legal_name
    name_key: { from: legal_name, transform: company_name_key }
    trading_key: { from: trading_name, transform: company_name_key }
    country: { from: country, transform: upper }
    registry_id: { from: [country, number], transform: registry_id }
```

- **`features`** are the columns of the feature table, which is what rules and strategies read. **`label`** is the feature a record is called by.
- **`mapping`** has a block per source. A feature is a column as it is, or columns given to a transform. A transform with no file is the DuckDB function of that name, such as `upper`. A feature a source does not map is empty for that source's records, and every feature is mapped by at least one source.
- **A column's value reaches a transform as text**, whatever JSON type it arrived with. A transform that returns another type, such as `try_cast(text AS DATE)`, gives the feature that type.
- **What is not declared is refused.** `spool build` fails on a mapping that names a source without a file, a column its source does not list, or a transform given the wrong number of columns. Ingest refuses rows from a source without a file, rows from a source the type's mapping has no block for, and rows with a column the source's file does not list. A column the rows lack is empty.

The engine compiles the mapping into one query over `record`, its view of the type's records, with one row per record. The query, the transforms it calls and the columns DuckDB infers for it are the type's declaration (tested). Another project's `company` can have different features.

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
  record_id UUID, other_record_id UUID,
  entity_type VARCHAR, constraint_kind VARCHAR   -- 'must_link' | 'cannot_link'
);

CREATE TABLE spool.identity (             -- projection
  identity_id UUID NOT NULL, entity_type VARCHAR NOT NULL,
  created_seq BIGINT NOT NULL,
  merged_into UUID, merged_seq BIGINT,    -- always points at an active identity (chains are flattened)
  merge_target UUID                       -- the identity the merge named, which may itself have been merged since
);

CREATE TABLE spool.membership (           -- projection with history; current rows have to_seq IS NULL
  record_id UUID NOT NULL, identity_id UUID NOT NULL,
  from_seq BIGINT NOT NULL, to_seq BIGINT
);

CREATE TABLE spool.constraint_link (      -- projection with history
  record_a UUID NOT NULL, record_b UUID NOT NULL, kind VARCHAR NOT NULL,
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
- **An extracted mention's key is its locator.** `source_key` holds the mention's place in its document: `<document_id>#<entity_type>:<start>-<end>`, where `start` and `end` count Unicode code points and `end` is the position after the last one. Two identical mentions at different places are then two records, and each keeps its own `evidence`. Repeating the same extraction produces the same IDs and is a no-op. The cost is that a document which names one entity many times yields that many records, which all match each other.
- **The locator names a version of the document, not its key.** A document is text under a source and a key, and its ID is derived from all three, so a changed text is another document that names the one it supersedes. Had the locator named the key, a mention at characters 10 to 20 of the new text would supersede whatever was at 10 to 20 of the old one, and a run treats a version and the one before it as a match (below). With the document's ID in it, a locator never spans two versions. What follows is a limit: the mentions found in an earlier version stay as records, and members of their identities, after the document has changed.
- **Documents are text.** Turning a PDF or a web page into text is the caller's work, and a place in a document is a place in that text. Pages and bounding boxes are not recorded.
- **Feature tables are generated from a declaration, never hand-written.** A declaration is one SQL query over the type's records. The engine creates `spool.features_<entity_type>` from its result and fills it with one `INSERT ... SELECT` per ingest. No engine code names a feature column, and `ingest` rejects a type that has not been declared.
- **Changing a declaration changes no identity.** Applying a changed `entity.yaml`, or a changed transform it uses, for an existing type increases its version and rebuilds the feature table from the stored payloads. Identity state refers to records and to the type's name, never to feature columns, so every identity ID stays in place. A match run records the declaration version it read, so its judgements stay traceable.
- **Typed columns, not one generic table.** A single features table holding JSON for every type would need no generation. It would also move normalisation into every matching query and give up the typed columns that blocking joins and similarity functions work on.
- **Operations are rows, not a JSON blob.** A first load is one changeset with as many `assign` rows as records, applied with `INSERT ... SELECT`. When measured, a one-million-operation changeset wrote its log rows and its projection rows in one transaction in 0.4 s.
- **Membership keeps intervals.** "What was this identity at log position N" is a filter, and undoing a merge can read which records it moved.
- **A merged identity keeps both pointers.** `merged_into` is the active identity that stands for it now, so a lookup follows any chain of merges in one step. `merge_target` is the identity its own merge named. Undoing a merge in the middle of a chain needs the second: the identities that were merged into the reinstated one point at it again, and no further.
- **A changed row supersedes the one before.** At ingest, a keyed row whose content is new names the latest version of the same entity type, source and key in `supersedes`. Versions sent in one call follow each other in row order. The earlier version is kept, stays a member of its identity and can still be matched. A run treats a version and the one it supersedes as a match whatever the rules say, so a row that changed joins the identity of its earlier version, or is left for review if it now also matches another. One limit: a source that sends content it has sent before under the same key sends a duplicate, which is ignored, so a row that changes from A to B and back to A ends at B.
- **A row its source takes back stays as evidence and leaves its identity.** A source withdraws rows: a registry strikes a company off, a customer asks to be forgotten by a list. A retraction names rows by source and key, and it covers every version of each row. Nothing is deleted. `spool.retraction (record_id, retract_run, restored_run, detached_seq)` says which records are taken back, and a retraction is in force while `restored_run` is null. The records that had an identity leave it by one changeset of `Assign { record, from, to: None }` operations, recorded under whoever asked, so the log says when they left and why. The retract run's report names the changeset, and `detached_seq` is its log position for each record that left an identity by it. The identity keeps its ID and its other members, and an identity left with no member stays, as after any other change. A record that is taken back has no feature row, so no rule and no strategy sees it, no run examines it, and it does not count as waiting for an identity. The engine refuses any changeset that would give it one, including the undo of the changeset it left by. A draft that would place it is closed as superseded, and a review item is closed as superseded once none of its records still waits. Ingesting a row under the same key, changed or not, ends the retraction of every version: they get their feature rows back, and a version that comes back unchanged counts as a duplicate, with `restored` in the ingest's report. Each goes back to the identity it left, or to the one that stands for it now if it was merged meanwhile. What put it there still holds, whether a run did or a reviewer, so the identity keeps its ID through a retraction even when the record was its only member, and a reviewer's placement is not lost to a rule that would not have made it. The ingest commits one changeset of `Assign { record, from: None, to }` operations for this, as the engine, after the rows are stored; the ingest's report counts the records as `returned` and names the changeset. A record goes back only where a changeset may put it: one that a `cannot_link` made meanwhile keeps out of its identity, or that had no identity when it was taken back, has none and is new to the next match run, as are the new versions of a row that comes back changed. Three limits, and one thing that is reported instead of done. A row without a key cannot be taken back, because nothing names it. Other records that reached an identity only through a record that was taken back stay where they are: the engine never splits an identity on its own. It does say so. The retract run looks at each identity it took a record out of, and where nothing the store knows holds the records that are left together, it raises a `possible_split` review item that names the identity and the groups that still hold (section 4). An ingest does the same for an identity it gave a record back to, since what held the record there may still be gone. A rematch could not report this: it suggests a split where a rule now says `no_match` between two of the members that are left, and says nothing where the two are simply no longer compared. The benchmark of changing data measures how much of what a retraction leaves behind the items cover (section 9). A record that comes back to its identity is placed by no run again, and a record that arrived while it was away was placed without it: if only the returning record says that the two are the same, they stay in two identities until a rematch lists them as a suggested merge. And a retracted record has no label, since a label is a feature. The format version is 8 with this table and 9 with the groups of a `possible_split`; a store of format 7 or 8 is brought up to date when it is opened.
- **No primary keys or foreign keys on the large tables at first.** Uniqueness is guaranteed by the Rust validator and by anti-join inserts, and checked by a `verify` routine in tests. DuckDB constraints did behave correctly when tested (including update and delete-then-insert of a keyed row in one transaction), but index maintenance on bulk loads is a cost that has not been measured, and the identity rules must not depend on the database.
- **Review items are rows, and they are evidence.** `spool.review_item` holds what the engine would not decide, so the question outlives the call that raised it. Most come from a reconcile run and are about records it left without an identity. A `possible_split` comes from a retraction, or from a record that comes back, and is about an identity whose records nothing holds together any more. The table is not identity state: it is not in the log, no changeset mentions it, and replaying the log does not need it. The engine keeps `status` in step with membership inside the transaction that commits a changeset. Section 4 says when an item opens and closes.
- **A draft left for review is a row too.** `spool.draft` holds a changeset that a reconcile run drafted and nobody has committed yet, with its operations in `spool.draft_op`. Like a review item it is not identity state: the log does not mention it until it is committed, and then under the same changeset ID. A match run whose draft waits has the status `drafted`, not `succeeded`. What is new to a run is counted from the last run of the type that succeeded, so the records of a draft that is rejected, replaced or never decided are examined again by the next run. A type has at most one open draft: drafting it again closes the earlier one as superseded.
- **A run that the engine was working on when it stopped is marked failed.** A run is `running` only while a call is inside the engine. Opening a store marks any run it finds `running` as `failed`, with an error that says it was interrupted, so a client reading the run sees an end.
- **Public read surface is views.** `spool.identity_current`, `spool.membership_current`, `spool.review_open` and a generated per-type entity view are documented and stable; base tables are not.

Tradeoffs of declared types:

- A SQL query cannot express every normaliser. Reducing a URL to its registrable domain needs the public suffix list, for example. Feature columns computed by user code (section 6) cover that case and come with Milestone 4.
- The query must be deterministic, or the table is not rebuildable. The engine cannot check that in general.
- The type's name becomes part of a table name, so the engine restricts directory names to a fixed pattern. The query is SQL that the engine runs. It comes only from the project directory, which is built into the server image; the API accepts no declarations.
- Feature columns differ per type, so a client cannot offer static types for feature rows until it generates them from a manifest.

Tradeoff: two write targets per commit (log and projection) and no database-level safety net. Mitigation is a standing test that the projection equals a replay of the log. A simpler log-versus-projection comparison over a million rows took 0.05 s when measured.

**Decide now:** the evidence versus identity-state split, content-derived record IDs, extracted mentions keyed by their locator, entity types declared by the project with versioned declarations and generated feature tables, operation rows, interval membership, review items kept in the store, a format version from the first commit. **Defer:** indexes and keys, feature columns computed by user code, rebuilding a feature table after its declaration changes (Milestone 2), retention of old candidates and judgements, erasing the payload of a record its source took back, a separate table of locators so that repeated mentions share one record, retiring the mentions of a document version that was superseded, documents that are not text.

## 2. Separating Rust identity semantics from DuckDB persistence

`spool-core` depends on `serde`, `uuid`, `sha2` and `thiserror`, and on neither `duckdb` nor `arrow`. The compiler then enforces the boundary. It contains the types, the validator, the reconciler, operation inversion, replay, and an in-memory state used as the reference model in tests.

```rust
pub struct RecordId(u128);   // content-derived, computed only by the engine
pub struct IdentityId(Uuid);      // minted UUIDv7 from an injected generator
pub struct Seq(u64);

pub enum Verdict { Match, NoMatch, Unsure }
pub enum ConstraintKind { MustLink, CannotLink }
pub enum IdentityStatus { Active, MergedInto(IdentityId) }

pub enum Op {
    CreateIdentity { identity: IdentityId, entity_type: EntityType },
    /// Attach (from = None), move, or detach (to = None). `from` is the precondition.
    Assign { record: RecordId, from: Option<IdentityId>, to: Option<IdentityId> },
    /// Moves every current member of `from` to `into` and redirects `from`.
    /// Rejected if either identity's membership changed after the changeset's `base`.
    Merge { from: IdentityId, into: IdentityId },
    Constrain   { kind: ConstraintKind, a: RecordId, b: RecordId },
    Unconstrain { kind: ConstraintKind, a: RecordId, b: RecordId },
    /// Makes a merged identity active again; used only to undo a merge.
    Reinstate { identity: IdentityId },
}

pub struct Changeset {
    pub id: ChangesetId, pub base: Seq, pub actor: Actor,
    pub reason: Option<String>, pub run: Option<RunId>, pub ops: Vec<Op>,
}

/// Read access the validator needs. Batch-shaped so a SQL store answers with one query per call.
pub trait IdentityState {
    fn identities(&self, ids: &[IdentityId]) -> Result<Vec<Option<IdentityEntry>>>;
    fn assignments(&self, obs: &[RecordId]) -> Result<Vec<Option<IdentityId>>>;
    fn members(&self, ids: &[IdentityId]) -> Result<Vec<(IdentityId, RecordId)>>;
    fn constraints_on(&self, obs: &[RecordId]) -> Result<Vec<Constraint>>;
    fn constraints_of_members(&self, ids: &[IdentityId]) -> Result<Vec<Constraint>>;
    /// The log positions at which each identity's membership last changed: what a merge is checked against.
    fn membership_changed(&self, ids: &[IdentityId]) -> Result<Vec<Seq>>;
    /// What undo reads: the records that left an identity at a log position,
    /// and the identity a reinstated identity had been merged into.
    fn left_at(&self, identity: IdentityId, seq: Seq) -> Result<Vec<RecordId>>;
    fn merged_before(&self, identity: IdentityId, seq: Seq) -> Result<Option<IdentityId>>;
}

/// The only way to obtain a `Validated`, and a store accepts nothing else.
pub fn validate(state: &impl IdentityState, cs: Changeset) -> Result<Validated, Vec<Rejection>>;

pub fn reconcile(evidence: &EvidenceSummary, state: &impl IdentityState, policy: &Policy,
                 ids: &mut impl IdGenerator) -> Draft;   // Draft = Changeset + review items
```

The seam between the two halves is **SQL prepares, Rust decides**. DuckDB does the heavy join of judgements against current membership and returns a compact summary per new record (which identities it matched, how many match and no-match judgements, the best score). The reconciler is a pure function over that summary. Core never sees SQL, and DuckDB never decides what a merge means.

The DuckDB store in the `spool` crate implements `IdentityState` and one write: apply a `Validated` changeset by inserting log rows and updating projections in a single transaction. One conformance suite runs against both the in-memory state and the DuckDB store, and property tests compare random operation sequences between the two. A second persistent store for identity state, such as Postgres, would be a third implementation that must pass the same suite.

Split is deliberately not an operation. It is `CreateIdentity` plus `Assign` moves, usually with a `cannot_link` constraint so the matcher does not undo it, in one changeset. The original ID stays with whichever side the author leaves in place; the default policy keeps it with the larger side, as OpenSanctions does. The engine writes that changeset for a caller who names one side: the larger side keeps the ID, the side that was not named on a tie, and one `cannot_link` between the smallest record of each side is recorded unless the caller declines it.

What the validator holds a changeset to, beyond each operation's precondition:

- **A merge** needs two different active identities of one entity type whose membership has not changed since the changeset's `base`.
- **A record moves once per changeset**, whether an `Assign` or a `Merge` moves it, and an identity is merged or reinstated once. Operations are checked in the order written, so an `Assign` out of an identity comes before the `Merge` of that identity. The rule keeps a changeset's meaning independent of how a store applies it, which is set-based.
- **A constraint** names two different records of one type. A pair has at most one constraint in force, of either kind, and a constraint is recorded or removed once per changeset.
- **The state a changeset leaves breaks no constraint in force.** The two records of a `must_link` share an identity, and those of a `cannot_link` do not. A record without an identity breaks nothing. The rejection names the constraint, the two identities, and the last operation that had a part in it. A changeset can therefore move one record of a `must_link` only by moving the other with it or by removing the constraint first.

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

Each operation is its own precondition. `Assign.from` must equal the record's current identity. `CreateIdentity` needs an unused ID. `Merge` needs both identities active and their membership unchanged since the changeset's `base_seq`, because a merge moves every member and its author saw only the members that existed then. The membership intervals answer that check without a new field. Commit checks these against current state and does not require `base_seq` to be the latest.

One server and one writer do not remove the need for this. A draft can wait for review while other runs commit, and two clients can author changesets at the same time. Drafts that touch different identities all commit cleanly, in whatever order they are approved. A stale one is rejected with the exact operation that conflicted.

Undo is a new changeset computed by the framework from the log and the membership intervals: the inverse of a merge reinstates the merged ID and moves its former members back, so anyone who stored the old ID gets the right entity again. History is never rewritten.

The inverse reverses the operations last first. An `Assign` is reversed. `Constrain` and `Unconstrain` undo each other. A `Merge` becomes a `Reinstate` and the `Assign`s that move back the records the merge moved, leaving out any that are no longer in the surviving identity. A `Reinstate` becomes the `Merge` it had undone, so an undo can itself be undone. `CreateIdentity` is not undone: an identity ID is never taken back, and an identity left without members stays. The undo is validated like any other changeset, against current state. If state has moved on so that an operation no longer holds, it is rejected and names that operation, and nothing is undone by halves.

DuckDB as the log store has one measured cost: small transactions took about 5 ms each (five statements, durable file), so roughly 200 commits a second. That is fine for batch reconciliation and interactive corrections, and not suited to per-record streaming. Batching operations into fewer changesets is the answer for now.

**Decide now:** hybrid, per-operation preconditions, permanent redirects, undo as a forward changeset. **Defer:** snapshots and log compaction, hash-chaining the log for tamper evidence. **Not planned:** synchronising changesets between stores.

## 4. Minimal data contracts that keep matchers interchangeable

Three relation shapes, each an Arrow schema:

| Relation | Columns | Produced by |
| --- | --- | --- |
| Features | `record_id` plus the columns the entity type's declaration defines | Framework, from the declaration |
| Candidates | `left_id`, `right_id`, optional `rule` | Strategy, or the built-in blocker |
| Judgements | `left_id`, `right_id`, `verdict`, optional `score`, optional `detail` | Strategy |

Plus one rule about scope: a run is told which records are new, and it must judge new-versus-existing and new-versus-new pairs only. New means ingested since the last match run of the type that succeeded, plus the records of open review items that still have no identity (below) and the records that came back since that run after their source had taken them back. A record that is taken back is never in scope, on either side of a pair. When the engine matches, reconciles and commits in one call, it records the match run as succeeded only after the commit, so a call that is cut short leaves nothing behind: the next one examines the same records again. The engine rejects judgements that name unknown records.

Candidates and judgements have the same shape for every entity type. Features do not: their columns come from the type's declaration. A strategy is therefore written against a declaration's columns, and nothing in the engine or the contract fixes what those are.

A strategy is anything that fills the judgements relation. That covers a SQL statement, a Rust function, a Python model, an LLM call and a human reviewer. Blocking and scoring can be one step or two; the contract does not care. In Milestone 1 the single built-in strategy is a plain Rust function that runs the SQL rules in the project directory (section 8). There is no `Matcher` trait until a second native strategy exists.

What the contract leaves out, on purpose:

- **Comparable scores.** A Splink match weight, a logistic probability and an LLM's "yes" are not on one scale. The verdict is the common currency and each strategy applies its own threshold.
- **Clusters.** A strategy that natively produces groups (dedupe, in-context LLM clustering) has its output expanded to pairwise `match` judgements by its adapter. Transitive closure is the reconciler's decision.
- **Identity IDs.** Strategies see records. They can read current membership as an input feature and cannot write it.

The reconciler's default policy for Milestone 1 is conservative:

1. A new record with `match` judgements to members of exactly one identity, and no `no_match` to that identity, is assigned to it. Two new records that would join the same identity this way, with a `no_match` between them, are both left unassigned and flagged: joining would put two things that were judged different into one identity.
2. New records that match only each other form a new identity, unless the group contains a `no_match` pair or a `cannot_link` constraint, in which case it is left unassigned and flagged.
3. A new record that matches members of two or more identities is left unassigned, and a "possible merge" review item is recorded.
4. Existing identities are never merged or split by judgements alone.

Rule 4 is what protects IDs from model upgrades: rerunning a better matcher over old data yields a list of suggested changes, not changes. The cost is that this greedy, incremental policy depends on arrival order, which is why order sensitivity is a benchmark metric in section 9.

Rerunning a matcher over old data is a **rematch**: a run whose scope is every record of the type, not only the new ones. It is the one kind of run that may judge two records that both have an identity. The policy is the same, so those judgements move nothing. The run reports them as suggestions: each pair of identities with a `match` between their members is a suggested merge, and each identity with a `no_match` between two of its members is a suggested split, both with the counts of pairs for and against. A reviewer who agrees commits a merge or a split, which is a changeset like any other. Records that have no identity are placed by a rematch as by any run.

Constraints and superseding versions enter here as evidence that outranks the strategy's. Before the summary is built, the judgement on a pair is replaced, or added where the strategy gave none: a `must_link` in force reads as `match`, a `cannot_link` as `no_match`, and a version and the one it supersedes read as `match`, with a constraint ahead of supersession. The policy is unchanged and needs no rule about constraints. A `cannot_link` against the only identity a record matches leaves it to form its own. A `cannot_link` against one member of an identity it otherwise matches is a conflict, and is left for review. A `must_link` to a member of an identity places the record there when nothing else matches. What a run reports as the strategy's judgements are still the strategy's.

Whenever the policy leaves records unassigned, the reconcile run records a review item in `spool.review_item` (section 1). Its kind is `possible_merge` when the records match members of two or more identities, and `conflict` when a `no_match` contradicts their matches. The item names the records, the identities they matched and the run that raised it. It is evidence, like a judgement: it changes no identity, and nothing is committed on its account.

A review item is open while any of its records has no identity. It closes in one of three ways:

- **Resolved.** A changeset gives the last of its records an identity, and the item records that log position. Any changeset does this, whether a reviewer wrote it or a later run drafted it.
- **Superseded.** A later run examines the records and raises an item that differs, for example because they now match a third identity. The old item records that run.
- **Dismissed.** A reviewer closes the item without placing its records, and the item records who, when and why. The records stay without an identity and leave the scope of later runs, so the question is not raised again. A changeset can still place them.

The records of open review items stay in scope. Every later match run of their entity type examines them again, alongside what was newly ingested. An item is therefore reconsidered whenever state or rules may have changed: once a reviewer has moved the members of one identity into the other, or a rule has been corrected, the record matches one identity and is assigned. A run that reaches the same conclusion leaves the item as it is. Assigning a record that had no identity changes no existing ID, so this does not weaken rule 4.

This has two costs. A type with open review items gets a run even when nothing was ingested. And a record under review that matches a later arrival holds that arrival back with it, exactly as it would have if the two had arrived together.

A `possible_split` is the other way round. Its records have an identity, and the question is whether they should share it. Records come to share an identity through evidence, and that evidence often runs through a third record. When the source of that record takes it back, the others stay, and nothing may be left that says they are one thing. No rule says they differ either, so no run reports it. The engine therefore sorts the members of an identity that lost a record into groups. Two members are in one group when a chain of these leads from one to the other through members only:

- the latest judgement of the two by a run that did not fail is a `match`;
- a `must_link` in force names the two;
- one is an earlier version of the other;
- a decision joined them. A changeset that is not the engine's and places records in an identity says they belong with the members it had, and one that merges identities says the same of theirs. It joins whole sides: the records it placed, the members of each identity it merged away, and the members that were there. A side that has since come apart itself is joined with nothing, because the decision does not say which part of it was meant. The undo of a merge says nothing about the records it puts back.

One group is the common case and nothing happens. More than one is raised as a `possible_split` item that names the identity and the groups. The engine does not split: the groups may well be one thing, and only what joined them is gone. An ingest that gives a record back to the identity it left does the same check, because what held the record there may still be taken back.

Such an item differs from the others in three ways. It makes no work for a match run, since its records wait for nothing. The rule that closes an item once none of its records waits does not apply to it. And dismissing it keeps no record out of later runs. There is at most one open item for an identity, and its records and groups are kept in step with the identity: in the transaction that commits a changeset which touches it, and when a reconcile run has judged. A record that joins is in the group it matches, and one that leaves is gone from the item, so a split made along the groups is a split of the identity as it is. It closes like this:

- **Resolved.** A changeset left the identity's records in one group, or left it only one of them: a reviewer split it, or recorded a `must_link` between the groups, or the record that was taken back came back, or a run placed a new record that matches both groups, or a whole group left.
- **Superseded.** A reconcile run's judgements hold the groups together and it committed nothing that touches the identity, as a rematch may.
- **Dismissed.** A reviewer closes it and the identity stays as it is. A later retraction from the identity, or a record that comes back to it, asks again.

Three limits. A retraction that was cut short between its changeset and its report raises no item when it is sent again, because the identities the records left are no longer known from the call. An item names every record of its identity, which is long for a large one. And an identity can also be left without what held it together when a row changes: a new version of a row may no longer match what the earlier version matched, and the earlier version, which stays a member, still does. Nothing reports that yet.

Tradeoff: pairwise judgements fit entity-centric strategies awkwardly. An LLM asked "which of these five existing companies is this?" answers about an identity. For now its adapter emits a judgement against a member record and records the reasoning in `detail`. Whether to add record-to-identity judgements is an open question.

**Decide now:** the three relations, verdict as the contract, clustering only in the reconciler, no automatic merges of existing identities by default, review items that persist and are examined again by later runs. **Defer:** a strategy trait, policy plug-ins, entity-centric judgements, calibrated probabilities, closing a review item without giving its records an identity.

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
    pub fn run(&self, run: RunId) -> Result<RunEntry>;                       // the task record
    pub fn identity_of(&self, record: RecordId) -> Result<Option<IdentityId>>;
    pub fn current_id(&self, id: IdentityId) -> Result<IdentityId>;          // follows redirects
    pub fn query(&self, sql: &str) -> Result<impl RecordBatchReader>;        // read-only, against the views
}
```

The listing is the core. The library also has typed reads for everything the API returns: runs, drafts, review items, identities with their members and history, records, the log, and a summary of the store.

`spool serve` wraps that type in an HTTP API. The server is the only process that opens the store. Applications and agents reach it through a client, and Spool does not run inside the caller's process or sandbox. Tests and benchmarks use the library directly, with no server.

Why one server, and not an engine in each caller:

- **Matching needs the existing evidence.** Resolving a new record means comparing it with what is already known. An engine in each caller needs a copy of that evidence, and choosing which part to copy is the blocking problem again.
- **The server can be sized for the work.** A caller such as an agent sandbox may be small and short-lived. A network call to a larger machine is the better trade for analytical joins.
- **A DuckDB file has one owner.** While the engine has the file open, no other process can open it, even read-only (tested). One server that owns the file fits that.
- **One writer has nothing to synchronise.** Sync transport, conflict policy, and two copies each creating an identity for the same new entity all disappear.

Every path of the API is under `/v1` and needs a caller the server knows: a person who signed in, or an agent that presents a token. The Access part below describes both. `GET /healthz` needs neither, and nor do the files of the studio, which the Inspection part describes. Small values are JSON. `spool schema` prints one JSON Schema for all of them, a copy is committed at `schema/spool-api.v1.schema.json`, and a test fails when the two differ.

| Call | What it does | Response |
| --- | --- | --- |
| `GET /v1/info` | What the store holds: the manifest hash, the head of the log, and counts per entity type and per source of each | JSON |
| `POST /v1/ingest` | Adds rows as records of one entity type, from a source the project declares. The source's file says which column is the key. The body is JSON with the rows as objects, or an Arrow stream with the other fields in the query string. A row the source had taken back comes back to the identity it left (section 1) | The ingest run, with how many records came back and the changeset that returned them to their identities |
| `POST /v1/retract` | Takes back rows a source no longer holds, named by `source`, `entity_type` and `keys`. Every version of each row stays as evidence and leaves its identity, by a changeset under the caller with the `reason` given; no run examines them until the row is ingested again. A key the source has no row under is counted as unknown | The retract run, with how many records were taken back and the log position where they left |
| `POST /v1/documents` | Stores documents of one source as text, for an extractor to read. A changed text under a key is the document's next version | The ingest run |
| `GET /v1/documents`, `GET /v1/documents/{id}` | Lists documents in pages, without their text; reads one version with its text and each time an extractor read it | JSON |
| `POST /v1/extractions` | Runs one of the project's extractors, which `GET /v1/info` lists, over the latest version of each document it has not read, or with `again` over those it has. What it finds is stored as records | The extract run |
| `POST /v1/runs` | Matches the new records of one entity type, or with `rematch` all of them. With no strategy, the server runs the rules and reconciles, and `commit` says whether the draft is committed (the default) or left for review. The name of one of the project's user-code strategies, which `GET /v1/info` lists, runs that instead. With any other name the run opens and waits for the caller's judgements | The match run, or `null` when nothing was new. A rematch lists its suggestions |
| `GET /v1/runs`, `GET /v1/runs/{run}` | Lists runs; reads one, optionally waiting for it to end | Runs |
| `GET /v1/runs/{run}/candidates` | The candidate pairs of an open run with both sides' features | Arrow, or JSON when the `Accept` header asks for it |
| `POST /v1/runs/{run}/judgements` | Stores judgements, as Arrow or JSON | A count |
| `POST /v1/runs/{run}/reconcile` | Turns the run's judgements into a changeset and commits it, or leaves it as a draft with `commit: false` | The resolution: counts, review items, and the commit or the draft's ID |
| `POST /v1/runs/{run}/cancel` | Gives up an open run | The run |
| `GET /v1/drafts`, `GET /v1/drafts/{draft}` | Lists drafts; reads one with its changeset | JSON |
| `POST /v1/drafts/{draft}/approve`, `.../reject` | Commits a draft, or closes it | The commit, or the operations that were rejected; the draft |
| `GET /v1/changesets`, `POST /v1/changesets` | Reads the log in pages; commits a changeset the caller wrote. A merge, a move and a constraint are each a changeset of one operation. The server mints the changeset's ID if the caller sends none, and takes the head of the log as its base | The log; the commit, or the operations that were rejected |
| `POST /v1/changesets/{seq}/undo` | Takes back the changeset at a log position with a new one, or with `commit: false` only returns it | The undo and its commit, or the operations that were rejected |
| `POST /v1/identities/{id}/split` | Divides an identity in two, given the records of one side. With a `base`, refused if the identity's members changed after that log position | The two identities, what moved, and the commit |
| `GET /v1/constraints` | The constraints in force, or with `removed=true` their history, for one record or one identity's members | JSON |
| `POST /v1/ids` | Mints identity and changeset IDs for a caller that writes its own changeset | JSON |
| `GET /v1/identities`, `GET /v1/identities/{id}`, `.../history` | Lists identities in pages, each with its label and how many of its members came from each source, with `q` those a member of which holds every word, or with `ids` those with the IDs given; reads one with its members, following redirects; the changesets that touched it | JSON |
| `GET /v1/records`, `GET /v1/records/{id}` | Finds records by source and key, those found in one document, the members of one identity, those that wait for an identity (they have none, and their source has not taken them back), or with `q` those that hold every word; reads one | JSON |
| `POST /v1/records/identities` | The identities of many records in one call | JSON |
| `GET /v1/review`, `GET /v1/review/{id}`, `.../case` | Review items, typed, by status, entity type or a record they name; one of them; one with what a reviewer decides it from | JSON |
| `GET /v1/explain?a=&b=` | Why two records share an identity, or why they do not | JSON |
| `POST /v1/review/{id}/dismiss` | Closes a review item without placing its records | The item |
| `GET /v1/people`, `POST /v1/people`, `POST /v1/people/{id}` | Lists everyone with an account; as an admin, adds a person or changes one | JSON |

- **A call that starts work says how long it will wait.** `POST /v1/ingest`, `POST /v1/retract`, `POST /v1/documents`, `POST /v1/extractions` and `POST /v1/runs` take `wait_ms`, 30 seconds if left out. If the run finishes within that time, the response is `200` with the finished run. Otherwise it is `202` with the run as far as it has got, and the client reads `GET /v1/runs/{run}?wait_ms=...` until its status is no longer `running`. The work goes on whether or not anyone waits. The run ID is `spool.run.run_id`, so tasks add no new concept. One limit: a call that finds the writer busy has no run to report until its own work starts, so it can wait longer than it asked to.
- **A run has one of five statuses.** `running` while the engine works on it, `open` while it waits for a caller's judgements, `drafted` while its draft waits for review, and then `succeeded` or `failed`.
- **A run either commits its draft or leaves it for review**, as the caller asks. A draft left for review is how an agent experiments without changing shared identities. The draft is stored on the server and named by its changeset ID, so whoever reviews it need not be whoever started the run. Approving it validates it like any changeset: if state moved under it, the response names the operations that no longer hold and the draft stays open. Section 1 says why a draft nobody commits does not strand its records.
- **Small values are JSON**, defined once with `serde` and described by one versioned JSON Schema. Client types are generated from it. The same serialised form is the log's exchange format. A request with a field the server does not know is refused, so a misspelt `commit` cannot commit by mistake.
- **Bulk data is Arrow**, as IPC streams in request and response bodies. Clients also accept plain objects for convenience, up to some thousands of rows. Rows sent as JSON become the same records as the same rows sent as Arrow: a number with no fractional part is stored as an integer whichever way it arrived, so a record's ID does not depend on the encoding or on the other rows in its batch. A JSON integer that its column cannot hold exactly is refused, not rounded: one of 2^53 or more beside numbers with a fraction, or one past the signed 64-bit range beside a negative one. A JSON row with no values is still a row.
- **IDs cross as strings** in the first version. DuckDB exports `UUID` columns to Arrow as strings by default (tested), which is also what JavaScript and pandas users expect.
- **Errors have one shape.** Every response that is not a success is `{"error": {"code", "message"}}`, with `rejections` when a changeset broke a rule. The codes are `unauthorized`, `forbidden`, `too_many_attempts`, `invalid`, `not_found`, `conflict`, `rejected`, `too_large`, `strategy_failed`, `extractor_failed` and `internal`.
- **A request refused before its body was read still gets its answer, and says whether the connection goes on.** A caller the server does not know is refused before the body is read, and a body over the limit when the limit is passed. Up to 64 KB of what is left is read and dropped, and the connection takes the next request. When more is left, the answer says `Connection: close`, so that a client does not send its next request into a connection that is gone. The server then ends its side and goes on dropping what the client sends until the client has finished, for up to 30 seconds, 5 with nothing arriving, and 1 GB. A server that is stopping gives such connections one second. Closing at once would stop a client in the middle of writing and reset the connection, and macOS, among others, then throws away an answer that had arrived and was not yet read. The code is `crates/spool-cli/src/server/unread.rs`.
- **A server that is told to stop does so within a fixed time, whatever its clients do.** On Ctrl-C or `SIGTERM`, `spool serve` takes no new connection and closes those with no request on them. A request that waits on a run is answered at once with the run as far as it has got, because `wait_ms` has no upper limit and a stop cannot wait for it. Every other request in flight is answered if that takes no more than 5 seconds. A request that a client has not sent in full by then gets no answer, so a client that goes quiet half way through a request does not keep the server running. A refused connection that is being read out has its one second after that, which makes 6 seconds the longest a server keeps its clients. The engine's work is not cut off: the process exits when the runs the engine was given have ended, and a run that is killed first is recorded as failed when the store is next opened. Whether a stop should also end a long run early is an open decision.
- **The API is tested by hash.** With a seeded ID generator, the same fixture must produce the same changeset `content_hash` through the library, through the server with Arrow and with JSON bodies, and through the TypeScript client. Only Rust computes record IDs and content hashes.

### Inspection

A person reads names, and the store holds IDs. Three things the API answers with are there for whoever reads it, and none of them is stored:

- **A label.** A record's label is what a person would call it: its value of the feature its type's `entity.yaml` names as `label`. A type that names none has no labels. An identity's label is the one most of its members have, and of those as common, the one that was ingested first. It is judged from the hundred members ingested first, so that a very large identity costs no more to list.
- **A search.** `q` on `GET /v1/records` and `GET /v1/identities` finds the records that hold every word of it, in a value of the payload, in the source's name or in the key, whatever the case or the accents. For identities, one member has to hold all the words. It is a scan of the records by DuckDB, with no index to keep: on a store of 1.1 million records and 530,000 identities it answers in 0.07 to 0.3 seconds, the more words the longer. It finds what was typed, not what was meant: nothing ranks the answers, and a misspelt word finds nothing.
- **A case.** `GET /v1/review/{id}/case` is a review item with what a reviewer decides it from: its records in full, the identities they matched as they are now, each with its label, its sources and up to 25 of its members, those that were judged first, what the latest run made of each pair of one of its records with another or with any member of those identities, and the constraints in force that bear on the decision: those that name one of its records, and those between members of two of the identities, each with the identities its two records belong to now. The case of a `possible_split` also has `taken_back`: the records that were taken out of the identity when their source took them back and have not come back, with what was judged between them and the records that remain, which is what held the groups together. A judgement names the identity its other record belongs to, because the case holds only some of an identity's members and the judgements are against all of them. A `cannot_link` between two of the identities says a reviewer already decided they are not one, and the engine refuses a changeset that would merge them.

A person who asks why two records are one company, or why they are not, should not have to query the store. `GET /v1/explain` answers for a pair of records from what the store already holds, and stores nothing of its own:

- **Where each one is.** Its identity, and the changeset that put it there: the log position, the actor, the reason, and the reconcile run that drafted it if one did.
- **What each run made of the pair.** Every match run that had the pair as a candidate or judged it, oldest first: the block rules that found it, the verdict with the strategy's score and what the strategy kept with it, and, when the run's draft was left for review, whether that draft still waits, was approved or was rejected.
- **What a reviewer said.** The constraints between the two, with those that were removed, and the review items that name either.
- **What the rules say now.** Each block, `no_match` and `match` rule of the type applied to the two feature rows as they are, with the feature rows themselves. This is the present reading, and it can differ from what a run recorded: the rules or the features may have changed since. The explanation says so when they differ. A run applies `no_match` and `match` rules only to the pairs a block rule found, and the explanation applies them to any pair, so such a rule may meet values it was never written for. When SQL refuses one, as it refuses the cast of a value that does not convert, the explanation reports that those rules cannot be applied to the pair, with what SQL said, and is otherwise complete. The rules read are those the feature tables were built for: the explanation checks, in the transaction it reads in, that the store was applied with the manifest whose rules it has, and begins again when `spool dev` applied another in between.
- **What joins them.** When the two share an identity without having been compared, the shortest chain of members between them. A link is a `must_link` constraint, a later version of the same row, or a `match` judgement of a run that finished, preferred in that order, which is the order a reconcile run listens to them in. No chain means that what joined them was removed since, as a constraint can be, or that a changeset placed one of them directly. The explanation says who placed each.

The answer is data, with a `summary` that says the same in sentences. The engine writes the sentences, so the command, a client and the studio say the same thing, and a test of the engine covers the wording. A sentence names a record as its source and key, and a mention as its document and the span it was found at.

The explanation is about what was recorded. It does not run a strategy again, so for user code it reports the verdict, score and detail the strategy stored and cannot say what the strategy would judge now.

`spool show` and `spool explain` are the same API from a terminal. They are a client: they take `--url` and `--token`, by default the address of `spool dev`, which asks for no token, and never open a store, so they work against a development server and a deployed one alike. `show` lists runs, identities, records, documents, review items, drafts and the log, and shows one of each: an identity with its members, the constraints on them and its history; a record with its payload, its evidence and, for a mention, the text around it in its document; a draft with the operations it would commit; a review item with its records and where each is now. A `possible_split` is shown as the question it asks, its groups, the records that were taken back, and the three calls that answer it. `--json` prints what the server answered. `spool retract` is a client in the same way, and the only command that changes what a server holds: it sends the keys it is given, or those in a file, to `POST /v1/retract`, waits for the run and says what it took back. The commands speak HTTP with `ureq`, a crate the build of the DuckDB library already used, so the lock file gained no package.

The studio is the same API in a browser. The server answers `GET /` and the path of each screen with a page, and the page reads through `/v1` with the TypeScript client, like any other caller. It has a screen for each thing `show` shows and one for `explain`: an overview of the store, runs, identities, records, documents with each mention marked in the text, review items, drafts and the log. A value that names something else is a link to it, and two records ticked in any list lead to their explanation.

The screens are laid out for the questions a person brings to them:

- **What is known about an identity, and where from.** An identity's screen lists each field its records hold, with each value they give it and the records that hold that value, by source and key, and marks a field its records disagree on. An earlier version of a row is left out of that and counted. Below are the records themselves, each with the changeset that placed it there, the constraints on them and the identity's history.
- **What waits to be decided, and from what.** The review screen is the list of open items, each a card from the item's case: the record that waits beside the identities it matched, field by field with the values that agree marked, what the run found for each pair, and what a reviewer already decided about them. What the run found against an identity is said for all of its records, not only those the card shows: what speaks against a match comes first, and when there are many the first are written out and the rest counted by verdict until they are asked for. The decisions are on the card. A merge made from it is written against the log position read before the case was, so the engine refuses it if the identities changed after the card was read. One the engine would refuse is not offered: a record is not placed in an identity a constraint keeps it out of, and two identities a constraint keeps apart are not merged. The card of a `possible_split` asks whether the identity's records are still one thing and says why it asks. It sets the groups that still hold side by side, with the records that were taken back and what the runs had found between those and the records that remain. Under a group is the split that moves it to an identity of its own. The other two answers are below the card: keep the identity as one, which records a `must_link` between the groups, or dismiss the item.
- **Where something is.** A box on every screen takes an ID, a `source:key` or words, and lists the identities and the records that hold them.
- **Which identity.** Where a screen has only the ID of an identity, it shows the identity's label with the last characters of the ID, since two identities can be called the same.

A reviewer also makes changes from it, each on the screen of the thing it changes: a draft is approved or rejected, a review item's records are placed, the identities it matched are merged into one, or the item is dismissed, a record is moved, an identity is split or takes in another, a constraint between two records is recorded or removed, and a changeset in the log is undone.

- **A change is one call of the API, and the engine decides whether it holds.** The studio keeps no rule of its own. A changeset the engine rejects is shown in the engine's words, with the operations it names, and nothing is changed. Operations are counted from 0 everywhere, as the engine counts them.
- **A change is written against what the reviewer saw.** A move names the identity the record was in when the screen was read, and a merge is committed with the log position the screen was read at as its base. If either changed since, the engine rejects the change and the reviewer reads again. Before a merge, the other identity is looked up and its members shown. Reading the screen again moves the merge's base on, so what was looked up before that is taken away and is looked up again: a merge never takes in members the reviewer was not shown.
- **A split says which side stays before it is made.** The engine leaves the larger side under the identity's ID, whichever side was ticked, and the screen says which side that is. Which side is larger depends on every member, so the split carries the log position the screen was read at as its `base`, and the engine refuses it if the identity's members changed after that.
- **A change is recorded under whoever is signed in**, as `user:<id>`, with a reason if they give one. The studio sends no author, because the API takes none: the server knows its caller. Where the log names a person, the studio shows their name.
- **An undo can be seen before it is made.** The engine writes the changeset that would take another back, and checks it, without committing it.

- **It is a few files compiled into the binary.** `packages/studio` is a React application that Vite builds to a page, one script, one style sheet and the four files of its two fonts, with fixed names. Its components are those of Shelf UI, which are copied into the package as source rather than installed, so they are read, changed and built like the rest of it. The built files are committed under `crates/spool-cli/src/studio`, so building the binary needs neither `bun` nor the network, and a test fails if they are not what the package builds. The script carries the licence text of the packages in it, and the style sheet that of the fonts.
- **The files are open to anyone, and what they show is not.** The files are the same for every server and say nothing about its store. The page asks the server who is signed in, and shows a sign-in form when nobody is. It keeps no secret: the session is a cookie the page cannot read. A development server answers that its reader is the `dev` account, so it opens with no question.
- **The page can load and call nothing but its server.** It is served with a content security policy that allows its own script, style sheet and fonts and calls to the server it came from, and no inline script, no other origin and no frame. The server sends no CORS headers, so a page from another origin still cannot read the API.
- **The screen is named by the path of the address**, such as `/identities/<id>` or `/records?source=crm`. The server answers the path of every screen with the one page, and the page names its files from the root, so the studio is served at the root of its origin. A link to a screen, with its filters, can be copied. A browser asks for the files each time and is told when the copy it has is still the one, so a new binary's studio is shown without a hard reload.
- **`spool serve --no-studio` leaves it out**, for a deployment that wants the API alone. `spool dev` always serves it.

A client is a small package that speaks this API. The first is TypeScript, and a Python client is the same amount of work. No client contains engine code, so there are no native packages to build per platform. An agent built with a framework such as eve reaches Spool the same way: a tool in the agent calls the client.

Scope of the server: one store, one writer and one tenant. It scales up, not out. Tenancy and provisioning compute are out of scope, which keeps Spool short of a hosted control plane. Inside the server, reads run concurrently and commits are serialised, so the log's order is the order of commit. Reads use their own connections to the same DuckDB instance, so reading a run or looking up an identity does not wait for an ingest or a resolve in progress. Each read is one transaction, so it sees one committed state however many queries it takes.

### Access

A caller of `/v1` is a person or an agent, and the server says which made a change. No request names its author.

- **People have accounts, kept by the server** in a SQLite file beside the store and named after it: `spool.duckdb` has `spool.auth.sqlite`. An account is a username, a name, a password hashed with Argon2, an email, whether it is an admin, and whether it is on. It has a password, an email, or both: the password signs its owner in here, and the email is who they are to Google. The log names a person as `user:<id>`, the ID of the account, which stays when the name changes.
- **A person who signs in gets a session**, with a username and a password or with Google: a cookie the page cannot read (`HttpOnly`, `SameSite=Lax`, and `Secure` unless the server listens on loopback or was started with `--plain-http`) that ends twelve hours after signing in. Sessions are rows of the same file, under the hash of their ID, so they outlast a restart and a copy of the file signs nobody in. A session is good while the account is on and its password is the one the session began with, so a sign-in that was under way when the password was replaced starts nothing. Ten wrong passwords for a username within fifteen minutes refuse further tries for it, and the server checks four passwords at once: a sign-in beyond that is turned away, as `too_many_attempts`, so that nobody who reaches the server can have it hash as many passwords as they send. The paths that sign people in read a body of at most 64 KiB, since anyone can send one.
- **The first account is made with a link the server prints.** A server that starts with no admin who can sign in prints `/setup#<code>`. Whoever opens it makes the first account, an admin, and the code then stops working. No credential is put in the environment.
- **An admin adds everyone else**, and can change a name, set a new password, make an admin and turn an account off. Nobody signs themselves up. An account is turned off, not deleted, so the log keeps a name for its ID, and one admin who can sign in always remains: one with a password, or with an email where the server signs people in with Google. Turning an account off, or changing its password, ends its sessions. A person changes their own password with the current one.
- **An agent presents a token**, as `Authorization: Bearer <token>`. A token has a name, and what it changes is recorded under it, as `agent:<name>`, so each agent or pipeline is its own author in the log. An admin makes and revokes tokens on the studio's Tokens page or through `/v1/tokens`, and `spool token add`, `list` and `revoke` do the same on the server's machine, where they open the accounts file and not the store, so they work while the server runs. A token is 32 random bytes after `spool_`. It is shown once, when it is made: the file keeps its SHA-256, by which it is looked up, and when it was last used, to the minute. A revoked token is deleted, and its name is free for another. A token reads and writes as a person does, and manages neither people nor tokens. No token is put in the environment: the server reads none from it.
- **A write must come from the server's own pages.** A browser can send the session cookie with a request that another site's page makes. A write whose `Sec-Fetch-Site` is not `same-origin`, or, from a browser that sends none, whose `Origin` is not the host it was sent to, is refused as `forbidden`. A caller that is not a browser sends neither header and is let through.
- **`spool dev` has no sign-in.** Whoever opens it is the account `dev`, an admin with no password, so it listens on a loopback address only and refuses any other. It also answers only a request sent to `localhost` or to a loopback address, on every path: a page of another site can give this machine a name of its own, and the browser then sends that page's requests here as if to its own site, with nothing but the name to tell them by. A caller that is not a browser presents nothing and is `dev` too; one that presents a token must present a real one, and writes under its name. Nobody signs in as it, so a store that was developed on and is then served still prints the link, and `dev` does not count as the admin who must remain. What `dev` changed keeps its name in the log.
- **Passwords are for getting started.** A deployment that people depend on should sign them in through an identity provider, so that leaving the organisation stops them signing in and a second factor is the provider's to require. Google is the first, and [Signing people in](sign-in.md) is what an operator follows.
- **Google says who someone is, and an admin says whether they have an account.** With `SPOOL_GOOGLE_CLIENT_ID` and `SPOOL_GOOGLE_CLIENT_SECRET` set to an OAuth client, the sign-in page offers Google. An admin adds a person by the email of their Google account beforehand, and nobody else is let in: signing in with Google makes no account. Google must answer for the email: it has verified it, and it is an address of Gmail or of an organisation's Google Workspace (the token then has an `hd` claim). A Google account made with another provider's address is refused, since that address may have changed hands since Google checked it, and its former holder would get the account. The first sign-in binds the account to that Google account by Google's ID for it, so a later holder of the address is refused, and an admin who changes the email unbinds it and ends the sessions Google began, for good: each binding has a mark of its own, which its sessions keep, so the same Google account signing in under the new address starts anew. Such a session is good while the account is on, still has the binding the session began with, and the server still signs people in with Google.
- **The sign-in with Google is an OpenID Connect authorization code flow**, with PKCE, a nonce and a state. `GET /auth/google?next=<path>` sends the browser to Google and `GET /auth/google/callback` takes it back, to `next` if that is one of the server's own pages. What the flow must remember between the two is in a cookie of the browser that began it, which the page cannot read and which lasts ten minutes, so the server keeps nothing for a sign-in nobody finishes and a link made for one browser signs in no other. A sign-in that fails goes back to the page with the reason in the address, as `google=unknown` for an email no account has, and the detail goes to the server's log. The server asks Google eight things at once, a sign-in beyond that is told to try again, and each waits ten seconds for an answer, so a Google that is slow holds up nothing else. It reads where Google's sign-in is when someone first signs in, so it starts when Google cannot be reached. It must be told the address people reach it by, `--public-url`, unless it listens on loopback: Google sends the browser back there.

The paths that make a caller one are outside `/v1`: `GET /auth/session` says who is signed in and whether the server still waits for its first account, and whether it signs people in with Google, and `POST /auth/sign-in`, `/auth/sign-out`, `/auth/setup` and `/auth/password` do what they are called. The parts are `password-auth` for Argon2, `tower-sessions` for sessions, `rusqlite` for the file and `openidconnect` for what is exchanged with Google and the checks of what it answers, with its requests sent by the `ureq` the server already has.

### Assistants

The server answers the Model Context Protocol at `/mcp`, so that an assistant such as Claude or ChatGPT can be pointed at it. A tool there is a call of the API: it runs the handler the path under `/v1` runs, as the caller the request came from, so it is held to the same rules and what it changes is recorded under the same name. There are tools to find identities and records, to explain a pair, to read review items, constraints and the log, and to commit a changeset, split an identity, undo a change and dismiss a review item. The tools that write require a reason. Ingesting, match runs, drafts and people are not offered.

The transport is Streamable HTTP with no sessions, and the protocol is the `rmcp` crate's, compiled into the binary. `/mcp` takes the callers `/v1` takes. An assistant that can send a header presents a token of its own and is that agent in the log. Claude and ChatGPT on the web add a server through OAuth instead, and Spool is not an OAuth authorization server. `docs/mcp.md` lists the tools, says how to add the server to each client, and says what was tried.

Two packaging facts to plan around:

- `duckdb` 1.20000.2 and `arrow` 60 require Rust 1.88, and `uuid` 1.29 requires 1.89. Pin them at the workspace root and upgrade `duckdb` and `arrow` together.
- The `duckdb` crate links a prebuilt shared library by default (12 s clean build when measured, but a 96 MB library that must ship beside the binary) or compiles DuckDB from source with `bundled` (about 6 min 40 s on 4 cores when measured; that build was not linked or run). Decided in Milestone 2: the binary links the prebuilt shared library, in development and in the image. It finds the library through its own run path: beside itself, then in `../lib`, then where the build downloaded it. So a development build starts without `cargo run`, and the image puts the library in `/usr/local/lib`. The static build is not offered yet. The engine uses DuckDB's JSON and ICU extensions, which the prebuilt library includes and the `bundled` feature does not: a static build needs the crate's CMake build with both extensions, and has not been built or run.

Tradeoff: a server has to be run, backed up and secured, where a library needed none of that. Every call crosses the network, which is small next to matching work and costly for per-record lookups, so lookups need batch forms. The library remains for Rust callers and for tests.

**Decide now:** the engine as a library with the server as a wrapper, JSON and Arrow as the only two wire forms, the run ID as the task handle, one tenant and one writer, no native bindings. **Defer:** tokens that only read or that expire, streaming a run's progress, a queue so that a call never waits for the writer before it has a run ID, a Python client, a statically linked DuckDB, a published base image, Windows on ARM.

## 6. Python and TypeScript user code without embedded runtimes

The engine embeds no Python or JavaScript runtime and never calls into one. When a project contains user code, the server runs it as a child process and exchanges files with it.

A user-code strategy is a file under `strategies/`. `entities/company/strategies/splink.py` is the strategy `splink` for companies. A `.py` file runs with `uv run --script` and a `.ts` file with `bun run`. Each tool is a single binary that runs the file and resolves its dependencies: a Python file names its own in a comment at its top ([uv](https://docs.astral.sh/uv/guides/scripts/)), and `bun` fetches what a TypeScript file imports ([bun](https://bun.com/docs/runtime/typescript)). Files that belong to a strategy, such as a model, go in a directory beside it with the same name: `strategies/splink/model.json`. Anything else under `strategies/` is reported by `spool info` and not used.

The manifest records each strategy with its runtime, its path and a SHA-256 of the file and of the files in its directory. That hash is the strategy's version in `spool.run`. Before a run the engine checks that the file on disk still has it, and refuses to run one that changed: the project is built and applied again first. It checks every strategy before it starts the first, and each one again as it is started, since a run over several entity types can start its last strategy long after its first. `spool dev` does that as files are saved.

The protocol is a directory of files per task:

```text
$SPOOL_TASK_DIR/
├── task.json             # protocol version, run ID, entity type, strategy, whether it is a rematch, feature columns
├── candidates.parquet    # written by the engine: left_id, right_id, both sides' features as left_* and right_*
├── features.parquet      # written by the engine: record_id, is_new, identity_id, the feature columns
├── judgements.parquet    # written by the user code: left_id, right_id, verdict, score, detail
├── stdout.log
└── stderr.log
```

The engine writes the inputs, starts the process in the project directory with `SPOOL_TASK_DIR` set, waits, and reads the output. The exit code is the status and standard error is the log. IDs are strings in these files. Rust starts a command and never knows which language it is. The judgements are stored under the checks a caller's judgements get through the API (section 4), so user code cannot name an unknown record or, outside a rematch, judge two records that both have an identity.

`candidates.parquet` holds the pairs the project's block rules found. `features.parquet` holds every record of the type, which is what a strategy that finds its own pairs needs: Splink blocks for itself, and a nearest-neighbour search has no rules at all. `is_new` marks the records the run is to place, and `identity_id` says where the others are. Both files are written for every task. A task that fails is kept, with its inputs and what the process printed, and the error names its directory. A task that succeeds is removed.

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

While the process runs the engine holds no lock on the store, so reads and other writes are served. One strategy or extractor runs at a time. A process has ten minutes by default (`--task-timeout`), after which it is stopped with everything it started, and its run fails. A run that fails for any reason changes nothing: the next run examines the same records.

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

A mention is a place in a document, an entity type and attributes. The engine makes each one a record of that type: its source is the document's, its key is the locator of section 1, its payload is the attributes, and its evidence is the document's ID, key and URI, the place, the text there, and the extractor with its version. From there it is a record like any other. The document's source has a file like any source, whose columns are the attributes a mention may have, and the type's mapping has a block for it. A mention with another attribute, or from a source the type does not map, is refused with the rest of what the extractor found. The next match run places it.

- **Places count Unicode code points.** That is what Python's string indices are. JavaScript's count UTF-16 units, so the TypeScript helper's `document.mention` converts, and the same mention written in either language is the same record, with the same ID. `document.find(quote)` gives the places of a quote, for an extractor that is told what a document mentions and not where.
- **The engine cuts the quoted text itself**, from the document and the place. An extractor cannot attribute to a document words that are not in it.
- **Attributes are flat**: text, numbers, and true or false. A number with no fractional part is stored as an integer, as for rows (section 5).
- **One bad mention refuses them all.** A mention of a type the project does not declare, a place outside the document, a document the task did not hand out, or the same place and type twice fails the run, and nothing is stored. So does a `start` or `end` column that does not hold whole numbers: a position is never rounded to a place the extractor did not name.
- **An extractor reads a document once.** A run reads the latest version of each document that this extractor has not read, whatever version the extractor had then. Reading again after the extractor changed is asked for with `again`: mentions it finds at the same place with the same attributes are duplicates, and with other attributes they supersede. A mention it no longer finds stays.
- **What an extractor found is stored under an ingest run of its own**, started when the mentions are stored, not under the extract run. A match run examines the records ingested after the last run and before itself. An extractor can work for minutes, and a match run that began meanwhile would otherwise pass over what it found, for good.

The engine has no extractor built in, and calls no model. `benchmarks/extraction/company-news/extractors/llm.py` is an example of one that asks a language model, through the chat-completions API that many services accept and with nothing beyond Python's standard library. It asks for quotes and finds their places itself, and drops a quote the text does not have. It is tested against a stand-in for the API, and has not been measured against a model.

There are four places an extension plugs in:

1. **Entity types.** Files in the project directory.
2. **Records.** Clients post them through the API. An extraction pipeline that runs in the caller is an upstream producer, and the server does not know it exists: it posts rows whose key is a locator of its own. Or the caller posts documents, and an extractor in the project reads them.
3. **Candidates.** The SQL files under `rules/block/`, or pairs a user-code strategy computes itself, such as nearest neighbours from an embedding index.
4. **Judgements.** SQL rules, a user-code strategy, or a client that pulls a run's candidates and submits judgements through the API. The last is how a review interface or an external system takes part.

Reconciliation policy is configuration in the first versions. Someone who needs custom logic can author a changeset directly and it will be validated like any other.

Consequences to accept:

- An image with user code carries `uv` or `bun` and the code's dependencies. A project of data files stays one binary.
- User code is the operator's own code. It runs with the service's permissions, and Spool does not sandbox it. The one thing withheld is a token of the API that the process was started with: `SPOOL_TOKEN`, which the commands that call a server read, is taken out of the environment.
- Files cost a write and a read per task. That suits batches and is wrong for per-record calls, which stay with SQL rules.
- Every task gets both the candidate pairs and the feature rows, though most strategies read one of them. Writing only what a strategy asks for needs the strategy to say so before it runs, and is deferred.
- A Python file named after a library it imports would be found in place of the library, because Python looks first in the directory of the file it runs. The engine turns that off (`PYTHONSAFEPATH`), which needs Python 3.11 or later, so `splink.py` can `import splink`.
- Only the server opens the store. A studio or any other tool goes through the API.

**Decide now:** child processes and no embedded runtime, Parquet files as the task protocol, tasks that produce evidence only, no code accepted through the API, workers inside the one service. **Defer:** more than one user-code process at a time, extraction in batches of documents so that a long run stores what it has found so far, separate worker containers that pull tasks over the same protocol, a streaming protocol, per-task resource limits beyond a timeout, a published package of the TypeScript helper's types, policy plug-ins.

## 7. JSON, Arrow, Parquet or DuckDB relations

| Use | Form | Reason |
| --- | --- | --- |
| Changesets, drafts, review items, reports, run status, single lookups | JSON objects over the API | Small, structured, and they need schemas and good error messages |
| Ingesting many records, pulling candidates, submitting judgements through the API | Arrow IPC streams | Columnar and streamable |
| The server to user code and back | Parquet files in a task directory | Any language reads them, and a failed task leaves its inputs behind to inspect |
| Blocking, comparison, aggregation, metrics | DuckDB SQL inside the engine | DuckDB already does it; string similarity functions are built in |
| The project's definition | Files in the project directory, compiled to a manifest | Reviewable in git, validated at build |
| Datasets at rest, benchmark fixtures, exports | Parquet | Portable, and `COPY ... TO` works in the stock library |

The rule of thumb for client authors: accept plain objects for convenience up to some thousands of rows, convert them once, and document the Arrow path for anything larger. Simple examples never mention Arrow or DuckDB.

Testing confirmed the internal half of this: an Arrow batch appended into a table, a blocking join with `jaro_winkler_similarity` read back as Arrow, and an in-memory Arrow batch joined against a stored table without first loading it into a table. That last one is how submitted judgements get validated against known records.

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

- `rules/block/` predicates generate the candidate pairs. Each is joined on its own, so a block rule should be one equality, or several joined by `AND`: the database then finds the pairs by hashing. A rule that says `OR` is compared for every pair of records, which took 40 seconds for 155,000 records where its two halves as two rules took 0.6 seconds and found the same pairs.
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

The benchmark runs the ordinary path (ingest, match, reconcile, commit) and scores the committed state against a truth relation `(record_id, true_entity)`. Nothing is special-cased, so a Rust, Python or TypeScript strategy is scored identically. `spool bench` uses the engine in-process, with no server, and starts user-code strategies the same way the server does.

A benchmark is a **scenario**: an ordered list of steps (ingest this batch, apply this correction, switch to this strategy) with truth at each step. A single-batch dataset is a one-step scenario. The specification is a data file, not code. A scenario names the strategy its steps resolve with, a step may name its own, and `spool bench --strategy` names one for a scenario that names none. A step with `rematch = true` matches everything again, which is how a scenario switches strategy on data it has already resolved.

A step ingests files, applies corrections, and resolves, in that order. A correction is what a reviewer would do, written against records named by source and key: merge the identities of two records, put one record in the identity of another, split records out of their identity, record a `must_link` or a `cannot_link`, or undo an earlier correction by name. Each is made through the call any caller would use. A step of corrections alone shows what the next run makes of them. Switching strategy within a scenario comes with the second strategy, in Milestone 4.

Resolution metrics, per step:

- Blocking: pair completeness and reduction ratio.
- Pairs: precision and recall, reported separately. F1 is shown but is not the headline, for the reasons in the [survey](landscape-survey.md#5-benchmark-datasets-and-evaluation-practice).
- Clusters: B-cubed precision and recall, the count of false merges (identities containing more than one true entity) and false splits (true entities spread over more than one identity), and the share of records left unassigned.

Identity metrics, across steps. These are the ones no public benchmark has:

| Metric | Definition |
| --- | --- |
| ID stability | Of the records present before and after a step, the share whose identity ID is unchanged. Reported strictly and after following redirects. |
| Churn | Identities created, merged and records reassigned per thousand new records. The counts compare the state before the step with the state after it. |
| Order sensitivity | Agreement between final partitions when the same batches arrive in different orders: the steps as written against the same steps reversed, both without corrections and without the rows a source took back. Reported as the share of records grouped with the same records either way, and as the pairs together in both orders over the pairs together in either. |
| Agreement with a rebuild | After a step that takes rows back or brings them back: the store against a second one that ran the same steps and was never sent the rows that are gone. Reported as the share of records grouped with the same records in both, and as the pairs together in one and not in the other. |
| Upgrade stability | After a step that matches everything again with a strategy: of the records that had an identity, the share whose identity ID is unchanged; the merges and splits the strategy suggested; and the changes to existing identities that were committed during the step. |
| Split recovery | After a scripted wrong merge and a manual split, whether later batches keep the two sides apart: of the splits the scenario made, the share with no identity holding records of both sides. |
| Override persistence | Share of constraints still honoured after further batches and a strategy change: of the constraints the scenario recorded, those still in force with state that agrees with them. |

The company fixture has a second scenario, `corrections.toml`, for these. Between the two batches a reviewer makes two wrong merges, undoes one, splits the other, merges a pair the rules missed, and at the end places the record that was left for review. With the default policy, split recovery and override persistence can only be complete: the policy never merges identities, and the validator rejects a changeset that breaks a constraint. They become gates when a strategy's suggestions are applied. Order sensitivity is already less than complete on this fixture. Reversing its two batches leaves 32 of 39 records in the same group, because a record that arrives before the one that would have connected two identities gets an identity of its own, and the policy does not merge afterwards.

Upgrade stability is complete with the default policy, as split recovery is: a rematch commits nothing about an identity that exists. What the metric shows is the other half, how much the new strategy would change. The person project has two scenarios for it. `upgrade.toml` resolves two batches of the committed sample with the rules and then has Splink judge every record again: all 16 records keep their identity, and Splink suggests two merges. One is right, a record the rules left on its own. The other would join two different people who share a surname, a city and most of an e-mail address. `fake-1000-upgrade.toml` does the same on `fake_1000`: 1,000 of 1,000 records keep their identity and 66 merges are suggested.

The voter project has a scenario whose sources change, `changing.toml`: five sources of about 100,000 records, 100,000 more, 50,000 taken back by two of the sources, everything matched again, and 10,000 of the rows sent again. The store is verified after every step. Taking the rows back changes the identity of no other record, and all but three of the 10,012 records that come back return to the identity they left; those three had none. Agreement with a rebuild is where a retraction shows: 549,799 of 549,978 records are grouped as a store that never held the rows groups them, and 142 pairs are together only because a record that is gone once joined them. A rematch does not report them. The retractions do: they leave 70 `possible_split` review items, and the pairs across the groups of those items are the 142 and no others, 31 of them wrong merges by the truth (section 4). The numbers are in [`benchmarks/README.md`](../benchmarks/README.md).

Resolved from the start with Splink, `fake_1000` has pair recall 0.76 where the rules have 0.72, both with no false merge. On the sample's second batch Splink matches one new record to two identities, which is the wrong merge above arriving as evidence: the record is left for review, and no identity is merged.

Extraction is scored on its own. A step can store a directory of documents and run extractors over them before it resolves, and the scenario's truth can be labelled mentions: a file that says which document mentions which true entity at which place. Two things are then reported:

- **Extraction**: the mentions found against the labelled ones, as precision, recall and F1. A mention is right when a label has the same document, entity type, start and end. Half a name is a wrong mention and a missed one.
- **Resolution**, with the metrics above, over the records that have a true entity. For a mention that is the label at the same place. A mention the extractor made up has no true entity and is left out, so a bad extractor lowers the first score and not the second.

Running the same documents through an extractor that reads the labels gives resolution with perfect extraction, which is the other half of telling the two apart. `benchmarks/extraction/company-news` does both. It is ten invented news items in two batches, with 25 labelled mentions of ten companies, and the cases that make each half hard: names without their legal form, a capitalised word before a name, a product called "Falcon Ltd edition", accents a wire service dropped, an emoji before a name, a short name, a similar name, and two unrelated companies with one name. Its rule-based extractor finds 21 mentions, 18 of them labelled (precision 0.86, recall 0.72). Resolution over those 18 has pair recall 1.0 and one false merge, the two companies with one name. With the labels as the extractor, resolution over all 25 has pair recall 0.86, one false merge and one false split: "Northwind" on its own does not match "Northwind Traders Ltd", a miss the rule-based extractor hides by not finding the short name at all. The rule-based extractor is written in Python and again in TypeScript, and the two produce the same records, ID for ID.

Datasets for day one: Splink's `fake_1000` as a smoke test, `historical_50k` and FEBRL as mid-sized checks, and a small hand-built company fixture committed to the repository that contains the hard cases of company resolution (aliases, legal versus trading names, domain changes, subsidiaries, records with few attributes), split into two batches. Each fixture is a project directory with its own entity type and rules. The three public datasets are person records and the fixture is company records, so the benchmark runs two unrelated entity types through one engine from the start. The public datasets are downloaded and checksummed at benchmark time, because their redistribution terms have not been confirmed.

Two more projects resolve public records at a size the others do not reach, both from the [Leipzig benchmark sets](https://dbs.uni-leipzig.de/research/projects/benchmark-datasets-for-entity-resolution), where the records are real and the duplicates are generated. `benchmarks/resolution/voters` is North Carolina voters in five sources of a million each, with a name, a suburb and a postcode to go on. `benchmarks/resolution/music` is MusicBrainz songs in five sources that each write a title, a length and a year their own way, so each source has a mapping of its own. An ingest can take part of a file with `where`, a SQL condition on the file's columns, which is how five sources each take the same half of the people, and can name a `member` of a dataset that is an archive. A scenario is checked against its project and `datasets.toml` before anything is downloaded.

Five sources of 500,000 voters resolve in 82 seconds on four cores with a peak of 5.9 GB, and five of a million in 210 seconds with 13.4 GB. Recall is the same at every size and precision is not: 0.988, 0.938 and 0.880 for 0.5, 2.5 and 5 million voter records, because the pairs of different people who look alike grow with the square of the people and one wrong pair joins two identities. Rules have to be measured at the size they will run at. `benchmarks/README.md` has the table. `packages/stress/src/replay.ts` runs a scenario in process and again through a server, and fails unless the two stores agree on who is with whom.

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
3. Ingest batch one from CSV: content-hashed records and their feature rows.
4. Run the built-in SQL strategy with the fixture's rules to produce candidates and judgements.
5. Reconcile into a draft changeset, validate it, commit it, and print the identities.
6. Ingest batch two, which contains exact re-sends, new records for known companies, and new companies. Repeat steps 4 and 5.
7. Report pair precision and recall, B-cubed, false merges and ID stability against the fixture's truth.

Exit criteria, each a test:

- The `fake_1000` person data runs through the same binary as the company fixture, with only the project directory differing.
- Re-sent rows create no records and no operations.
- Every identity ID from batch one still exists after batch two, with the same members plus any additions.
- A subsidiary with a different registry identifier is not attached to its parent.
- Two records that a rule says are different are not attached to the same identity, even when each of them matches it.
- A record matching two identities is left unassigned with a review item.
- A review item is still in the store after the store is reopened, and its record is examined again by the next run.
- A resolve that is cut short at any step is completed by the next one.
- Rebuilding the projection from the log reproduces the state exactly.
- The in-memory state and the DuckDB store pass the same conformance suite.
- Two runs with the same seed produce the same changeset content hashes.
- A changeset with a stale precondition is rejected and names the conflicting operation.

Milestone 2's exit criteria, each a test in `crates/spool-cli/tests` unless it says otherwise:

- With the same seed, the company fixture produces the same changeset content hashes through the library, through the server with Arrow bodies and with JSON bodies, and through the TypeScript client (`packages/client/test`).
- Every call under `/v1` is refused without a caller the server knows.
- `serve` sets up an empty store, starts again on a store its manifest was applied to, and refuses a store that was applied with another manifest until `spool apply` has run.
- A call that outlasts its wait answers `202` with the run, the run is read until it ends, and reads are answered while it is in progress.
- A run leaves its draft for review when asked. The draft is listed, read, approved or rejected, and approving one that went stale names the operations at fault.
- A draft that is rejected, replaced or never decided does not strand its records: the next run examines them again.
- A caller reads a run's candidates and submits judgements, as Arrow and as JSON, and reconciles the run.
- Identities, members, history, records and typed review items are read through the API.
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
- A dismissed review item stays closed, and its records are not examined again.
- Rebuilding the projection from the log reproduces the state after every kind of operation.
- The corrections scenario reports ID stability, churn, split recovery, override persistence and order sensitivity, and gives the same report for the same seed (`crates/spool-cli/tests`).
- Each of these is reached through the API and through the TypeScript client (`crates/spool-cli/tests`, `packages/client/test`).

Milestone 4 adds user code: `crates/spool/src/usercode.rs`, which runs a project's strategy files, the two helpers, `packages/strategy`, which the TypeScript helper is built from, and Splink as a strategy of the person project. Its exit criteria, each a test (those that start `uv` or `bun` run with `--include-ignored`):

- The company fixture's rules written as a Python strategy and as a TypeScript strategy commit the changesets the SQL rules commit, with the same content hashes.
- A strategy that fails, writes nothing, or does not finish in time fails its run, changes nothing, and leaves its task directory to be looked at. The next run examines the same records.
- Judgements from user code are checked as a caller's are: an unknown record, or a pair of records that both have an identity, is refused.
- Only the file the manifest was compiled from runs. A file that changed is refused until the project is built again, and a name the project does not have runs nothing.
- A strategy is started without a token of the server, and with paths that do not depend on where the engine was started.
- The engine answers reads and commits writes while a strategy runs.
- A strategy that finds its own pairs is given every record with its identity.
- A rematch moves no record that has an identity, and lists the merges and splits the strategy would make.
- Splink resolves the person sample in two batches with no false merge, and leaves for review the record it matches to two identities. The same records give the same changesets.
- Switching from the rules to Splink keeps every identity ID and reports upgrade stability, on the sample and on `fake_1000`.
- On `fake_1000` Splink has higher pair recall than the rules and no false merge.
- A project's strategies are listed by `info` and by the API, run through the server and through the TypeScript client, and a strategy that fails is reported as `strategy_failed` (`crates/spool-cli/tests`, `packages/client/test`).
- `build` writes a Dockerfile that carries the runtimes the project's strategies need, and `build --install` starts each strategy once.
- The TypeScript helper the engine carries is the one `packages/strategy` builds.

Milestone 5 adds documents and extractors: `crates/spool/src/documents.rs` and `crates/spool/src/engine/extract.rs`, the extractor half of the two helpers, and the news benchmark. The store's format version is 5. Its exit criteria, each a test (those that start `uv` or `bun` run with `--include-ignored`):

- A document is stored once, a changed text is its next version, and an extractor reads only the latest version, once.
- Mentions become records that say where they were found, and the next match run resolves them with the type's rules. Repeating an extraction creates nothing.
- The same extractor written in Python and in TypeScript produces the same record IDs, with text that holds accents and an emoji.
- An extractor that fails, or returns a mention the engine refuses, fails its run, stores nothing, and leaves its task directory to be looked at.
- A match run that begins while an extractor works does not hide what the extractor finds from the run after it.
- Each entity type's `extract.md` reaches the extractor, and editing it rebuilds no feature table.
- A scenario with labelled mentions reports extraction apart from resolution. The news benchmark gives the numbers of section 9 with its rule-based extractor, and with the labels as the extractor.
- A label that does not fit its document is refused and named.
- The example extractor that asks a language model places what the model quotes, and drops what the text does not have (against a stand-in for the model's API).
- Documents and extractions are reached through the API and the TypeScript client, a project's extractors are listed by `info` and by the API, and one that fails is reported as `extractor_failed` (`crates/spool-cli/tests`, `packages/client/test`).
- `dev` runs an extractor as it was last saved, and `build` writes a Dockerfile that carries what the project's extractors need.

Milestone 6 begins with inspection: `crates/spool/src/explain.rs` and the commands `show` and `explain`. The store's format does not change. Its exit criteria, each a test:

- A pair that a run matched is explained by that run, the rule that held and the changeset that placed them (`crates/spool/tests/explain.rs`).
- Two members of one identity that were never compared are explained by the chain of members between them.
- A pair kept apart is explained by the `no_match` rule or the `cannot_link` constraint that did it, and a reviewer's merge, split and removed constraint each appear with the actor and the reason.
- A record left for review is explained by its review item, open or dismissed, and a pair whose draft waits or was rejected says so.
- The rules as they are now are read apart from what a run recorded. A rule that reads a null feature is reported as undecided, and `no_match` and `match` rules that SQL refuses on the pair as rules that cannot be applied.
- An explanation is reached through the API, the TypeScript client and `spool explain`, and review items are found by a record they name.
- `spool show` lists and shows runs, identities with their members, constraints and history, records with their evidence and the place in their document, documents, review items, drafts and the log, as text and as the server's JSON (`crates/spool-cli/tests/show.rs`).
- A mistake in a command is a usage error, and a server that refuses or does not answer is reported in a sentence.

Milestone 6 continues with the studio: `packages/studio`, `crates/spool-cli/src/studio.rs` and the built files beside it. Its exit criteria, each a test:

- The server answers a browser with the studio and asks nothing for its files, while `/v1` still does. Every other path is still nothing, and `serve --no-studio` serves no studio (`crates/spool-cli/tests/studio.rs`).
- The studio is served with a policy that lets it load only itself and call only its server, and a browser is told when the files it has are still the ones.
- The files compiled into the binary are what `packages/studio` builds (run with `--include-ignored`, as are the tests of the screens).
- Each screen shows what the server holds, against a real server with the company and the news scenarios: the overview, runs, identities with their members, constraints and history, records, a document with its mentions in the text, review items, drafts, the log, and an explanation with the engine's sentences (`packages/studio/test`). An identity shows what its records say of each field and which sources say it, a review item shows its record beside the identities it matched with what was found and what was decided, and the box that finds things goes to what an ID or a `source:key` names and lists what holds the words typed.
- The first person makes their account in the studio from the server's link, signs out and signs in again, and an admin adds a person and turns their account off.
- Looking at the screens makes no call that changes anything.
- Each change is made on its screen and is then what the server holds: a draft approved and rejected, a review item's record placed in an identity it matched and in a new one, the identities an item matched merged into one, an item dismissed, a possible split answered by splitting off a group, by keeping the identity as one and by dismissing it, a record moved to another identity, to a new one and out of any, an identity split with and without keeping the sides apart, an identity merged into another, a constraint recorded and removed, and a changeset undone after the undo was shown (`packages/studio/test/changes.test.tsx`).
- A change the engine rejects changes nothing and is shown with the operations it names: a draft whose record was placed elsewhere, a move and a merge of something that changed after the screen was read, a merge from a review item of identities that changed since, a split that a `must_link` constraint forbids, and an undo of what was undone before.
- An identity that was looked up for a merge is not merged after the screen was read again, until it is looked up again and shows the member it gained meanwhile.
- A split of the larger side says that side stays, and it does. A split is refused when the identity gained a member after the screen was read, and is made after the screen is read again (also `a_split_based_on_a_log_position_is_refused_when_the_members_changed_after_it` in `crates/spool/tests/lifecycle.rs`, and over HTTP in `packages/client/test/client.test.ts`).
- A change is recorded under whoever is signed in.
- `bun run demo` fills only the server it started, and stops when another server already listens at its address.

Accounts and tokens are `crates/spool-cli/src/accounts.rs`, the `/auth`, `/v1/people` and `/v1/tokens` paths of the server, and `spool token` (`crates/spool-cli/src/commands/token.rs`). Their exit criteria, each a test in `crates/spool-cli/tests/accounts.rs`:

- The first account is made with the code of the link the server printed, and only once. A store that `spool dev` ran on still gets one.
- A change is recorded under the person whose session sent it, and under the name of the token that did. A request that names its author is refused.
- An admin adds people and changes them, and nobody else does: not another person, and not a token.
- An admin makes a token, which is shown once and writes under its own name, and revokes it, after which it is refused. Nobody else sees or manages tokens: not another person, not a token, and not a stranger.
- `spool token add` prints a token that works at once on a running server, `spool token revoke` ends it, and neither makes an accounts file beside a store that is not there.
- Signing out, a new password and an account turned off each end sessions, and the last admin stays one.
- A sign-in beyond the passwords the server checks at once is turned away, and signing in works again afterwards.
- A body too large to be about an account is refused unread.
- A write that says it comes from another site is refused.
- Someone an admin added by email signs in with Google and is at the page they opened, and their changes are theirs. Google's word makes no account: an email nobody was given, one Google has not verified, one Google does not answer for, and an account that is off are each refused, and so is another Google account that later has the address (`crates/spool-cli/tests/google.rs`, as are the next four, against a stand-in for Google).
- A sign-in finishes only in the browser that began it, once, and goes back only to one of the server's own pages.
- A token that Google did not sign, or made for another client, another sign-in or an earlier time, is refused.
- A sign-in beyond those the server asks Google about at once is turned away, and signing in works again afterwards.
- A person has a way to sign in: a password, or an email where the server signs people in with Google. A server given half of a client, or no address to come back to, does not start.
- Nobody signs in to a development server, and its changes are the `dev` account's. It asks for no token, refuses one that is made up, and takes one that was made. It does not start on an address others reach (`crates/spool-cli/tests/cli.rs`), and answers nothing that was sent to a name other than its machine's, where a server people sign in to answers by any name.

Beside the milestones, `packages/stress` puts a real server under load and abuse. Each part is a command for a run of any size, and runs in a small form with the tests (`crates/spool-cli/tests/stress.rs`, with `--include-ignored`). Every part ends with the server stopped and the store checked by `Engine::verify`, through the `verify` example of the `spool-cli` crate:

- **Scale** (`src/scale.ts`). Company records are generated with their truth: aliases, trading names, subsidiaries that share a name, typing mistakes, bare rows, and rows sent again. They are ingested and matched over HTTP in batches and matched again, each step is timed with the server's memory and the store's size, and the calls a reader makes are timed.
- **Crashes** (`src/crash.ts`). The same records are loaded into two servers. One is killed at a moment chosen at random within each step and started again. After each kill the store verifies, no run is left `running`, and the calls of the API agree with each other. In the end both stores say the same about who is who.
- **A random reviewer** (`src/reviewer.ts`). Records arrive and are matched while identities are merged, split and taken apart, constraints are set, changesets are undone, drafts are decided and the server is stopped or killed, in an order chosen at random, by one reviewer or by several at once. What is out of date or names nothing is refused, the server never answers with a failure of its own, and the calls of the API keep agreeing with each other.
- **Hostile requests** (`src/hostile.ts`). Each is one request nobody should send and the status the server answers it with.

Out of scope after this part of Milestone 6: a studio that follows a running server without being asked to read again, explaining what a user-code strategy would judge now, explaining a pair as of an earlier log position, LLM matching, an extractor built into the engine, documents that are not text, relationships between entities, canonical attribute views, applying a rematch's suggestions automatically.

Proposed roadmap:

1. **Engine.** The Rust library and command described above: a project directory with SQL features and rules, two batches, the changeset log, the benchmark.
2. **Server and client.** `spool serve` and the API, runs as tasks, `init`, `dev`, `build` and `apply`, the generated Dockerfile, the TypeScript client.
3. **Identity lifecycle.** Merge, split, constraints, undo and superseding records, plus the identity benchmark.
4. **User code.** The child-process protocol, Python first and then TypeScript, with Splink as the second strategy.
5. **Extraction.** Records from unstructured documents.
6. **Developer tooling.** Inspection and a studio, both through the API. Both are built: the studio shows what the store holds, and a reviewer approves, places, merges, splits, constrains, undoes and dismisses from it.

Milestones 2 and 3 do not depend on each other. Sandbox synchronisation, initial Milestone 6, is not in this roadmap.

## Decisions at a glance

| Decide now | Defer |
| --- | --- |
| Evidence versus identity-state split; `spool-core` without DuckDB or Arrow | A Postgres store; any abstraction over evidence storage |
| One server that owns the store; thin clients; one tenant, one writer; no native bindings | Richer authentication; a Python client; a published base image |
| A project directory with path-derived names, compiled to a manifest; state outside the image | Mountable packages of entity types; `review.md` |
| Entity types declared by the project; sources declared with their columns; feature tables generated from a mapping of source columns through SQL transforms; rules as files; no built-in types | Transforms in Python or TypeScript; a row that feeds more than one entity type; relationships between entities; client types generated from a manifest |
| User code as child processes over Parquet files; evidence only; workers inside the service; only files the manifest was compiled from run | More than one user-code process at a time; separate worker containers; a streaming protocol; per-task resource limits |
| Extractors as project-level user code; documents as text; a mention keyed by its document version, entity type and code-point span; mentions stored under their own ingest run; reading again only when asked | An extractor built into the engine; documents that are not text; retiring the mentions of a superseded document; extraction in batches |
| Judgements and draft changesets in place of proposals | Entity-centric judgements; calibrated probabilities |
| Review items kept in the store and examined again by later runs, until a changeset places their records or a reviewer dismisses them | Reopening a dismissed review item |
| Minted UUIDv7 identity IDs, permanent redirects, content-derived record IDs | Public ID formatting; binary IDs in Arrow |
| Changeset log as truth, projection in the same transaction, per-operation preconditions | Compaction, hash chaining |
| Operation set: create, assign, merge, constrain, unconstrain, reinstate; split and undo as changesets the engine writes | Constraints that choose between identities during a run; operations on more than two records |
| Default policy never merges or splits existing identities automatically; a rematch suggests and does not change | Policy plug-ins; automatic merges above a threshold; applying suggestions in bulk |
| JSON and Arrow as the only wire forms; the run ID as the task handle; drafts stored on the server | Streaming run progress; an ad hoc SQL endpoint |
| Inspection through the API only; an explanation computed from what the store holds, with sentences the engine writes | Running a strategy again to explain; explaining a pair as of an earlier log position |
| A studio served by the server as files compiled into the binary, built from `packages/studio` and committed; the files open to anyone and the API to callers the server knows; a policy that lets the page call only its server | Live updates; a studio published apart from the binary |
| Changes from the studio as single calls of the API, written against what the reviewer saw and recorded under a name they give; the engine alone decides whether a change holds | Sign-in and who may change what; a changeset the reviewer puts together from operations of their choice; starting runs, ingesting and extracting from the studio |
| Scenario-based benchmark with identity metrics and a CI baseline; extraction scored by exact span, apart from resolution | GLEIF scenario generator; a public extraction dataset; partial credit for overlapping spans; one end-to-end score |
| Two library crates and one for the command and server; injected ID generator and clock; format version in the file; the prebuilt DuckDB library found by run path | Further crate splits; a statically linked DuckDB; indexes and keys |
