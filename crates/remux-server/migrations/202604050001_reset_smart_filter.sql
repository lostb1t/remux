-- Reset stored smart filter blobs; the serialization format changed with the typed FilterRule enum.
UPDATE media SET collection_smart_filter = NULL WHERE collection_smart_filter IS NOT NULL;
