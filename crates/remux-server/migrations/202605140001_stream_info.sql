-- Merge `url` (StreamDescriptor) and `provider_info` (StreamProviderInfo) into a
-- single `stream_info` JSON column that stores both transport and provider metadata.
--
-- The Http variant changes shape: old {"Http":"url"} → new {"Http":{"url":"...","request_headers":{},"response_headers":{}}}

ALTER TABLE media ADD COLUMN stream_info TEXT;

UPDATE media
SET stream_info = json_object(
    'descriptor', CASE
        -- Old Http(String) format → new Http { url, request_headers, response_headers }
        WHEN json_type(url, '$.Http') = 'text' THEN
            json_object('Http', json_object(
                'url', json_extract(url, '$.Http'),
                'request_headers', COALESCE(json_extract(provider_info, '$.aio.request_headers'), json_object()),
                'response_headers', COALESCE(json_extract(provider_info, '$.aio.response_headers'), json_object())
            ))
        -- Torrent, Local, Opendal: keep descriptor as-is
        ELSE json(url)
    END,
    'filename',    json_extract(provider_info, '$.aio.filename'),
    'name',        json_extract(provider_info, '$.aio.name'),
    'description', json_extract(provider_info, '$.aio.description'),
    'seeders',     json_extract(provider_info, '$.aio.seeders'),
    'size',        json_extract(provider_info, '$.aio.size'),
    'duration',    json_extract(provider_info, '$.aio.duration'),
    'subtitles',   COALESCE(json_extract(provider_info, '$.aio.subtitles'), json_array())
)
WHERE url IS NOT NULL;

ALTER TABLE media DROP COLUMN url;
ALTER TABLE media DROP COLUMN provider_info;
