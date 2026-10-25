-- `sessions.branch_id` becomes the scheduling key for every branch-scoped
-- session. Until now only the artifact-less push/pull pipelines recorded it;
-- commit, note and review sessions were linked to their branch solely through
-- the artifact row pointing back at them, so "what is queued or running on
-- this branch" was a three-way join re-derived on every read, and the link
-- vanished the moment the artifact was deleted — before the session's own
-- release path could tell which branch to move along.
--
-- Backfill once from those joins; new rows record the branch at insert.
-- Project-level and Pikchr child sessions have no artifact and stay NULL.
UPDATE sessions
   SET branch_id = (SELECT c.branch_id FROM commits c WHERE c.session_id = sessions.id LIMIT 1)
 WHERE branch_id IS NULL
   AND EXISTS (SELECT 1 FROM commits c WHERE c.session_id = sessions.id);
UPDATE sessions
   SET branch_id = (SELECT n.branch_id FROM notes n WHERE n.session_id = sessions.id LIMIT 1)
 WHERE branch_id IS NULL
   AND EXISTS (SELECT 1 FROM notes n WHERE n.session_id = sessions.id);
UPDATE sessions
   SET branch_id = (SELECT r.branch_id FROM reviews r WHERE r.session_id = sessions.id LIMIT 1)
 WHERE branch_id IS NULL
   AND EXISTS (SELECT 1 FROM reviews r WHERE r.session_id = sessions.id);
