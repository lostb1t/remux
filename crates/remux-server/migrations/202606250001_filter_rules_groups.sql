-- Migrate CollectionFilter from flat rules list to grouped format.
-- Old: {"match_mode":"all","rules":[...]}
-- New: {"match_mode":"all","groups":[{"match_mode":"all","rules":[...]}]}
--
-- json() wraps the array returned by json_extract so json_object embeds it as
-- JSON rather than re-quoting it as a string literal.
-- COALESCE guards against match_mode or rules being absent.

UPDATE media
SET collection_smart_filter = json_object(
    'match_mode', COALESCE(json_extract(collection_smart_filter, '$.match_mode'), 'all'),
    'groups', json_array(
        json_object(
            'match_mode', COALESCE(json_extract(collection_smart_filter, '$.match_mode'), 'all'),
            'rules', json(COALESCE(json_extract(collection_smart_filter, '$.rules'), '[]'))
        )
    )
)
WHERE collection_smart_filter IS NOT NULL
  AND json_extract(collection_smart_filter, '$.groups') IS NULL;

UPDATE users
SET policy = json_set(
    policy,
    '$.filter_rules',
    json_object(
        'match_mode', COALESCE(json_extract(policy, '$.filter_rules.match_mode'), 'all'),
        'groups', json_array(
            json_object(
                'match_mode', COALESCE(json_extract(policy, '$.filter_rules.match_mode'), 'all'),
                'rules', json(COALESCE(json_extract(policy, '$.filter_rules.rules'), '[]'))
            )
        )
    )
)
WHERE json_extract(policy, '$.filter_rules') IS NOT NULL
  AND json_extract(policy, '$.filter_rules.groups') IS NULL;

-- Fix old catalog rules that stored a single UUID as catalog_id instead of catalog_ids:[...].

UPDATE media
SET collection_smart_filter = json_set(
    collection_smart_filter,
    '$.groups[0].rules',
    (
        SELECT json_group_array(
            CASE
                WHEN json_extract(rule.value, '$.field') = 'catalog'
                     AND json_extract(rule.value, '$.catalog_id') IS NOT NULL
                THEN json(json_set(
                    json_remove(rule.value, '$.catalog_id'),
                    '$.catalog_ids',
                    json_array(json_extract(rule.value, '$.catalog_id'))
                ))
                ELSE json(rule.value)
            END
            ORDER BY rule.key
        )
        FROM json_each(collection_smart_filter, '$.groups[0].rules') AS rule
    )
)
WHERE collection_smart_filter IS NOT NULL
  AND json_extract(collection_smart_filter, '$.groups') IS NOT NULL
  AND EXISTS (
      SELECT 1 FROM json_each(collection_smart_filter, '$.groups[0].rules') AS r
      WHERE json_extract(r.value, '$.field') = 'catalog'
        AND json_extract(r.value, '$.catalog_id') IS NOT NULL
  );

UPDATE users
SET policy = json_set(
    policy,
    '$.filter_rules.groups[0].rules',
    (
        SELECT json_group_array(
            CASE
                WHEN json_extract(rule.value, '$.field') = 'catalog'
                     AND json_extract(rule.value, '$.catalog_id') IS NOT NULL
                THEN json(json_set(
                    json_remove(rule.value, '$.catalog_id'),
                    '$.catalog_ids',
                    json_array(json_extract(rule.value, '$.catalog_id'))
                ))
                ELSE json(rule.value)
            END
            ORDER BY rule.key
        )
        FROM json_each(policy, '$.filter_rules.groups[0].rules') AS rule
    )
)
WHERE json_extract(policy, '$.filter_rules.groups') IS NOT NULL
  AND EXISTS (
      SELECT 1 FROM json_each(policy, '$.filter_rules.groups[0].rules') AS r
      WHERE json_extract(r.value, '$.field') = 'catalog'
        AND json_extract(r.value, '$.catalog_id') IS NOT NULL
  );
