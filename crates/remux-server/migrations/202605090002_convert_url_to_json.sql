-- Convert any remaining plain-string URL values to the JSON StreamDescriptor format.
-- These are non-stream rows (tv_channel, track, album) that had HTTP URLs stored
-- before the switch from URL strings to JSON.
UPDATE media
SET url = '{"Http":' || json_quote(url) || '}'
WHERE url IS NOT NULL
  AND url NOT LIKE '{%'
  AND (url LIKE 'http://%' OR url LIKE 'https://%');

-- NULL out empty strings and anything else we can't auto-convert.
-- Rows will be repopulated by their respective addon on next refresh.
UPDATE media
SET url = NULL
WHERE url IS NOT NULL
  AND (url = '' OR url NOT LIKE '{%');
