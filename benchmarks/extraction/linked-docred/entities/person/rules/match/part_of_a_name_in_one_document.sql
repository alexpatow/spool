-- Within a document, a name that is the first or the last words of a longer
-- one is the same person: "Reyes" after "Dana Reyes". It is wrong for two
-- people of one family in one article.
l.document = r.document
AND (
    starts_with(l.name_key, r.name_key || ' ') OR ends_with(l.name_key, ' ' || r.name_key)
    OR starts_with(r.name_key, l.name_key || ' ') OR ends_with(r.name_key, ' ' || l.name_key)
)
