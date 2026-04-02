-- Add completed_at column to notes and reviews.
-- This records when the AI session finished producing the item, giving us a
-- stable timestamp for timeline sorting that won't shift on later edits
-- (unlike updated_at which bumps on every user interaction).
--
-- Commits don't need this column because their created_at is already set to
-- the git commit timestamp (i.e. the completion time).
--
-- NULL means the item hasn't completed yet (still queued/generating).
-- For existing rows we backfill with updated_at, which is the best
-- approximation we have.

ALTER TABLE notes ADD COLUMN completed_at INTEGER;
UPDATE notes SET completed_at = updated_at WHERE content != '';

ALTER TABLE reviews ADD COLUMN completed_at INTEGER;
UPDATE reviews SET completed_at = updated_at
  WHERE title IS NOT NULL
     OR id IN (SELECT DISTINCT review_id FROM comments);
