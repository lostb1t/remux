PRAGMA foreign_keys = OFF;

CREATE TABLE jellyfin_display_prefs_new (
    id      TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL,
    client  TEXT NOT NULL,
    data    TEXT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

INSERT INTO jellyfin_display_prefs_new SELECT * FROM jellyfin_display_prefs;
DROP TABLE jellyfin_display_prefs;
ALTER TABLE jellyfin_display_prefs_new RENAME TO jellyfin_display_prefs;

PRAGMA foreign_keys = ON;
