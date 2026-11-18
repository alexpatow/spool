-- The surname may be mistyped after its first letter.
l.given = r.given AND l.postcode = r.postcode
AND left(l.surname, 1) = left(r.surname, 1)
