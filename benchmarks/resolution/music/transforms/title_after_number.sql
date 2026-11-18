-- "006-Enslavers" is the title "enslavers".
lambda title: nullif(trim(regexp_replace(lower(strip_accents(regexp_replace(title, '^\s*\d{1,3}\s*-?', ''))), '[\p{P}\p{S}\s]+', ' ', 'g')), '')
