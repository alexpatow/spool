-- A transform is `lambda`, the names of its arguments, and a DuckDB
-- expression over them.
lambda name: lower(trim(name))
