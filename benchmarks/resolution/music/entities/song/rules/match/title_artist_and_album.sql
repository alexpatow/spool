-- The title, the artist and the album are each close to the other side's.
-- A title is also close to one that goes on after it, because a source that
-- keeps the album in the title does not always say where the title ends.
--
-- That is not enough: an album comes out more than once, and its songs are
-- then different songs here. So the track number, the length or the year is
-- the same, and neither the track number nor the year is different.
coalesce(
  l.title = r.title
  OR starts_with(l.title, r.title || ' ')
  OR starts_with(r.title, l.title || ' ')
  OR jaro_winkler_similarity(l.title, r.title) >= 0.92, false)
AND coalesce(jaro_winkler_similarity(l.artist, r.artist) >= 0.9, false)
AND coalesce(jaro_winkler_similarity(l.album, r.album) >= 0.9, false)
AND (coalesce(l.number = r.number, false)
  OR coalesce(abs(l.seconds - r.seconds) <= 1, false)
  OR coalesce(l.year = r.year, false))
AND NOT coalesce(l.number <> r.number, false)
AND NOT coalesce(l.year <> r.year, false)
