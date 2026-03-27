CREATE TABLE repo_badges (
    github_repo TEXT NOT NULL,
    subpath     TEXT NOT NULL DEFAULT '',
    short_name  TEXT NOT NULL,
    hue         REAL NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (github_repo, subpath)
);

CREATE UNIQUE INDEX idx_repo_badges_short_name ON repo_badges (short_name);
