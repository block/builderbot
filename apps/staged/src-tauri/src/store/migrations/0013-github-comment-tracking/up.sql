ALTER TABLE comments ADD COLUMN github_comment_id INTEGER;
ALTER TABLE comments ADD COLUMN github_comment_type TEXT;
ALTER TABLE comments ADD COLUMN github_comment_stale INTEGER NOT NULL DEFAULT 0;
