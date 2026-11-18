-- A name of two words or more is taken to be one person wherever it is
-- written. A name of one word is not: "Lee" in two articles is two people
-- more often than one.
l.name_key = r.name_key AND contains(l.name_key, ' ')
