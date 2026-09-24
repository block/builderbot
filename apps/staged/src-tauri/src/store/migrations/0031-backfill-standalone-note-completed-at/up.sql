-- A note with no session is never "still generating": nothing will ever fill it
-- in. Until now `completed_at` was only set when the body was non-empty, so a
-- note written as a single line — whole text in the title, empty body — was
-- stored with the same fingerprint as an in-flight session stub and was skipped
-- by the session preprompt and the `#note:` picker alike.
--
-- Backfill with `updated_at`, the same approximation 0007 used when the column
-- was added. Session-owned stubs keep NULL so a generating or failed note stays
-- hidden.
UPDATE notes SET completed_at = COALESCE(completed_at, updated_at) WHERE session_id IS NULL;
