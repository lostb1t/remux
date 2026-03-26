-- Separate EPG sources from channel sources
CREATE TABLE IF NOT EXISTS epg_sources (
    id               TEXT NOT NULL PRIMARY KEY,
    name             TEXT NOT NULL,
    url              TEXT NOT NULL,
    refresh_interval TEXT NOT NULL DEFAULT '24h'
);

-- Migrate any existing EPG URLs from iptv_sources into epg_sources
INSERT INTO epg_sources (id, name, url)
SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' || substr(hex(randomblob(2)),2) || '-' || substr('89ab',abs(random()) % 4 + 1, 1) || substr(hex(randomblob(2)),2) || '-' || hex(randomblob(6))),
       name || ' EPG',
       epg_url
FROM iptv_sources
WHERE epg_url IS NOT NULL AND epg_url != '';

-- Channel editor fields on media
ALTER TABLE media ADD COLUMN enabled     INTEGER NOT NULL DEFAULT 1;
ALTER TABLE media ADD COLUMN sort_order  INTEGER;
ALTER TABLE media ADD COLUMN custom_name TEXT;
