//! The store's tables. Everything lives in the `spool` schema, so a user's own
//! tables can share the file.

use duckdb::Connection;

use crate::error::{Error, Result};

/// Written to `spool.meta` when a store is created and checked when one is
/// opened.
pub(crate) const FORMAT_VERSION: &str = "12";

const TABLES: &str = "
CREATE SCHEMA spool;

CREATE TABLE spool.meta (key VARCHAR NOT NULL, value VARCHAR NOT NULL);

-- Evidence

CREATE TABLE spool.record (
  record_id      UUID        NOT NULL, -- first 128 bits of sha256(source_id, source_key, canonical payload); of that and `supersedes` for a version that holds a payload its key held before
  entity_type    VARCHAR     NOT NULL, -- a declared type
  source_id      VARCHAR     NOT NULL,
  source_key     VARCHAR,              -- key within the source: a row key, or an extracted mention's locator
  supersedes     UUID,                 -- previous version of the same (entity_type, source_id, source_key)
  payload        JSON        NOT NULL, -- attributes exactly as received
  evidence       JSON,                 -- where in the source the record was found
  observed_at    TIMESTAMPTZ,          -- when the source says it held, if known
  loaded_at      TIMESTAMPTZ NOT NULL,
  load_run       BIGINT      NOT NULL
);

CREATE TABLE spool.retraction (        -- rows a source took back; kept. One is in force while restored_run IS NULL
  record_id    UUID   NOT NULL,        -- a record the source no longer holds: it stays as evidence and has no identity
  retract_run  BIGINT NOT NULL,        -- the retract run that took it back
  restored_run BIGINT,                 -- the load run in which the source sent the row again
  detached_seq BIGINT                  -- the log position of the changeset that took it out of its identity, if it had one
);

CREATE TABLE spool.document (          -- text a source holds, kept so that what a mention was found in can be read
  document_id UUID        NOT NULL,    -- first 128 bits of sha256(source_id, source_key, text); of that and `supersedes` for a version that holds a text its key held before
  source_id   VARCHAR     NOT NULL,
  source_key  VARCHAR     NOT NULL,    -- the document's key within the source
  supersedes  UUID,                    -- previous version of the same (source_id, source_key)
  uri         VARCHAR,                 -- where the source keeps it, if it said
  text        VARCHAR     NOT NULL,
  loaded_at   TIMESTAMPTZ NOT NULL,
  load_run    BIGINT      NOT NULL
);

CREATE TABLE spool.extraction (        -- one row each time an extractor read a document, or could not
  document_id       UUID    NOT NULL,
  extractor         VARCHAR NOT NULL,
  extractor_version VARCHAR NOT NULL,
  run_id            BIGINT  NOT NULL,  -- the extract run
  mentions          BIGINT  NOT NULL,  -- how many it found in the document
  failure           VARCHAR            -- why it could not read the document; the document is then not read
);

CREATE TABLE spool.extraction_found (  -- one row for each mention a reading found, new or found again
  run_id            BIGINT  NOT NULL,  -- the extract run
  document_id       UUID    NOT NULL,
  extractor         VARCHAR NOT NULL,
  extractor_version VARCHAR NOT NULL,
  entity_type       VARCHAR NOT NULL,
  record_key        VARCHAR NOT NULL,  -- the mention's locator, which is its record's key
  record_id         UUID    NOT NULL   -- the version of the record it found
);

CREATE TABLE spool.entity_type (       -- declared by the project; the engine ships none
  entity_type      VARCHAR     NOT NULL,
  version          INTEGER     NOT NULL,
  declaration      JSON        NOT NULL, -- the features, the label, and the query and transforms compiled from entity.yaml
  declaration_hash VARCHAR     NOT NULL,
  declared_at      TIMESTAMPTZ NOT NULL
);

CREATE TABLE spool.run (               -- also the task record a client reads
  run_id           BIGINT      NOT NULL,
  kind             VARCHAR     NOT NULL, -- 'load' | 'extract' | 'match' | 'reconcile' | 'retract'
  status           VARCHAR     NOT NULL, -- 'running' | 'open' | 'drafted' | 'succeeded' | 'failed'
  strategy         VARCHAR,
  strategy_version VARCHAR,
  config           JSON,
  config_hash      VARCHAR,
  engine_version   VARCHAR     NOT NULL,
  started_at       TIMESTAMPTZ NOT NULL,
  finished_at      TIMESTAMPTZ,
  error            VARCHAR,              -- failed: why
  result           JSON,                 -- what the run reported: a load's, an extraction's or a retraction's counts, a match run's resolution
  started_by       VARCHAR     NOT NULL  -- who asked for the run, as an actor; 'engine' when nobody did
);

CREATE TABLE spool.candidate (         -- per run, disposable
  run_id   BIGINT NOT NULL,
  left_id  UUID   NOT NULL,
  right_id UUID   NOT NULL,            -- left_id < right_id
  rule     VARCHAR
);

CREATE TABLE spool.judgement (         -- kept
  run_id   BIGINT  NOT NULL,
  left_id  UUID    NOT NULL,
  right_id UUID    NOT NULL,           -- left_id < right_id
  verdict  VARCHAR NOT NULL,           -- 'match' | 'no_match' | 'unsure'
  score    DOUBLE,                     -- strategy-local scale, never compared across strategies
  detail   JSON
);

CREATE TABLE spool.review_item (       -- what the engine would not decide; kept
  review_id       BIGINT  NOT NULL,    -- dense, assigned by the engine
  run_id          BIGINT  NOT NULL,    -- the run that raised it: a reconcile run, or for a possible_split a retract run
  entity_type     VARCHAR NOT NULL,
  kind            VARCHAR NOT NULL,    -- 'possible_merge' | 'conflict' | 'possible_split'
  record_ids UUID[]  NOT NULL,    -- the records left without an identity; possible_split: the members nothing holds together
  identity_ids    UUID[]  NOT NULL,    -- the identities they matched; empty when the conflict is among the records; possible_split: the identity they are in
  status          VARCHAR NOT NULL,    -- 'open' | 'resolved' | 'superseded' | 'dismissed'
  closed_seq      BIGINT,              -- resolved: the changeset after which every record had an identity; possible_split: after which nothing was left to ask
  closed_run      BIGINT,              -- superseded: the run that raised it again in another form, the retract run that took back what it asked about, or the reconcile run whose judgements answered it
  closed_by       VARCHAR,             -- dismissed: who decided, as an actor
  closed_at       TIMESTAMPTZ,         -- dismissed: when
  closed_reason   VARCHAR,             -- dismissed: why, if the reviewer said
  groups          UUID[][]             -- possible_split: record_ids as the groups that are each still held together; NULL otherwise
);

CREATE TABLE spool.draft (             -- a changeset a reconcile run drafted and left for review; kept
  changeset_id  UUID    NOT NULL,      -- the draft's handle, and the ID it is committed under
  run_id        BIGINT  NOT NULL,      -- reconcile run that drafted it
  match_run     BIGINT  NOT NULL,      -- match run whose judgements it rests on
  entity_type   VARCHAR NOT NULL,
  base_seq      BIGINT  NOT NULL,      -- log position it was drafted against
  actor         VARCHAR NOT NULL,
  reason        VARCHAR,
  status        VARCHAR NOT NULL,      -- 'open' | 'committed' | 'rejected' | 'superseded'
  closed_seq    BIGINT,                -- committed: its log position
  closed_run    BIGINT,                -- superseded: the reconcile run that drafted the type again, or the retract run that took back a record it places
  closed_at     TIMESTAMPTZ,           -- rejected: when
  closed_reason VARCHAR                -- rejected: why, if the reviewer said
);

CREATE TABLE spool.draft_op (          -- a draft's operations, in the shape of spool.changeset_op
  changeset_id         UUID    NOT NULL,
  op_index             INTEGER NOT NULL,
  kind                 VARCHAR NOT NULL,
  identity_id          UUID,
  from_identity_id     UUID,
  record_id            UUID,
  other_record_id      UUID,
  entity_type          VARCHAR,
  constraint_kind      VARCHAR
);

-- Identity state

CREATE TABLE spool.changeset (         -- the log; source of truth
  seq          BIGINT      NOT NULL,   -- dense, assigned at commit; this store's total order
  changeset_id UUID        NOT NULL,   -- minted by the author
  base_seq     BIGINT      NOT NULL,   -- log position the author had seen
  actor        VARCHAR     NOT NULL,   -- 'engine' | 'user:<id>' | 'agent:<id>'
  reason       VARCHAR,
  run_id       BIGINT,                 -- reconcile run that drafted it, if any
  content_hash VARCHAR     NOT NULL,   -- hash of the canonical encoding of its operations
  committed_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE spool.changeset_op (      -- one row per operation, so apply is set-based
  seq                  BIGINT  NOT NULL,
  op_index             INTEGER NOT NULL,
  kind                 VARCHAR NOT NULL, -- 'create_identity' | 'assign' | 'merge' | 'constrain' | 'unconstrain' | 'reinstate'
  identity_id          UUID,             -- created identity / assign target / merge survivor
  from_identity_id     UUID,             -- assign source / merged-away identity
  record_id            UUID,
  other_record_id      UUID,
  entity_type          VARCHAR,
  constraint_kind      VARCHAR           -- 'must_link' | 'cannot_link'
);

CREATE TABLE spool.identity (          -- projection
  identity_id UUID    NOT NULL,
  entity_type VARCHAR NOT NULL,
  created_seq BIGINT  NOT NULL,
  merged_into  UUID,                   -- the active identity that stands for it; NULL while it is active
  merged_seq   BIGINT,                 -- the merge that made it inactive
  merge_target UUID                    -- the identity that merge named; not merged_into once that one is merged in turn
);

CREATE TABLE spool.membership (        -- projection with history; current rows have to_seq IS NULL
  record_id      UUID   NOT NULL,
  identity_id    UUID   NOT NULL,
  from_seq       BIGINT NOT NULL,
  to_seq         BIGINT
);

CREATE TABLE spool.constraint_link (   -- projection with history; in force while to_seq IS NULL
  record_a      UUID    NOT NULL,      -- record_a < record_b
  record_b      UUID    NOT NULL,
  kind          VARCHAR NOT NULL,
  from_seq      BIGINT  NOT NULL,
  to_seq        BIGINT
);

-- The read surface. Base tables may change between format versions.

CREATE VIEW spool.identity_current AS
  SELECT identity_id, entity_type, created_seq
  FROM spool.identity
  WHERE merged_into IS NULL;

-- What to call each record. Replaced when a project is applied.
CREATE VIEW spool.record_label AS
  SELECT NULL::UUID AS record_id, NULL::VARCHAR AS label WHERE false;

CREATE VIEW spool.membership_current AS
  SELECT record_id, identity_id, from_seq
  FROM spool.membership
  WHERE to_seq IS NULL;

CREATE VIEW spool.constraint_current AS
  SELECT record_a, record_b, kind, from_seq
  FROM spool.constraint_link
  WHERE to_seq IS NULL;

CREATE VIEW spool.review_open AS
  SELECT review_id, run_id, entity_type, kind, record_ids, identity_ids
  FROM spool.review_item
  WHERE status = 'open';

CREATE VIEW spool.draft_open AS
  SELECT changeset_id, run_id, match_run, entity_type, base_seq
  FROM spool.draft
  WHERE status = 'open';
";

/// Creates the schema in an empty database, or checks the format version of
/// an existing one. A store of another format is refused and left as it is.
/// Nothing brings an earlier one up to date: format 10 renamed columns that
/// were there, and format 11 says who started each run, which no earlier
/// store knows.
pub(crate) fn ensure(conn: &mut Connection) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT count(*) > 0 FROM information_schema.schemata WHERE schema_name = 'spool'",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        let tx = conn.transaction()?;
        tx.execute_batch(TABLES)?;
        tx.execute(
            "INSERT INTO spool.meta VALUES ('format_version', ?)",
            [FORMAT_VERSION],
        )?;
        tx.commit()?;
        return Ok(());
    }
    let found = meta(conn, "format_version")?.unwrap_or_else(|| "none".to_owned());
    if found != FORMAT_VERSION {
        return Err(Error::FormatVersion {
            found,
            expected: FORMAT_VERSION.to_owned(),
        });
    }
    Ok(())
}

pub(crate) fn meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut statement = conn.prepare("SELECT value FROM spool.meta WHERE key = ?")?;
    let mut rows = statement.query([key])?;
    Ok(match rows.next()? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

pub(crate) fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute("DELETE FROM spool.meta WHERE key = ?", [key])?;
    conn.execute("INSERT INTO spool.meta VALUES (?, ?)", [key, value])?;
    Ok(())
}
