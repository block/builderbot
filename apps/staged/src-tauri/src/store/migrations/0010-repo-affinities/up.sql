CREATE TABLE repo_affinities (
    repo_a       TEXT NOT NULL,
    repo_b       TEXT NOT NULL,
    co_use_count INTEGER NOT NULL DEFAULT 1,
    last_seen_at INTEGER NOT NULL,
    PRIMARY KEY (repo_a, repo_b)
);
-- idx on repo_a is unnecessary: the PRIMARY KEY (repo_a, repo_b) already
-- covers lookups on repo_a alone. Only repo_b needs its own index.
CREATE INDEX idx_repo_affinities_b ON repo_affinities(repo_b);
