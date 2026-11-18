-- Lower case, no accents, `&` spelled out, punctuation dropped, and legal
-- forms removed from the end: "Müller & Söhne GmbH" becomes "muller and sohne".
lambda name: nullif(regexp_replace(
    trim(regexp_replace(replace(lower(strip_accents(name)), '&', ' and '), '[^a-z0-9]+', ' ', 'g')),
    '( (ltd|limited|plc|inc|corp|corporation|gmbh|ag|sa))+$', ''
), '')
