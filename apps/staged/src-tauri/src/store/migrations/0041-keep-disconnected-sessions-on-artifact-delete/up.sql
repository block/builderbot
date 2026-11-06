-- A `disconnected` session is a Blox session whose remote turn may still be
-- running on its workstation, with no Staged process attached. Deleting its
-- last artifact must not delete the session row out from under that turn,
-- any more than for a `running` one: the reconcile service still has to find
-- the row to record how the turn ended, and the branch queue still treats it
-- as busy.
--
-- Recreate the `trg_cleanup_session_after_*_delete` triggers (last defined in
-- 0005) with `disconnected` spared alongside `running`. The bodies are
-- otherwise unchanged.

DROP TRIGGER IF EXISTS trg_cleanup_session_after_commit_delete;
DROP TRIGGER IF EXISTS trg_cleanup_session_after_note_delete;
DROP TRIGGER IF EXISTS trg_cleanup_session_after_review_delete;
DROP TRIGGER IF EXISTS trg_cleanup_session_after_project_note_delete;
DROP TRIGGER IF EXISTS trg_cleanup_session_after_image_delete;

CREATE TRIGGER trg_cleanup_session_after_commit_delete
AFTER DELETE ON commits
WHEN OLD.session_id IS NOT NULL
BEGIN
    DELETE FROM sessions
    WHERE id = OLD.session_id
      AND status NOT IN ('running', 'disconnected')
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
      AND status NOT IN ('running', 'disconnected')
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
      AND status NOT IN ('running', 'disconnected')
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
      AND status NOT IN ('running', 'disconnected')
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
      AND status NOT IN ('running', 'disconnected')
      AND NOT EXISTS (SELECT 1 FROM commits WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM reviews WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM project_notes WHERE session_id = OLD.session_id)
      AND NOT EXISTS (SELECT 1 FROM images WHERE session_id = OLD.session_id);
END;
