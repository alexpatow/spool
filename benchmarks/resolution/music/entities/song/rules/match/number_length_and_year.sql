-- The track number and the year are the same and the length is within a
-- second, and one of the title, the artist and the album is close.
coalesce(l.number = r.number, false)
AND coalesce(abs(l.seconds - r.seconds) <= 1, false)
AND coalesce(l.year = r.year, false)
AND (coalesce(
  l.title = r.title
  OR starts_with(l.title, r.title || ' ')
  OR starts_with(r.title, l.title || ' ')
  OR jaro_winkler_similarity(l.title, r.title) >= 0.92, false)
  OR coalesce(jaro_winkler_similarity(l.artist, r.artist) >= 0.9, false)
  OR coalesce(jaro_winkler_similarity(l.album, r.album) >= 0.9, false))
