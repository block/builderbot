-- The auto-commit feature (an opt-in per action that ran `git add -A` plus a
-- `chore: <action>` commit in the worktree after the action succeeded) has been
-- removed: git stays authoritative, and commits are the user's to make. The
-- flag has no reader left, so drop it.
ALTER TABLE repo_actions DROP COLUMN auto_commit;
