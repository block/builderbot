-- Attribution for rows the agent produced outside a live user turn (a
-- background continuation while the session was held open). NULL means the row
-- belongs to a turn the user prompted.
ALTER TABLE session_messages ADD COLUMN acp_origin TEXT DEFAULT NULL;
