-- Distinguishes user-authored notes (written directly in the editor dialog)
-- from agent/session notes. NULL = produced by a session; 'written' = authored
-- by the user and therefore editable in place.
ALTER TABLE notes ADD COLUMN subtype TEXT;

-- Existing session-less notes (drag-dropped files, saved action output) are
-- exactly the user-authored class, so they become editable too.
UPDATE notes SET subtype = 'written' WHERE session_id IS NULL;
