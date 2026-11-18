-- Two of the four values are the same and each of the other two is one edit
-- from its pair.
levenshtein(l.given, r.given) <= 1
AND levenshtein(l.surname, r.surname) <= 1
AND levenshtein(l.suburb, r.suburb) <= 1
AND levenshtein(l.postcode, r.postcode) <= 1
AND (l.given = r.given)::INT + (l.surname = r.surname)::INT
  + (l.suburb = r.suburb)::INT + (l.postcode = r.postcode)::INT = 2
