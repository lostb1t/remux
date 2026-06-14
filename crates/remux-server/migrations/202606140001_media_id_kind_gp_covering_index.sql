-- Covering index for the nextup/shows query that JOINs media on id and needs
-- kind + grandparent_id. On a ROWID table with a BLOB primary key, a plain PK
-- lookup costs two B-tree hops (pk-index → rowid → main table). Including kind
-- and grandparent_id in this index makes the lookup one hop — the planner can
-- satisfy id + kind + grandparent_id entirely from the index leaf without
-- fetching the main row.
CREATE INDEX IF NOT EXISTS idx_media_id_kind_gp
    ON media(id, kind, grandparent_id);
