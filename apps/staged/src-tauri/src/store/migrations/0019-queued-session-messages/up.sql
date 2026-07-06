CREATE TABLE queued_session_messages (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    branch_id       TEXT REFERENCES branches(id) ON DELETE SET NULL,
    content         TEXT NOT NULL,
    image_ids       TEXT,
    status          TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'sending', 'sent')),
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    claimed_at      INTEGER,
    owner_pid       INTEGER,
    sent_message_id INTEGER REFERENCES session_messages(id) ON DELETE SET NULL
);

CREATE INDEX idx_queued_session_messages_session_status
    ON queued_session_messages(session_id, status, created_at);
CREATE INDEX idx_queued_session_messages_branch
    ON queued_session_messages(branch_id);
CREATE INDEX idx_queued_session_messages_sent_message
    ON queued_session_messages(sent_message_id);
