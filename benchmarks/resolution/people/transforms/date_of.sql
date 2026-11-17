-- Text that is not a date is no date.
lambda text: try_cast(text AS DATE)
