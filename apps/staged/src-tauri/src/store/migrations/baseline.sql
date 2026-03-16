CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    github_repo TEXT,
    location    TEXT NOT NULL DEFAULT 'local',
    subpath     TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_projects_name ON projects(name);

CREATE TABLE project_repos (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    github_repo TEXT NOT NULL,
    branch_name TEXT NOT NULL DEFAULT 'main',
    subpath     TEXT,
    is_primary  INTEGER NOT NULL DEFAULT 0,
    reason      TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_project_repos_unique
    ON project_repos(project_id, github_repo, COALESCE(subpath, ''));
CREATE INDEX idx_project_repos_project ON project_repos(project_id);

CREATE TABLE branches (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    project_repo_id     TEXT REFERENCES project_repos(id) ON DELETE SET NULL,
    branch_name         TEXT NOT NULL,
    base_branch         TEXT NOT NULL,
    pr_number           INTEGER,
    branch_type         TEXT NOT NULL DEFAULT 'local',
    workspace_name      TEXT,
    workspace_status    TEXT,
    agent               TEXT,
    pr_state            TEXT,
    pr_checks_status    TEXT,
    pr_review_decision  TEXT,
    pr_mergeable        INTEGER,
    pr_draft            INTEGER,
    pr_url              TEXT,
    pr_updated_at       INTEGER,
    pr_fetched_at       INTEGER,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    UNIQUE(project_id, project_repo_id, branch_name)
);

CREATE TABLE workdirs (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path            TEXT NOT NULL,
    branch_id       TEXT REFERENCES branches(id) ON DELETE SET NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE(project_id, path)
);
CREATE INDEX idx_workdirs_project ON workdirs(project_id);
CREATE INDEX idx_workdirs_branch ON workdirs(branch_id);

CREATE TABLE sessions (
    id              TEXT PRIMARY KEY,
    prompt          TEXT NOT NULL,
    status          TEXT NOT NULL,
    working_dir     TEXT NOT NULL DEFAULT '',
    provider        TEXT,
    agent_id        TEXT,
    error_message   TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    owner_pid       INTEGER
);

CREATE TABLE commits (
    id              TEXT PRIMARY KEY,
    branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    sha             TEXT,
    session_id      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX idx_commits_branch ON commits(branch_id);
CREATE UNIQUE INDEX idx_commits_branch_sha
    ON commits(branch_id, sha) WHERE sha IS NOT NULL;

CREATE TABLE session_messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    content         TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    image_ids       TEXT DEFAULT NULL
);
CREATE INDEX idx_session_messages_session
    ON session_messages(session_id);

CREATE TABLE notes (
    id              TEXT PRIMARY KEY,
    branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    session_id      TEXT,
    title           TEXT NOT NULL,
    content         TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX idx_notes_branch ON notes(branch_id);

CREATE TABLE project_notes (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id      TEXT,
    title           TEXT NOT NULL,
    content         TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX idx_project_notes_project ON project_notes(project_id);

CREATE TABLE reviews (
    id              TEXT PRIMARY KEY,
    branch_id       TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    commit_sha      TEXT NOT NULL,
    scope           TEXT NOT NULL,
    session_id      TEXT,
    title           TEXT DEFAULT NULL,
    is_auto         INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
CREATE INDEX idx_reviews_branch ON reviews(branch_id);

CREATE TABLE reviewed_files (
    review_id   TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    PRIMARY KEY (review_id, path)
);

CREATE TABLE comments (
    id              TEXT PRIMARY KEY,
    review_id       TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
    path            TEXT NOT NULL,
    span_start      INTEGER NOT NULL,
    span_end        INTEGER NOT NULL,
    content         TEXT NOT NULL,
    author          TEXT NOT NULL DEFAULT 'user',
    comment_type    TEXT,
    created_at      INTEGER NOT NULL
);
CREATE INDEX idx_comments_review ON comments(review_id);

CREATE TABLE reference_files (
    review_id   TEXT NOT NULL REFERENCES reviews(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    PRIMARY KEY (review_id, path)
);

CREATE TABLE action_contexts (
    id                      TEXT PRIMARY KEY,
    github_repo             TEXT NOT NULL,
    subpath                 TEXT,
    has_detected_actions    INTEGER NOT NULL DEFAULT 0,
    detecting_actions       INTEGER NOT NULL DEFAULT 0,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_action_contexts_repo_subpath
    ON action_contexts(github_repo, COALESCE(subpath, ''));

CREATE TABLE repo_actions (
    id                  TEXT PRIMARY KEY,
    context_id          TEXT NOT NULL REFERENCES action_contexts(id) ON DELETE CASCADE,
    name                TEXT NOT NULL,
    command             TEXT NOT NULL,
    action_type         TEXT NOT NULL,
    sort_order          INTEGER NOT NULL,
    auto_commit         INTEGER NOT NULL DEFAULT 0,
    run_detection_mode  TEXT DEFAULT NULL,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);
CREATE INDEX idx_repo_actions_context ON repo_actions(context_id);

CREATE TABLE recent_repos (
    id              TEXT PRIMARY KEY,
    github_repo     TEXT NOT NULL,
    subpath         TEXT,
    last_used_at    INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_recent_repos_unique
    ON recent_repos(github_repo, COALESCE(subpath, ''));
CREATE INDEX idx_recent_repos_last_used
    ON recent_repos(last_used_at DESC);

CREATE TABLE images (
    id          TEXT PRIMARY KEY,
    branch_id   TEXT REFERENCES branches(id) ON DELETE CASCADE,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    session_id  TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    filename    TEXT NOT NULL,
    mime_type   TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX idx_images_branch ON images(branch_id);
