-- Format 11 kept counts for each reading and evidence on the records it
-- inserted. Per-reading mention links begin with format 12 extractions.
ALTER TABLE spool.extraction ADD COLUMN failure VARCHAR;

CREATE TABLE spool.extraction_found (
  run_id            BIGINT  NOT NULL,  -- the extract run
  document_id       UUID    NOT NULL,
  extractor         VARCHAR NOT NULL,
  extractor_version VARCHAR NOT NULL,
  entity_type       VARCHAR NOT NULL,
  record_key        VARCHAR NOT NULL,  -- the mention's locator, which is its record's key
  record_id         UUID    NOT NULL   -- the version of the record it found
);
