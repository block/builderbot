-- Moving a branch to another project used to leave its child notes'
-- `parent_project_note_id` pointing at a project note in the source project.
-- The note stayed hidden from the branch's timeline in its new project, and
-- deleting that project note deleted the note out of a project it no longer
-- shared. A move now clears the link; clear it here for branches moved before
-- that.
--
-- Only a link to a project note that exists in another project is cut. A link
-- to a project note that is gone, or to one in the branch's own project, is
-- left alone.
UPDATE notes
   SET parent_project_note_id = NULL
 WHERE parent_project_note_id IS NOT NULL
   AND EXISTS (
       SELECT 1
         FROM project_notes pn
         JOIN branches b ON b.id = notes.branch_id
        WHERE pn.id = notes.parent_project_note_id
          AND pn.project_id <> b.project_id
   );
