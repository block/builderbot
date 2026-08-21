-- Add ordered suggested next step storage to notes and project_notes.
-- Legacy single-step columns remain for compatibility and fallback conversion.

ALTER TABLE notes ADD COLUMN suggested_next_steps TEXT;
ALTER TABLE project_notes ADD COLUMN suggested_next_steps TEXT;
