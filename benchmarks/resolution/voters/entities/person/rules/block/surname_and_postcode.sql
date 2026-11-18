-- The given name may be mistyped after its first letter.
l.surname = r.surname AND l.postcode = r.postcode
AND left(l.given, 1) = left(r.given, 1)
