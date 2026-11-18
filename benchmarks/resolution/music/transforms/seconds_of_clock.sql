-- "04:21" is 261 seconds.
lambda length: try_cast(split_part(length, ':', 1) AS INTEGER) * 60
  + try_cast(split_part(length, ':', 2) AS INTEGER)
