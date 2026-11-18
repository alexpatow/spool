-- "Natural Order (1992)" is the album "natural order".
lambda album: nullif(trim(regexp_replace(lower(strip_accents(regexp_replace(album, '\s*\((\d+|unknown)\)\s*$', ''))), '[\p{P}\p{S}\s]+', ' ', 'g')), '')
