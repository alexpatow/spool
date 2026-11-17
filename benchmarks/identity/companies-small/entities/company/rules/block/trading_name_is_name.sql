-- One company's trading name is the other's name. The pair the other way
-- round is `name_is_trading_name`: a block rule is one equality, because a
-- rule that says `OR` compares every record with every other.
l.trading_key = r.name_key
