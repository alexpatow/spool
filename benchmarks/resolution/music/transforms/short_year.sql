-- "1992", "92" and "'92" are the year "92": two sources have no century.
lambda year: nullif(right(regexp_replace(year, '\D', '', 'g'), 2), '')
