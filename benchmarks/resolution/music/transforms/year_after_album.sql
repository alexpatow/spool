-- "Natural Order (1992)" is the year "92".
lambda album: nullif(right(regexp_extract(album, '\((\d{4})\)\s*$', 1), 2), '')
