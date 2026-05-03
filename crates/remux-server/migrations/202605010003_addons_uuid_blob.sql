-- Convert addons.id from TEXT (36-char string) to BLOB (16-byte UUID) to match
-- the format sqlx uses when binding Uuid values. The seed migrations inserted
-- UUIDs as plain SQL strings; this migration re-encodes them as blobs.

CREATE TABLE addons_new (
    id          BLOB PRIMARY KEY NOT NULL,
    kind        TEXT NOT NULL,
    name        TEXT NOT NULL,
    config      TEXT NOT NULL DEFAULT '{}',
    resources   TEXT NOT NULL DEFAULT '[]',
    types       TEXT NOT NULL DEFAULT '[]',
    enabled     INTEGER NOT NULL DEFAULT 1,
    priority    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

INSERT INTO addons_new SELECT
    -- TEXT uuid → BLOB; already-BLOB ids (inserted via sqlx) pass through unchanged.
    CASE typeof(id)
        WHEN 'text' THEN unhex(replace(id, '-', ''))
        ELSE id
    END,
    kind,
    name,
    config,
    resources,
    COALESCE(types, '[]'),
    COALESCE(enabled, 1),
    priority,
    created_at,
    updated_at
FROM addons;

DROP TABLE addons;
ALTER TABLE addons_new RENAME TO addons;

CREATE INDEX idx_addons_kind ON addons(kind);
