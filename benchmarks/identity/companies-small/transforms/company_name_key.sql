-- Lower case, no accents, `&` spelled out, punctuation dropped, and legal
-- forms removed from the end: "Acorn Tool Co." becomes "acorn tool".
lambda name: nullif(regexp_replace(
    trim(regexp_replace(replace(lower(strip_accents(name)), '&', ' and '), '[^a-z0-9]+', ' ', 'g')),
    '( (ltd|limited|plc|llp|llc|inc|co|company|corp|corporation))+$', ''
), '')
