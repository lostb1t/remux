ALTER TABLE media ADD COLUMN collection_source TEXT;
ALTER TABLE media ADD COLUMN collection_default_sort TEXT;
ALTER TABLE media ADD COLUMN collection_default_sort_order TEXT;
ALTER TABLE media_catalog_items ADD COLUMN item_order INTEGER;
