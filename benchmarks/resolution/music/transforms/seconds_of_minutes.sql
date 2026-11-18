-- "4.351" minutes is 261 seconds.
lambda length: try_cast(floor(try_cast(length AS DOUBLE) * 60 + 0.001) AS INTEGER)
