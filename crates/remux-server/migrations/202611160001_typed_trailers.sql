-- Trailers were stored as bare YouTube ids; they are now typed
-- (`{"source": {"youtube": id}}` or `{"source": {"url": url}}`, with an
-- optional name) so direct videos can be told apart.
UPDATE media
SET trailers = (
    SELECT json_group_array(json_object('source', json_object('youtube', value)))
    FROM json_each(media.trailers)
    WHERE type = 'text' AND value <> ''
)
WHERE json_valid(trailers)
  AND json_type(trailers) = 'array'
  AND EXISTS (SELECT 1 FROM json_each(media.trailers) WHERE type = 'text');
