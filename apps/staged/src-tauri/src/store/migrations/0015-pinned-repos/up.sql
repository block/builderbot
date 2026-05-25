ALTER TABLE repo_badges ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE repo_badges ADD COLUMN pin_sort_order INTEGER;
ALTER TABLE repo_badges ADD COLUMN default_branch TEXT;
CREATE INDEX idx_repo_badges_pinned ON repo_badges (pinned, pin_sort_order);
