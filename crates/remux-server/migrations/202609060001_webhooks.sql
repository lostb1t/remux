CREATE TABLE IF NOT EXISTS webhooks (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    destination TEXT NOT NULL DEFAULT '{"kind":"http","config":{"url":"","headers":{}}}',
    events TEXT NOT NULL DEFAULT '[]',
    user_ids TEXT NOT NULL DEFAULT '[]',
    media_types TEXT NOT NULL DEFAULT '[]',
    template TEXT NOT NULL DEFAULT '',
    fields TEXT NOT NULL DEFAULT '{}',
    send_all_properties INTEGER NOT NULL DEFAULT 0,
    trim_whitespace INTEGER NOT NULL DEFAULT 0,
    skip_empty_body INTEGER NOT NULL DEFAULT 0
);
