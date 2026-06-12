-- Drop the FOREIGN KEY constraint on left_media_id so that catalog UUIDs
-- (which identify addon catalogs, not media rows) can be stored as left_media_id
-- in catalog relations without requiring a corresponding row in media.
--
-- right_media_id keeps its FK + CASCADE so that deleting a media item still
-- removes all its incoming relations automatically.

PRAGMA foreign_keys = OFF;

CREATE TABLE media_relations_new (
    relation_id    TEXT NOT NULL PRIMARY KEY,
    left_media_id  TEXT NOT NULL,
    right_media_id TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    weight         INTEGER,
    role           TEXT,
    character      TEXT
);

INSERT INTO media_relations_new SELECT * FROM media_relations;

DROP TABLE media_relations;
ALTER TABLE media_relations_new RENAME TO media_relations;

CREATE UNIQUE INDEX uniq_media_relation
    ON media_relations (left_media_id, right_media_id, COALESCE(role, ''));

PRAGMA foreign_keys = ON;
