-- "https://www.acorn.example/about" becomes "acorn.example".
lambda website: nullif(regexp_extract(lower(website), '^(?:https?://)?(?:www\.)?([^/:?#]+)', 1), '')
