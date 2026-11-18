-- Four of the six values agree. A title agrees with one that is close to it
-- or that goes on after it, because a source that keeps the album in the
-- title does not always say where the title ends. An artist and an album
-- agree with one that is close, a length with one a second away, and a
-- track number and a year only with the same one.
  coalesce(
    l.title = r.title
    OR starts_with(l.title, r.title || ' ')
    OR starts_with(r.title, l.title || ' ')
    OR jaro_winkler_similarity(l.title, r.title) >= 0.92, false)::INT
  + coalesce(jaro_winkler_similarity(l.artist, r.artist) >= 0.9, false)::INT
  + coalesce(jaro_winkler_similarity(l.album, r.album) >= 0.9, false)::INT
  + coalesce(l.number = r.number, false)::INT
  + coalesce(abs(l.seconds - r.seconds) <= 1, false)::INT
  + coalesce(l.year = r.year, false)::INT >= 4
