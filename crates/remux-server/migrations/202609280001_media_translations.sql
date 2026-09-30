-- Title/description text and the Primary image in languages other than the
-- server's `preferred_metadata_language`, which stays on `media.title` /
-- `media.description` / `media_images`. `kind` copies `media.kind` for title
-- search.
-- `locked_fields` lists fields a provider refresh must not overwrite for
-- this language (JSON array of MetadataField names).
CREATE TABLE IF NOT EXISTS media_translations (
    media_id      TEXT     NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    language      TEXT     NOT NULL,
    kind          TEXT     NOT NULL,
    title         TEXT,
    description   TEXT,
    primary_image TEXT,
    source        TEXT     NOT NULL DEFAULT 'provider',
    locked_fields TEXT,
    updated_at    DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (media_id, language)
);

CREATE INDEX IF NOT EXISTS idx_media_translations_search
    ON media_translations(language, kind, title, media_id);
