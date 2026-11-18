-- The same surname at the same place under another given name is a relative.
l.surname = r.surname AND l.suburb = r.suburb AND l.postcode = r.postcode
AND l.given <> r.given
