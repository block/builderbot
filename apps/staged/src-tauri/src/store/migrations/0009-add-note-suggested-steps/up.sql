-- Add suggested next step columns to notes and project_notes.
-- These store the AI-extracted prompts for follow-up actions.

ALTER TABLE notes ADD COLUMN suggested_next_commit_step TEXT;
ALTER TABLE notes ADD COLUMN suggested_next_note_step TEXT;

ALTER TABLE project_notes ADD COLUMN suggested_next_commit_step TEXT;
ALTER TABLE project_notes ADD COLUMN suggested_next_note_step TEXT;
