-- Display preferences are per user and client, as in Jellyfin: every user
-- saves under the same ids ("usersettings", a library's id), so the id alone
-- let the last user to save take the row from everyone else.
PRAGMA foreign_keys = OFF;

CREATE TABLE jellyfin_display_prefs_new (
    id      TEXT NOT NULL,
    user_id TEXT NOT NULL,
    client  TEXT NOT NULL,
    data    TEXT NOT NULL,
    PRIMARY KEY (id, user_id, client),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

INSERT INTO jellyfin_display_prefs_new SELECT * FROM jellyfin_display_prefs;
DROP TABLE jellyfin_display_prefs;
ALTER TABLE jellyfin_display_prefs_new RENAME TO jellyfin_display_prefs;

PRAGMA foreign_keys = ON;
