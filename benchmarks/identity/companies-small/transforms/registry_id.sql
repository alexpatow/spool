-- A registry number means something only within its country: "GB-01234567".
lambda country, number: upper(country) || '-' || number
