//! The store's tables. Everything lives in the `spool` schema, so a user's own
//! tables can share the file.

use duckdb::Connection;

use crate::error::{Error, Result};

/// Written to `spool.meta` when a store is created and checked when one is
/// opened.
pub(crate) const FORMAT_VERSION: &str = "1";

const TABLES: &str = "
CREATE SCHEMA spool;

CREATE TABLE spool.meta (key VARCHAR NOT NULL, value VARCHAR NOT NULL);

-- Evidence

CREATE TABLE spool.observation (
  observation_id UUID        NOT NULL, -- first 128 bits of sha256(source_id, source_key, canonical payload)
  entity_type    VARCHAR     NOT NULL, -- a declared type
  source_id      VARCHAR     NOT NULL,
  source_key     VARCHAR,              -- key within the source: a row key, or an extracted mention's locator
  supersedes     UUID,                 -- previous version of the same (source_id, source_key)
  payload        JSON        NOT NULL, -- attributes exactly as received
  evidence       JSON,                 -- where in the source the observation was found
  observed_at    TIMESTAMPTZ,          -- when the source says it held, if known
  ingested_at    TIMESTAMPTZ NOT NULL,
  ingest_run     BIGINT      NOT NULL
);

CREATE TABLE spool.entity_type (       -- declared by the project; the engine ships none
  entity_type      VARCHAR     NOT NULL,
  version          INTEGER     NOT NULL,
  declaration      JSON        NOT NULL, -- the features query and the columns inferred from it
  declaration_hash VARCHAR     NOT NULL,
  declared_at      TIMESTAMPTZ NOT NULL
);

CREATE TABLE spool.run (
  run_id           BIGINT      NOT NULL,
  kind             VARCHAR     NOT NULL, -- 'ingest' | 'match' | 'reconcile'
  status           VARCHAR     NOT NULL, -- 'running' | 'succeeded' | 'failed'
  strategy         VARCHAR,
  strategy_version VARCHAR,
  config           JSON,
  config_hash      VARCHAR,
  engine_version   VARCHAR     NOT NULL,
  started_at       TIMESTAMPTZ NOT NULL,
  finished_at      TIMESTAMPTZ
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
  observation_id       UUID,
  other_observation_id UUID,
  entity_type          VARCHAR,
  constraint_kind      VARCHAR           -- 'must_link' | 'cannot_link'
);

CREATE TABLE spool.identity (          -- projection
  identity_id UUID    NOT NULL,
  entity_type VARCHAR NOT NULL,
  created_seq BIGINT  NOT NULL,
  merged_into UUID,                    -- always an active identity
  merged_seq  BIGINT
);

CREATE TABLE spool.membership (        -- projection with history; current rows have to_seq IS NULL
  observation_id UUID   NOT NULL,
  identity_id    UUID   NOT NULL,
  from_seq       BIGINT NOT NULL,
  to_seq         BIGINT
);

CREATE TABLE spool.constraint_link (   -- projection with history
  observation_a UUID    NOT NULL,
  observation_b UUID    NOT NULL,
  kind          VARCHAR NOT NULL,
  from_seq      BIGINT  NOT NULL,
  to_seq        BIGINT
);

-- The read surface. Base tables may change between format versions.

CREATE VIEW spool.identity_current AS
  SELECT identity_id, entity_type, created_seq
  FROM spool.identity
  WHERE merged_into IS NULL;

CREATE VIEW spool.membership_current AS
  SELECT observation_id, identity_id, from_seq
  FROM spool.membership
  WHERE to_seq IS NULL;
";

/// Creates the schema in an empty database, or checks the format version of
/// an existing one.
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
