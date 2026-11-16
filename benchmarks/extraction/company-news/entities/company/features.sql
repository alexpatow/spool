-- One row per company observation. `observation` is the engine's table of
-- this type's observations. For a mention an extractor found, `payload`
-- holds the attributes the extractor gave, and `evidence` says where in
-- which document the mention is.
--
-- Lower case, no accents, `&` spelled out, punctuation dropped, and legal
-- forms removed from the end: "Müller & Söhne GmbH" becomes "muller and sohne".
SELECT
    observation_id,
    payload->>'name' AS name,
    nullif(regexp_replace(
        trim(regexp_replace(replace(lower(strip_accents(payload->>'name')), '&', ' and '), '[^a-z0-9]+', ' ', 'g')),
        '( (ltd|limited|plc|inc|corp|corporation|gmbh|ag|sa))+$', ''
    ), '') AS name_key
FROM observation
