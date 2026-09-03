-- Preserve structured provider/protocol metadata for later recovery logic.
-- Examples include ACP prompt errors, prompt responses, and usage updates.
ALTER TABLE sessions ADD COLUMN provider_metadata TEXT;
