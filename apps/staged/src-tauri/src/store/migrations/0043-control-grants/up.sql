-- Durable class B control grants: an outside client the user approved by
-- name in the consent dialog. The row holds only the SHA-256 of the client's
-- reconnect secret (the verification key for its reconnect proofs); the
-- live bearer the client drives the app with is in-memory and dies with the
-- app. `scope_project_id` NULL means every project. `access` mirrors the
-- CLI's --read-only flag.
CREATE TABLE control_grants (
    id                    TEXT PRIMARY KEY,
    client                TEXT NOT NULL,
    scope_project_id      TEXT REFERENCES projects(id) ON DELETE CASCADE,
    access                TEXT NOT NULL CHECK (access IN ('read', 'write')),
    reconnect_secret_hash TEXT NOT NULL UNIQUE,
    created_at            INTEGER NOT NULL,
    expires_at            INTEGER NOT NULL,
    last_used_at          INTEGER,
    revoked_at            INTEGER
);

CREATE INDEX idx_control_grants_project ON control_grants(scope_project_id);
