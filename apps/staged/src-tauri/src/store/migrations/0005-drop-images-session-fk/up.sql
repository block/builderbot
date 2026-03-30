-- Drop the foreign key constraint on images.session_id so that pending
-- images can use a sentinel session_id value without violating referential
-- integrity.  SQLite does not support ALTER TABLE … DROP CONSTRAINT, so we
-- recreate the table.

-- 1. Drop ALL triggers that reference the images table (directly or in
--    subqueries).  They will be recreated after the table swap.
DROP TRIGGER IF EXISTS trg_cleanup_session_after_commit_delete;
DROP TRIGGER IF EXISTS trg_cleanup_session_after_note_delete;
DROP TRIGGER IF EXISTS trg_cleanup_session_after_review_delete;
DROP TRIGGER IF EXISTS trg_cleanup_session_after_project_note_delete;
DROP TRIGGER IF EXISTS trg_cleanup_session_after_image_delete;

-- 2. Recreate the table without the FK on session_id.
CREATE TABLE images_new (
    id          TEXT PRIMARY KEY,
    branch_id   TEXT REFERENCES branches(id) ON DELETE CASCADE,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id  TEXT,
    filename    TEXT NOT NULL,
    mime_type   TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);

INSERT INTO images_new SELECT * FROM images;

DROP TABLE images;

ALTER TABLE images_new RENAME TO images;

-- 3. Recreate the index.
CREATE INDEX idx_images_branch ON images(branch_id);

-- 4. Recreate all cleanup triggers.
CREATE TRIGGER trg_cleanup_session_after_commit_delete
AFTER DELETE ON commits
WHEN OLD.session_id IS NOT NULL
BEGIN
    DELETE FROM sessions
    WHERE id = OLD.session_id
      AND status != 'running'
      AND NOT EXISTS (SELECT 1 FROM commits WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM reviews WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM project_notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM images WHERE session_id = OLD.session_id);
END;

CREATE TRIGGER trg_cleanup_session_after_note_delete
AFTER DELETE ON notes
WHEN OLD.session_id IS NOT NULL
BEGIN
    DELETE FROM sessions
    WHERE id = OLD.session_id
      AND status != 'running'
      AND NOT EXISTS (SELECT 1 FROM commits WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM reviews WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM project_notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM images WHERE session_id = OLD.session_id);
END;

CREATE TRIGGER trg_cleanup_session_after_review_delete
AFTER DELETE ON reviews
WHEN OLD.session_id IS NOT NULL
BEGIN
    DELETE FROM sessions
    WHERE id = OLD.session_id
      AND status != 'running'
      AND NOT EXISTS (SELECT 1 FROM commits WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM reviews WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM project_notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM images WHERE session_id = OLD.session_id);
END;

CREATE TRIGGER trg_cleanup_session_after_project_note_delete
AFTER DELETE ON project_notes
WHEN OLD.session_id IS NOT NULL
BEGIN
    DELETE FROM sessions
    WHERE id = OLD.session_id
      AND status != 'running'
      AND NOT EXISTS (SELECT 1 FROM commits WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM reviews WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM project_notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM images WHERE session_id = OLD.session_id);
END;

CREATE TRIGGER trg_cleanup_session_after_image_delete
AFTER DELETE ON images
WHEN OLD.session_id IS NOT NULL
BEGIN
    DELETE FROM sessions
    WHERE id = OLD.session_id
      AND status != 'running'
      AND NOT EXISTS (SELECT 1 FROM commits WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM reviews WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM project_notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM images WHERE session_id = OLD.session_id);
END;
