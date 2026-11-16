-- A date of birth with one digit mistyped.
l.first_name = r.first_name
AND l.surname = r.surname
AND levenshtein(l.dob::VARCHAR, r.dob::VARCHAR) <= 1
