-- A block rule says which pairs are worth comparing: here, two names that
-- begin with the same four characters. `l` and `r` are two rows of the
-- feature table.
left(l.name_key, 4) = left(r.name_key, 4)
