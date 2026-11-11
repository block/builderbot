-- Record who started a session, when it was not the user in the app.
--
-- `started_by` names the control-server actor behind a session start made
-- through `stagedctl`: `session:<session-id>` for a session Staged spawned
-- (class A of the two-caller model) and `app:<grant-id>` for an outside
-- client the user approved by name (class B). NULL for every start made from
-- the app's own UI, and for every row that predates this column. Nothing
-- persisted this before: `parent_session_id` marks only MCP and pikchr
-- children, and the launch context is serialized into the prompt text.
ALTER TABLE sessions ADD COLUMN started_by TEXT DEFAULT NULL;
