-- Persist selected ACP config values for a session as category-keyed JSON.
ALTER TABLE sessions ADD COLUMN acp_config_selection TEXT DEFAULT NULL;
