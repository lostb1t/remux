CREATE TABLE addon_users (
    addon_id BLOB NOT NULL REFERENCES addons(id) ON DELETE CASCADE,
    user_id  BLOB NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    PRIMARY KEY (addon_id, user_id)
);

CREATE INDEX idx_addon_users_user_id ON addon_users(user_id);
