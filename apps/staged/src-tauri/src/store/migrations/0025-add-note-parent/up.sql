ALTER TABLE notes ADD COLUMN parent_project_note_id TEXT;
CREATE INDEX idx_notes_parent_project ON notes(parent_project_note_id);
