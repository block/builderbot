-- The auto-commit option has been removed from project actions. Actions now
-- only execute their command and never create git commits, so the column that
-- backed the per-action toggle is no longer needed.
ALTER TABLE repo_actions DROP COLUMN auto_commit;
