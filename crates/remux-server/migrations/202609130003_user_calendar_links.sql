-- A user's subscribable ICS calendar feed.
--
-- The feed URL carries the only credential a calendar client can present, so
-- the token is stored as its SHA-256 digest: a database leak must not yield
-- working feed URLs. The digest is the primary key because feed requests look
-- the row up by digest alone.
--
-- One row per user (user_id UNIQUE): rotating replaces the row's token rather
-- than accumulating live URLs, so a rotation immediately kills the old link.
CREATE TABLE user_calendar_links (
    token_hash BLOB PRIMARY KEY NOT NULL,
    user_id    TEXT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    created_at DATETIME NOT NULL,
    rotated_at DATETIME
);
