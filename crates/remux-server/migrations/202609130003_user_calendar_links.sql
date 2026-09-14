-- A user's subscribable ICS calendar feed.
--
-- The token is stored in plaintext because only an administrator creates and
-- distributes these links, so the admin UI must be able to display an existing
-- one instead of rotating it. This matches `api_keys`, which stores its
-- `access_token` the same way for the same reason.
--
-- Consequence: this table is credential material. A leak yields working feed
-- URLs, which grant read access to a user's calendar — nothing else, and each
-- link is revocable.
--
-- One row per user (user_id UNIQUE): rotating replaces the row's token rather
-- than accumulating live URLs, so a rotation immediately kills the old link.
CREATE TABLE user_calendar_links (
    token      TEXT NOT NULL PRIMARY KEY,
    user_id    TEXT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    created_at DATETIME NOT NULL,
    rotated_at DATETIME
);
