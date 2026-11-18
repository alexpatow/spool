-- The title is mistyped or missing. The artist and the album are close, the
-- track number is the same and the length is within a second, and the year
-- is not different.
coalesce(jaro_winkler_similarity(l.artist, r.artist) >= 0.9, false)
AND coalesce(jaro_winkler_similarity(l.album, r.album) >= 0.9, false)
AND coalesce(l.number = r.number, false)
AND coalesce(abs(l.seconds - r.seconds) <= 1, false)
AND NOT coalesce(l.year <> r.year, false)
