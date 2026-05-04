-- Add pipeline JSON column to sessions table.
-- When present, contains the serialized PipelineExecution state.
ALTER TABLE sessions ADD COLUMN pipeline TEXT;
