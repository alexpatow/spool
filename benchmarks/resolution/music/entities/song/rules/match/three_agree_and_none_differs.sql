-- Three of the six values agree, and where both sides have a track number,
-- a length or a year, they are the same. Half of the songs are in a source
-- with a value missing.
  coalesce(
    l.title = r.title
    OR starts_with(l.title, r.title || ' ')
    OR starts_with(r.title, l.title || ' ')
    OR jaro_winkler_similarity(l.title, r.title) >= 0.92, false)::INT
  + coalesce(jaro_winkler_similarity(l.artist, r.artist) >= 0.9, false)::INT
  + coalesce(jaro_winkler_similarity(l.album, r.album) >= 0.9, false)::INT
  + coalesce(l.number = r.number, false)::INT
  + coalesce(abs(l.seconds - r.seconds) <= 1, false)::INT
  + coalesce(l.year = r.year, false)::INT >= 3
AND NOT coalesce(l.number <> r.number, false)
AND NOT coalesce(abs(l.seconds - r.seconds) > 5, false)
AND NOT coalesce(l.year <> r.year, false)
