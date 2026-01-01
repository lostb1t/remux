-- Auth sessions table (SQLite)

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS auth_devices (
    user_id      TEXT NOT NULL,
    id           TEXT NOT NULL,
    access_token TEXT NOT NULL UNIQUE,
    name  TEXT NOT NULL,
    app_name     TEXT NOT NULL,
    app_version  TEXT NOT NULL,

    PRIMARY KEY (user_id, id)
);

CREATE TABLE IF NOT EXISTS auth_users (
  id            TEXT NOT NULL PRIMARY KEY,
  username      TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  aio_url       TEXT NOT NULL
);
