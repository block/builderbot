-- Tags, single-choice tag groups and project tag assignments. Replaces
-- projects.status_override (0032) + the frontend `project-status-options`
-- preference, which Store::import_legacy_project_statuses migrates (see marker).
CREATE TABLE tag_groups (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    position      INTEGER NOT NULL,           -- order of pills, settings sections, board selector
    is_icon_group INTEGER NOT NULL DEFAULT 0 CHECK (is_icon_group IN (0, 1)),
    created_at    INTEGER NOT NULL,           -- ms, now_timestamp()
    updated_at    INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_tag_groups_name ON tag_groups (name COLLATE NOCASE);
-- At most one group drives the project icon; none = computed status everywhere.
CREATE UNIQUE INDEX idx_tag_groups_icon ON tag_groups (is_icon_group) WHERE is_icon_group = 1;

CREATE TABLE tags (
    id         TEXT PRIMARY KEY,               -- legacy slugs/UUIDs preserved by the import, else UUID v4
    -- NULL = ungrouped (multi-select). No ON DELETE action on purpose:
    -- delete_tag_group ungroups or deletes the tags itself first.
    group_id   TEXT REFERENCES tag_groups (id),
    name       TEXT NOT NULL,
    color      TEXT NOT NULL CHECK (color IN ('green','red','amber','blue','purple','cyan','gray')),
    icon       TEXT,                           -- kebab-case Lucide name, NULL = coloured dot
    position   INTEGER NOT NULL,               -- dense 0..n-1 within its group / the ungrouped set
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
-- Unique within a group, and among ungrouped tags ("Priority › High" and "Effort › High" coexist).
CREATE UNIQUE INDEX idx_tags_scope_name ON tags (ifnull(group_id, ''), name COLLATE NOCASE);
CREATE INDEX idx_tags_group ON tags (group_id, position);

CREATE TABLE project_tags (
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    tag_id     TEXT NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    -- Reserved for manual order within a board column (Phase 3).
    -- NULL = default newest-first order. Not read or written in v1.
    sort_order INTEGER,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, tag_id)
);
CREATE INDEX idx_project_tags_tag ON project_tags (tag_id);

-- One-shot marker. Present until Store::import_legacy_project_statuses has
-- seeded the Status group from preferences.json (unreadable from SQL) and
-- copied projects.status_override into project_tags; the import drops this
-- table in the same transaction. projects.status_override stays, frozen.
CREATE TABLE legacy_project_status_import (id INTEGER PRIMARY KEY CHECK (id = 1));
INSERT INTO legacy_project_status_import (id) VALUES (1);
