-- One-shot marker recording that a pipeline (pr/push) session's completion
-- outcome events were already delivered. Everything the completion hook gates
-- on (pipeline JSON, prompt, branch_id) persists on the session row forever, so
-- resuming a finished pr/push session re-ran the hook against the *old*
-- pipeline and transcript on the follow-up turn's completion — re-emitting
-- `pr-created`, or (destructively) re-clearing the branch's PR status because
-- the old all-succeeded push pipeline still classifies as a fresh success.
ALTER TABLE sessions ADD COLUMN completion_effects_at INTEGER DEFAULT NULL;

-- Backfill the existing inventory of finished pipeline sessions: those are
-- exactly the rows whose next resume would replay the effects.
UPDATE sessions SET completion_effects_at = updated_at
WHERE status = 'completed' AND pipeline IS NOT NULL;
