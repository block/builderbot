-- Link branch-scoped sessions that create no artifact directly to their branch.
--
-- Queued/running sessions are normally found for a branch through their commit,
-- note, or review row. Push pipelines produce none of those, so they were
-- invisible to the branch queue. Existing rows keep resolving via artifacts;
-- only sessions with no artifact need this column populated.
ALTER TABLE sessions ADD COLUMN branch_id TEXT DEFAULT NULL REFERENCES branches(id) ON DELETE CASCADE;
CREATE INDEX idx_sessions_branch ON sessions(branch_id);
