-- Track who requested a session cancellation when the source is known.
ALTER TABLE sessions ADD COLUMN cancellation_source TEXT;
