-- When the user last opened a note or project note, in ms from the backend
-- clock (the same clock as `completed_at`). A note reads as unread from the
-- moment its content lands (`completed_at`) until it is opened after that;
-- "Mark as unread" clears it.
ALTER TABLE notes ADD COLUMN read_at INTEGER;
ALTER TABLE project_notes ADD COLUMN read_at INTEGER;
-- Treat notes from before the upgrade as read so it doesn't light up every old
-- note. Ones still being written stay NULL and read as unread when they land.
UPDATE notes SET read_at = completed_at;
UPDATE project_notes SET read_at = completed_at;
