-- A no_match rule says a pair is not the same thing, and wins over any
-- match rule. This one holds only when both rows have a country.
l.country <> r.country
