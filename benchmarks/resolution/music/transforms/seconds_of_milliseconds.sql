-- "261066" milliseconds is 261 seconds.
lambda length: try_cast(floor(try_cast(length AS DOUBLE) / 1000) AS INTEGER)
