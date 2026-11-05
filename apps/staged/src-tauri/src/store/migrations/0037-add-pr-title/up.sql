-- The title of a branch's linked PR, so project search can match it and say
-- why a project matched ("PR #123 · Fix login redirect").
--
-- Filled by the PR status poll, which already fetches every linked PR, and
-- straight away when a PR is detected or linked. No backfill: existing rows
-- pick their title up on the next poll.
ALTER TABLE branches ADD COLUMN pr_title TEXT;
