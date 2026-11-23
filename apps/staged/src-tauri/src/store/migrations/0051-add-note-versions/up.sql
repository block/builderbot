-- Keep the bodies a note had before it was revised, so the viewer can show
-- what changed.
--
-- A row is the (title, content) a note held until a write replaced it; the
-- note's current text stays in `notes` / `project_notes` and is never copied
-- here. So the table is empty until a note is first revised, and notes that
-- predate this migration have no rows: their history starts at their next
-- revision.
--
-- `note_id` is a `notes.id` or a `project_notes.id`. Both are UUIDs, so one
-- id names at most one row across the two tables and one table serves both
-- kinds. That is also why there is no foreign key: the id has two possible
-- parents. The triggers below do the cleanup a cascade would, including
-- when the note row itself goes in a cascade (a deleted branch or project).
--
-- `saved_at` is the superseded row's `updated_at`: when the text in this row
-- was written, not when it was replaced. The store keeps the newest few rows
-- per note (`store/note_versions.rs`), read newest first through the index.
CREATE TABLE note_versions (
    id        TEXT PRIMARY KEY,
    note_id   TEXT NOT NULL,
    title     TEXT NOT NULL,
    content   TEXT NOT NULL,
    saved_at  INTEGER NOT NULL
);
CREATE INDEX idx_note_versions_note ON note_versions(note_id, saved_at);

CREATE TRIGGER trg_notes_delete_versions AFTER DELETE ON notes
BEGIN
    DELETE FROM note_versions WHERE note_id = OLD.id;
END;

CREATE TRIGGER trg_project_notes_delete_versions AFTER DELETE ON project_notes
BEGIN
    DELETE FROM note_versions WHERE note_id = OLD.id;
END;
