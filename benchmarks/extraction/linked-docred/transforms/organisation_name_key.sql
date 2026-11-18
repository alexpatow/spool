-- As `name_key`, and without the legal forms at the end: "Müller & Söhne
-- GmbH" becomes "muller and sohne". A transform cannot call another, so the
-- first part is written out again.
lambda name: nullif(regexp_replace(regexp_replace(
    trim(regexp_replace(replace(lower(strip_accents(name)), '&', ' and '), '[^\p{L}\p{N}]+', ' ', 'g')),
    '^the ', ''),
    '( (ltd|limited|plc|inc|incorporated|corp|corporation|co|company|llc|gmbh|ag|sa|nv|ab))+$', ''
), '')
