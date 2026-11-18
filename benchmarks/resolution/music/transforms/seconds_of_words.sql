-- "4m 21sec" is 261 seconds.
lambda length: try_cast(regexp_extract(length, '(\d+)\s*m', 1) AS INTEGER) * 60
  + try_cast(regexp_extract(length, '(\d+)\s*sec', 1) AS INTEGER)
