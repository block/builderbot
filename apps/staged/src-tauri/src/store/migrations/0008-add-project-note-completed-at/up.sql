-- Add completed_at to project_notes so queued project notes sort by when the
-- session finished producing them, not when they were queued.
--
-- NULL means the note hasn't completed yet (still queued/generating).
-- For existing rows we backfill with updated_at, which is the best
-- approximation we have.

ALTER TABLE project_notes ADD COLUMN completed_at INTEGER;
UPDATE project_notes SET completed_at = updated_at WHERE content != '';
