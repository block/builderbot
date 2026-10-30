-- When the user last read a session's chat, in ms from the backend clock (the
-- same clock as `session_messages.created_at`). A note row lights its chat
-- button while the chat has replies newer than both the note and this.
ALTER TABLE sessions ADD COLUMN chat_read_at INTEGER;
-- Treat existing chats as read so the upgrade doesn't light up every old note.
UPDATE sessions SET chat_read_at = CAST(strftime('%s', 'now') AS INTEGER) * 1000;
