-- Links an OpenID Connect identity (provider slug + `sub` claim) to a local user.
CREATE TABLE user_sso_links (
    provider   TEXT NOT NULL,
    subject    TEXT NOT NULL,
    user_id    BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (provider, subject)
);

CREATE INDEX idx_user_sso_links_user_id ON user_sso_links(user_id);
