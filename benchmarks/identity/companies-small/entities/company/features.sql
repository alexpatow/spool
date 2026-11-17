-- One row per company record. `record` is the engine's table of
-- this type's records; `payload` holds the row as it arrived.
--
-- The CRM sends name, website, country, city and sometimes registry_number.
-- The registry sends number, country, legal_name, trading_name and city.
WITH named AS (
    SELECT
        record_id,
        payload,
        [coalesce(payload->>'name', payload->>'legal_name'), payload->>'trading_name'] AS names
    FROM record
),
keyed AS (
    SELECT
        record_id,
        payload,
        -- Lower case, no accents, `&` spelled out, punctuation dropped, and
        -- legal forms removed from the end: "Acorn Tool Co." becomes "acorn tool".
        list_transform(names, lambda name: nullif(regexp_replace(
            trim(regexp_replace(replace(lower(strip_accents(name)), '&', ' and '), '[^a-z0-9]+', ' ', 'g')),
            '( (ltd|limited|plc|llp|llc|inc|co|company|corp|corporation))+$', ''
        ), '')) AS keys
    FROM named
)
SELECT
    record_id,
    keys[1] AS name_key,
    keys[2] AS trading_key,
    nullif(regexp_extract(lower(payload->>'website'), '^(?:https?://)?(?:www\.)?([^/:?#]+)', 1), '') AS domain,
    upper(payload->>'country') AS country,
    upper(payload->>'country') || '-' || coalesce(payload->>'registry_number', payload->>'number') AS registry_id
FROM keyed
