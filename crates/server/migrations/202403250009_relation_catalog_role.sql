PRAGMA foreign_keys = OFF;

-- Add 'catalog' to the role CHECK on media_relations.
-- SQLite can't ALTER CHECK constraints, so recreate the table.
CREATE TABLE media_relations_new (
    relation_id   TEXT PRIMARY KEY,
    left_media_id TEXT NOT NULL,
    right_media_id TEXT NOT NULL,
    weight        INTEGER,
    role          TEXT CHECK (role IN ('actor', 'director', 'writer', 'catalog')),
    FOREIGN KEY (left_media_id)  REFERENCES media(id) ON DELETE CASCADE,
    FOREIGN KEY (right_media_id) REFERENCES media(id) ON DELETE CASCADE
);

INSERT INTO media_relations_new
    SELECT relation_id, left_media_id, right_media_id, weight, role
    FROM media_relations;

DROP TABLE media_relations;
ALTER TABLE media_relations_new RENAME TO media_relations;

CREATE UNIQUE INDEX uniq_media_relation
    ON media_relations (left_media_id, right_media_id, COALESCE(role, ''));

PRAGMA foreign_keys = ON;
