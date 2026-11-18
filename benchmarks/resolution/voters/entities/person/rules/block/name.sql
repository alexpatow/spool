-- The same name. A third of the people with errors have them in the suburb
-- and the postcode, so neither can be asked for whole. The suburb's first
-- letter keeps people called "james smith" from being compared across the
-- whole state.
l.given = r.given AND l.surname = r.surname
AND left(l.suburb, 1) = left(r.suburb, 1)
