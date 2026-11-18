-- Lower case, no accents, `&` spelled out, punctuation dropped, and no "the"
-- at the start: "The Müller & Söhne Trust" becomes "muller and sohne trust".
lambda name: nullif(regexp_replace(
    trim(regexp_replace(replace(lower(strip_accents(name)), '&', ' and '), '[^\p{L}\p{N}]+', ' ', 'g')),
    '^the ', ''
), '')
