-- "German" and "Ger." are the language "ger".
lambda language: nullif(nullif(left(lower(regexp_replace(language, '[^A-Za-z]', '', 'g')), 3), 'unk'), '')
