-- One row per person record, from rows with the columns first_name,
-- surname, dob, city and email.
SELECT
    record_id,
    lower(trim(payload->>'first_name')) AS first_name,
    lower(trim(payload->>'surname')) AS surname,
    try_cast(payload->>'dob' AS DATE) AS dob,
    lower(trim(payload->>'city')) AS city,
    lower(trim(payload->>'email')) AS email
FROM record
