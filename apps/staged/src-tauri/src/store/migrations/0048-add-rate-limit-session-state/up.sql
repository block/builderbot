-- Persist manual recovery state for sessions paused by provider rate/usage limits.
ALTER TABLE sessions ADD COLUMN rate_limit_resets_at TEXT;
ALTER TABLE sessions ADD COLUMN rate_limit_kind TEXT;
