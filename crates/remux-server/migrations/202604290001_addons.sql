CREATE TABLE addons (
    id          TEXT PRIMARY KEY NOT NULL,
    kind        TEXT NOT NULL,
    name        TEXT NOT NULL,
    config      TEXT NOT NULL DEFAULT '{}',
    resources   TEXT NOT NULL DEFAULT '[]',
    priority    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX idx_addons_kind ON addons(kind);
