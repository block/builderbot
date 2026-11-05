-- Persist what a Blox session needs to be picked back up after Staged loses
-- its connection to the remote agent (the new `disconnected` status).
--
-- `pre_head_sha` is the worktree HEAD when the session's run started. It was
-- only ever held in the runner's memory, so a session that outlives its runner
-- had no base to find the commits its remote turn made.
--
-- `blox_command_id` is the Blox `ws` command id of the remote agent process
-- serving the session. Nullable and not yet populated; reserved for
-- reconnecting a disconnected session.
--
-- `sessions.status` has no CHECK constraint, so `disconnected` needs no schema
-- change of its own.
ALTER TABLE sessions ADD COLUMN pre_head_sha TEXT DEFAULT NULL;
ALTER TABLE sessions ADD COLUMN blox_command_id TEXT DEFAULT NULL;
