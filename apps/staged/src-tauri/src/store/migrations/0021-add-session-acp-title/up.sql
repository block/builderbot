-- Persist the latest ACP-provided session title (session_info_update) for display while running.
ALTER TABLE sessions ADD COLUMN acp_title TEXT DEFAULT NULL;
