-- The same suburb and postcode, which thousands of people share, so the
-- names have to begin alike.
l.suburb = r.suburb AND l.postcode = r.postcode
AND left(l.surname, 2) = left(r.surname, 2)
AND left(l.given, 1) = left(r.given, 1)
