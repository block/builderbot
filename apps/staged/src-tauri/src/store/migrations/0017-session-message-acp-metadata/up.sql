ALTER TABLE session_messages ADD COLUMN acp_event_kind TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_message_id TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_tool_call_id TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_tool_kind TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_tool_status TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_raw_input TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_raw_output TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_content TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_locations TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_usage TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_session_info TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_config_options TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_session_mode_state TEXT DEFAULT NULL;

CREATE INDEX idx_session_messages_acp_message
    ON session_messages(session_id, acp_message_id);
CREATE INDEX idx_session_messages_acp_tool_call
    ON session_messages(session_id, acp_tool_call_id);
