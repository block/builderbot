-- Link a repo session spawned by `start_repo_session` back to the project
-- session that requested it.
--
-- Child repo sessions were previously unattributable to their parent: notes
-- carry `parent_project_note_id`, but that only covers the `child_note`
-- outcome and only note-to-project-note. Recording the parent on the session
-- itself covers all four outcomes uniformly, so the project detail view can
-- show which repo work a running project session is waiting on.
--
-- `SET NULL` rather than `CASCADE`: a child session and its artifact should
-- survive deletion of the parent.
ALTER TABLE sessions ADD COLUMN parent_session_id TEXT DEFAULT NULL REFERENCES sessions(id) ON DELETE SET NULL;
CREATE INDEX idx_sessions_parent ON sessions(parent_session_id);
