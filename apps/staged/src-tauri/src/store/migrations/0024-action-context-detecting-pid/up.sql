-- The pid of the Staged process that claimed a context's detection window,
-- mirroring `sessions.owner_pid` / `queued_session_messages.owner_pid`. The
-- `detecting_actions` flag is only ever cleared by code inside the process
-- that set it, so a hard kill mid-detection leaves it set forever and every
-- later detection for that repo is rejected as already in progress. Recording
-- the owner lets a startup sweep tell an orphan apart from a window a second,
-- still-live Staged instance is running.
ALTER TABLE action_contexts ADD COLUMN detecting_pid INTEGER DEFAULT NULL;

-- Any row reaching this migration with the flag set is orphaned by definition:
-- a detection window cannot outlive the process that claimed it, and no
-- shipped build ever cleared the flag from outside that process. This heals
-- every database already wedged, which no amount of new runtime code reaches.
UPDATE action_contexts SET detecting_actions = 0 WHERE detecting_actions = 1;
