-- Remove regular series cast duplicated on episodes.
-- Episodes now only store genuine guest stars (actors not in the series-level cast).
DELETE FROM media_relations
WHERE relation_id IN (
    SELECT mr_ep.relation_id
    FROM media_relations mr_ep
    JOIN media ep ON ep.id = mr_ep.left_media_id AND ep.kind = 'episode'
    WHERE mr_ep.role = 'actor'
      AND EXISTS (
          SELECT 1 FROM media_relations mr_series
          WHERE mr_series.left_media_id = ep.grandparent_id
            AND mr_series.role = 'actor'
            AND mr_series.right_media_id = mr_ep.right_media_id
      )
);
