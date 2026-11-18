-- "Enslavers (Natural Order)" is the title "enslavers".
lambda title: nullif(trim(regexp_replace(lower(strip_accents(regexp_replace(title, '\s*\([^()]*\)\s*$', ''))), '[\p{P}\p{S}\s]+', ' ', 'g')), '')
