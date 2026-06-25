-- Migrate CollectionFilter from flat rules list to grouped format.
-- Old: {"match_mode":"all","rules":[...]}
-- New: {"match_mode":"all","groups":[{"match_mode":"all","rules":[...]}]}
--
-- json() wraps the array returned by json_extract so json_object embeds it as
-- JSON rather than re-quoting it as a string literal.
-- COALESCE guards against match_mode or rules being absent.

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
