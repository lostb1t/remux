CREATE TABLE media_tags (
    media_id TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    tag      TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (media_id, tag COLLATE NOCASE)
);
CREATE INDEX idx_media_tags_tag ON media_tags(tag COLLATE NOCASE);
