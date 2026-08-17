CREATE TABLE webhooks (
    id                      TEXT PRIMARY KEY NOT NULL,
    name                    TEXT NOT NULL,
    enabled                 INTEGER NOT NULL DEFAULT 1,
    url                     TEXT NOT NULL,
    template                TEXT NOT NULL DEFAULT '',
    destination             TEXT NOT NULL,               -- JSON WebhookDestination (tagged "Type")
    notification_types      TEXT NOT NULL DEFAULT '[]',  -- JSON [NotificationType]
    user_filter             TEXT NOT NULL DEFAULT '[]',  -- JSON [uuid]
    item_types              TEXT NOT NULL,               -- JSON WebhookItemTypes
    send_all_properties     INTEGER NOT NULL DEFAULT 0,
    trim_whitespace         INTEGER NOT NULL DEFAULT 0,
    skip_empty_message_body INTEGER NOT NULL DEFAULT 0,
    created_at              TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at              TEXT NOT NULL DEFAULT (datetime('now'))
);
