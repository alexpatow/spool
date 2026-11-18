-- The title is close, and so is the artist or the album, which one source
-- in five does not have. The track number is the same and the length is
-- within a second, and the year is not different.
coalesce(
  l.title = r.title
  OR starts_with(l.title, r.title || ' ')
  OR starts_with(r.title, l.title || ' ')
  OR jaro_winkler_similarity(l.title, r.title) >= 0.92, false)
AND (coalesce(jaro_winkler_similarity(l.artist, r.artist) >= 0.9, false)
  OR coalesce(jaro_winkler_similarity(l.album, r.album) >= 0.9, false))
AND coalesce(l.number = r.number, false)
AND coalesce(abs(l.seconds - r.seconds) <= 1, false)
AND NOT coalesce(l.year <> r.year, false)
