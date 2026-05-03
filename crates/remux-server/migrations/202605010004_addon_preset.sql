-- Merge kind+config into a single preset JSON column, drop kind column.
CREATE TABLE addons_new (
    id          BLOB NOT NULL PRIMARY KEY,
    name        TEXT NOT NULL,
    preset      TEXT NOT NULL DEFAULT '{"kind":"","config":{}}',
    resources   TEXT NOT NULL DEFAULT '[]',
    types       TEXT NOT NULL DEFAULT '[]',
    enabled     INTEGER NOT NULL DEFAULT 1,
    priority    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

INSERT INTO addons_new SELECT
    id,
    name,
    json_object('kind', kind, 'config', json(config)),
    resources,
    COALESCE(types, '[]'),
    COALESCE(enabled, 1),
    priority,
    created_at,
    updated_at
FROM addons;

DROP TABLE addons;
ALTER TABLE addons_new RENAME TO addons;

CREATE INDEX idx_addons_preset_kind ON addons(json_extract(preset, '$.kind'));
