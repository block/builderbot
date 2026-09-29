-- A user-chosen project status that replaces the computed PR/cloud icon.
-- Holds the id of an option from the frontend's `project-status-options`
-- preference; NULL means Default (show the computed status). Ids that no
-- longer match an option render as Default too, so deleting an option needs
-- no cleanup here.
ALTER TABLE projects ADD COLUMN status_override TEXT;
