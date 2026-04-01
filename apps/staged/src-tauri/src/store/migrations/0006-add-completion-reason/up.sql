-- Add completion_reason column to sessions to distinguish why a session
-- reached its terminal state (turn_complete, interrupted, crashed, app_quit, unknown).
ALTER TABLE sessions ADD COLUMN completion_reason TEXT;

-- Backfill existing terminal sessions with 'unknown'.
UPDATE sessions SET completion_reason = 'unknown'
WHERE status IN ('completed', 'error', 'cancelled');
