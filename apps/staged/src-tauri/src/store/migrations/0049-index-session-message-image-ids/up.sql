-- The image-reference check (`Store::referenced_by_chat` in store/images.rs)
-- asks whether any chat message attaches a given image by searching
-- `image_ids` for the quoted id. Nothing indexed that column, so every check
-- read all of `session_messages`, the largest table in the store, once per
-- image. Deleting or moving a note with images froze the window for seconds.
--
-- Messages without attachments store NULL (`image_ids_json` in
-- store/messages.rs), so a partial index over the non-NULL values holds only
-- the messages that attach something, and the check reads that instead of the
-- table. The query has to spell out `image_ids IS NOT NULL` itself: SQLite
-- does not infer it from `instr(image_ids, ?) > 0`, and without it the
-- partial index is not considered.
--
-- `queued_session_messages` is left alone: it holds one row per queued
-- follow-up, not one per message.

CREATE INDEX idx_session_messages_image_ids
    ON session_messages(image_ids) WHERE image_ids IS NOT NULL;
