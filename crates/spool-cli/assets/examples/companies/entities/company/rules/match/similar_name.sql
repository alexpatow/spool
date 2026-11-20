-- A match rule says a pair is the same thing: here, two names that are
-- close. The similarity is 1 for equal names and 0 for names with nothing
-- in common.
jaro_winkler_similarity(l.name_key, r.name_key) >= 0.92
