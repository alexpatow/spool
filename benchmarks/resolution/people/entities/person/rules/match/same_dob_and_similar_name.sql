l.dob = r.dob
AND (
    jaro_winkler_similarity(l.first_name, r.first_name) >= 0.9
    OR jaro_winkler_similarity(l.surname, r.surname) >= 0.9
)
