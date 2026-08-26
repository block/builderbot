-- The auto-review feature (background reviews started automatically after
-- commit sessions and when adding repos) has been removed. Delete the rows
-- it created — reviews adopted by the user were already flipped to
-- is_auto = 0 — and drop the flag. Children (reviewed_files, comments,
-- reference_files) cascade.
DELETE FROM reviews WHERE is_auto = 1;
ALTER TABLE reviews DROP COLUMN is_auto;
