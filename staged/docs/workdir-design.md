# Workdir Design: Separating Branches from Working Directories

## Problem

Staged currently hard-couples branches to worktrees: every `Branch` has a
`worktree_path: TEXT NOT NULL`, and creating a branch always means
`git worktree add`. This works well for normal-sized repos but falls apart for
large monorepos where each worktree costs gigabytes of disk and minutes of
setup time (dependency installs, build caches, etc.).

We want to support parallel agent work as the default, but also support users
who can't afford N full working copies on disk.

## Core Idea

Separate **what you're working on** (a branch) from **where the work happens**
(a working directory). A working directory is a pooled resource owned by the
project. Branches are assigned to a workdir when they need one and release it
when they don't.

```
Project
  ├── workdirs[]       pool of filesystem locations
  └── branches[]       logical branches, assigned to a workdir when active
```

## Target Data Model

### Workdir (new table)

A filesystem location where git operations can happen. Could be the main
checkout itself, or a `git worktree`.

```sql
CREATE TABLE workdirs (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path            TEXT NOT NULL,
    branch_id       TEXT REFERENCES branches(id) ON DELETE SET NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE(project_id, path)
);
```

`branch_id` is nullable — a workdir with no branch is **available**. A workdir
with a branch is **occupied**. The UNIQUE constraint on `(project_id, path)`
prevents duplicates. A separate unique constraint or application-level check
ensures at most one workdir points to a given `branch_id`.

### Branch (changed)

Drop `worktree_path`. A branch's working directory is found by joining through
`workdirs` — or it may not have one at all (branch exists in git but isn't
checked out anywhere).

```sql
CREATE TABLE branches (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    branch_name     TEXT NOT NULL,
    base_branch     TEXT NOT NULL,
    pr_number       INTEGER,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE(project_id, branch_name)
);
```

### How Modes Emerge

No strategy enum needed. The mode is determined by how many workdirs a project
has and how they're provisioned:

| Mode                       | Workdirs                  | Parallelism | How it works                                                                                                                     |
| -------------------------- | ------------------------- | ----------- | -------------------------------------------------------------------------------------------------------------------------------- |
| **Full** (current default) | 1 per branch              | Unlimited   | Creating a branch also creates a workdir via `git worktree add`. 1:1 lifetime.                                                   |
| **Shared** (monorepo)      | 1 total (the repo itself) | Serial      | All branches share the repo checkout. Activating a branch does `git checkout` in that workdir. Only one branch active at a time. |
| **Pool** (future)          | Fixed N (e.g. 3-4)        | Up to N     | User pre-creates a few persistent worktrees. Branches claim an available workdir when they need one, release when done.          |

### Key Query: "Where do I run git for this branch?"

```sql
SELECT w.path
FROM workdirs w
WHERE w.branch_id = ?
```

Returns one row if the branch is checked out somewhere, zero rows if it isn't.
All downstream code that currently reads `branch.worktree_path` would use this
instead.

### Key Query: "Can I start work on this branch?"

```sql
-- Find an available workdir for this project
SELECT w.id, w.path
FROM workdirs w
WHERE w.project_id = ? AND w.branch_id IS NULL
LIMIT 1
```

If a row comes back, assign the branch to it (checkout + update `branch_id`).
If no row comes back, the branch queues — the UI can show "waiting for a
workdir" and the scheduler retries when one frees up.

## What to Do Now (Before Implementing Shared/Pool)

The current model only needs to support "full" mode today. The question is:
what do we change _now_ so that adding workdirs later is easy and doesn't
require a painful migration?

### Recommended: introduce `workdirs` now, keep it 1:1

Add the `workdirs` table. When creating a branch with `git worktree add`,
also create a workdir row and link them. Branch keeps a `workdir_id` for
convenience (nullable FK), but the workdir table is the source of truth for
the path.

**Branch model becomes:**

```rust
pub struct Branch {
    pub id: String,
    pub project_id: String,
    pub branch_name: String,
    pub base_branch: String,
    pub pr_number: Option<u64>,
    pub created_at: i64,
    pub updated_at: i64,
}
```

No `worktree_path`. Code that needs the path does:

```rust
let workdir = store.get_workdir_for_branch(&branch.id)?;
let path = workdir.map(|w| w.path);
```

**Workdir model:**

```rust
pub struct Workdir {
    pub id: String,
    pub project_id: String,
    pub path: String,
    pub branch_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
```

### Why not just keep `worktree_path` on Branch for now?

It's tempting to defer, but `worktree_path: TEXT NOT NULL` bakes in two
assumptions that are wrong for shared/pool modes:

1. **Every branch always has a path** — in shared/pool modes, most branches
   won't be checked out anywhere. Making this nullable is a half-measure that
   leaves the field on the wrong table.

2. **The path is a static property of the branch** — in pool mode the path
   changes every time the branch gets assigned to a different workdir. Storing
   it on the branch means updating it on every swap, with the workdir table
   as the actual source of truth anyway.

Introducing the workdir table now means:

- The Branch struct and schema are clean — no field that's "sometimes null,
  sometimes stale, will be moved later."
- All code that touches working directory paths goes through one pattern from
  the start, so there's no scattered `branch.worktree_path` usage to hunt
  down later.
- Adding shared/pool modes is purely additive: change how workdirs are
  provisioned, add scheduling logic. No schema migration, no model refactor.

### What about the main repo checkout?

When a project is created in "full" mode, the main repo checkout at
`project.repo_path` is _not_ tracked as a workdir. It's just where we run
`git worktree add` from. Workdirs are only the locations we manage.

When we later add "shared" mode, the repo path itself _becomes_ the single
workdir. That's when we create a workdir row pointing to `project.repo_path`.
This is a clean additive change — no existing rows need updating.
