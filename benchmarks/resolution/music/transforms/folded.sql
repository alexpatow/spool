-- Lower case, no accents, and one space for each run of punctuation.
lambda text: nullif(trim(regexp_replace(lower(strip_accents(text)), '[\p{P}\p{S}\s]+', ' ', 'g')), '')
