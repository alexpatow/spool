-- "Enslavers - Natural Order" is the album "natural order".
lambda title: nullif(trim(regexp_replace(lower(strip_accents(regexp_extract(title, ' - ([^-]*)$', 1))), '[\p{P}\p{S}\s]+', ' ', 'g')), '')
