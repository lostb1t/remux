-- Durable performance telemetry. Raw events are intentionally compact; tokens,
-- query strings, IPs, and full stream URLs are never stored.
CREATE TABLE IF NOT EXISTS telemetry_request_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    method TEXT NOT NULL,
    route_template TEXT NOT NULL,
    status INTEGER NOT NULL,
    latency_ms REAL NOT NULL,
    sample_reason TEXT NOT NULL,
    user_id TEXT,
    device_id TEXT,
    device_name TEXT,
    client_name TEXT,
    client_version TEXT,
    item_id TEXT,
    playback_key TEXT,
    error_category TEXT
);
CREATE INDEX IF NOT EXISTS idx_telemetry_request_time ON telemetry_request_events(created_at);
CREATE INDEX IF NOT EXISTS idx_telemetry_request_route_time ON telemetry_request_events(route_template, created_at);
CREATE INDEX IF NOT EXISTS idx_telemetry_request_device_time ON telemetry_request_events(device_id, created_at);
CREATE INDEX IF NOT EXISTS idx_telemetry_request_item_time ON telemetry_request_events(item_id, created_at);

CREATE TABLE IF NOT EXISTS telemetry_playback_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    playback_key TEXT NOT NULL,
    event TEXT NOT NULL,
    elapsed_ms REAL,
    item_id TEXT,
    item_name TEXT,
    series_name TEXT,
    source_id TEXT,
    source_name TEXT,
    delivery_class TEXT,
    error_category TEXT,
    user_id TEXT,
    device_id TEXT,
    device_name TEXT,
    client_name TEXT,
    client_version TEXT,
    details_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_telemetry_playback_time ON telemetry_playback_events(created_at);
CREATE INDEX IF NOT EXISTS idx_telemetry_playback_key ON telemetry_playback_events(playback_key, created_at);
CREATE INDEX IF NOT EXISTS idx_telemetry_playback_device ON telemetry_playback_events(device_id, created_at);

CREATE TABLE IF NOT EXISTS telemetry_saved_views (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    config_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
