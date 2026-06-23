ALTER TABLE session_messages ADD COLUMN acp_protocol_version TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_agent_capabilities TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_auth_methods TEXT DEFAULT NULL;
ALTER TABLE session_messages ADD COLUMN acp_agent_info TEXT DEFAULT NULL;

CREATE INDEX idx_session_messages_acp_event
    ON session_messages(session_id, acp_event_kind);
