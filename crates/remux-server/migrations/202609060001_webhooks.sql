CREATE TABLE IF NOT EXISTS webhooks (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    url TEXT NOT NULL,
    events TEXT NOT NULL DEFAULT '[]',
    user_ids TEXT NOT NULL DEFAULT '[]',
    media_types TEXT NOT NULL DEFAULT '[]',
    template TEXT NOT NULL DEFAULT '',
    headers TEXT NOT NULL DEFAULT '{}',
    fields TEXT NOT NULL DEFAULT '{}',
    send_all_properties INTEGER NOT NULL DEFAULT 0,
    trim_whitespace INTEGER NOT NULL DEFAULT 0,
    skip_empty_body INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id TEXT PRIMARY KEY NOT NULL,
    webhook_id TEXT NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
    event TEXT NOT NULL,
    attempt INTEGER NOT NULL DEFAULT 1,
    success INTEGER NOT NULL DEFAULT 0,
    status_code INTEGER,
    error TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_webhook
    ON webhook_deliveries(webhook_id, created_at DESC);
