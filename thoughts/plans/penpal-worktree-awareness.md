# Penpal Worktree Awareness

## Problem

Penpal currently treats each project as a single directory rooted at its filesystem path. It has no concept of git worktrees — in fact, it actively hides worktree directories from its file browser. When a developer (or their agents) works across multiple worktrees of the same repository, Penpal can't distinguish between them. Comments, threads, and file views are all scoped to the single "main" project path.

This means:

- **Agents in worktrees are invisible.** An agent running in `/repo/.claude/worktrees/foo/` calls `penpal_find_project` and either gets matched to the parent repo (wrong context) or fails to match at all.
- **Comments from worktree agents have no home.** If an agent creates a thread while working in a worktree, the thread is anchored to the main project's comment store, even though the file content may differ between worktrees.
- **No way to see what's happening across worktrees.** A developer reviewing agent work has no visibility into which worktree produced which comments or changes.
- **File content is always from the main checkout.** When viewing a file in Penpal, you always see the main worktree's version, never a branch-specific worktree version.

## Goals

1. A developer can see all active worktrees for a project and navigate between them.
2. An agent running in a worktree can find and interact with the correct Penpal project context automatically.
3. Comments and threads are scoped to the worktree they were created in.
4. The UI makes it clear which worktree you're looking at and provides easy switching.
5. Worktrees that are cleaned up disappear from the UI gracefully.

## Non-Goals

- Creating or deleting worktrees from the Penpal UI.
- Merging or diffing across worktrees (that's git tooling).
- Supporting worktrees from different remotes or unrelated repositories.

## Key Concepts

### Worktree as a Project Variant

A worktree is not a separate project — it's a variant of the same project. The mental model should be: one project, multiple working copies. This is analogous to how a browser has one site with multiple tabs, not multiple sites.

### Worktree Identity

Each worktree has:
- A **filesystem path** (e.g., `/repo/.claude/worktrees/fancy-name/`)
- A **branch** it's checked out to
- A **name** derived from the directory name (e.g., `fancy-name`)
- A **qualified name** using the `@` suffix (e.g., `Development/repo@fancy-name`)
- A relationship to a **main worktree** (the original clone)

The main worktree is the default and uses the bare qualified name (`Development/repo`) or equivalently `Development/repo@main`. Worktrees are additive — they appear alongside the main view, not instead of it. Resolution uses two-phase lookup: match the project by longest-prefix on the part before `@`, then resolve the worktree from the suffix.

## Requirements

### R1: Worktree Discovery

Penpal must discover worktrees associated with each project.

- **R1.1:** On project discovery, detect all git worktrees by parsing `.git/worktrees/` in the main repo or by running `git worktree list`.
- **R1.2:** Each discovered worktree becomes a navigable variant of its parent project, not a separate top-level project.
- **R1.3:** Worktree discovery must handle the `.claude/worktrees/` convention (where agent worktrees typically live) but also support worktrees at arbitrary paths.
- **R1.4:** The filesystem watcher must watch for worktree creation and deletion so the UI updates live.

### R2: Project Resolution for Agents

Agents running in worktrees must be able to resolve their project context correctly.

- **R2.1:** `penpal_find_project` must resolve paths inside a worktree to the correct project + worktree identifier. For example, `/repo/.claude/worktrees/foo/thoughts/plan.md` should resolve to project `Development/repo` with worktree `foo`.
- **R2.2:** All MCP tools (`penpal_list_threads`, `penpal_create_thread`, `penpal_reply`, etc.) must accept an optional `worktree` parameter to scope operations to a specific worktree.
- **R2.3:** When `worktree` is omitted, tools operate on the main worktree (backward compatible).
- **R2.4:** `penpal_find_project` should return the worktree identifier in its response so agents don't need to compute it themselves.

### R3: Worktree-Scoped Comments

Comments and threads must be scoped to the worktree they belong to.

- **R3.1:** Each worktree gets its own comment storage namespace. A thread created in worktree `foo` must not appear when viewing the main worktree's version of the same file.
- **R3.2:** Comment sidecar files for worktrees should be stored under the worktree's own `.penpal/comments/` directory (i.e., at the worktree path, not the main repo path).
- **R3.3:** When a worktree is deleted, its comments become orphaned but are not automatically deleted. They should be queryable via an archival mechanism or simply left on disk.

### R4: UI Navigation

The UI must make worktrees discoverable and navigable.

- **R4.1:** The project view must show active worktrees — their name, branch, and file activity.
- **R4.2:** Provide a worktree switcher when viewing a file that exists in multiple worktrees. This could be a dropdown, tab bar, or sidebar element.
- **R4.3:** The URL structure must encode the worktree. Proposed: `/file/{qualifiedName}/@{worktree}/{filePath}`. The worktree segment is always required. The main worktree uses `@main`. Old URLs without a worktree segment are redirected to `@main` for backward compatibility. Examples:
  - `/file/Development/repo/@main/thoughts/plan.md` — main worktree
  - `/file/Development/repo/thoughts/plan.md` → redirects to `@main` variant
  - `/file/Development/repo/@fancy-name/thoughts/plan.md` — worktree variant
- **R4.4:** The file browser sidebar, when scoped to a project, should show which worktree is currently selected and allow switching.
- **R4.5:** "In Review" and "Recent" views should indicate which worktree each file belongs to and allow filtering by worktree.

### R5: Real-Time Updates

SSE events must be worktree-aware.

- **R5.1:** `EventFilesChanged` must include the worktree identifier so the frontend can update the correct view.
- **R5.2:** `EventCommentsChanged` must include the worktree identifier.
- **R5.3:** The filesystem watcher must watch worktree directories for file changes, not just the main project root.
- **R5.4:** `EventProjectsChanged` should fire when worktrees are created or removed.

### R6: Agent Visibility

The UI should show agent activity per-worktree.

- **R6.1:** Agent heartbeats (from MCP tool calls) should be associated with their worktree.
- **R6.2:** The project overview should show which worktrees have active agents.

### R7: Lifecycle

Worktrees are ephemeral. The system must handle their transient nature.

- **R7.1:** When a worktree directory is deleted, Penpal must stop watching it and remove it from the active worktree list within one watcher cycle.
- **R7.2:** Stale worktrees (directory gone) must not cause errors in the UI or API.
- **R7.3:** If a worktree is re-created at the same path, it should be treated as a new worktree (fresh comment state, since the old `.penpal/` directory would have been deleted with the worktree).

## Data Model Changes

### Project (updated)

```
Project {
  ...existing fields...
  Worktrees []Worktree   // discovered worktrees for this project
  IsWorktree bool        // true if this project IS a worktree (for internal tracking)
  MainWorktreePath string // if IsWorktree, path to the main repo
}
```

### Worktree (new)

```
Worktree {
  Name     string    // directory name (e.g., "fancy-name")
  Path     string    // absolute filesystem path
  Branch   string    // checked-out branch
  IsMain   bool      // true for the original clone
  AgentCount int     // number of active agents in this worktree
}
```

### Cache / FileInfo (updated)

```
FileInfo {
  ...existing fields...
  Worktree string    // worktree name, empty for main
}
```

## API Changes

### MCP Tools

All existing tools gain an optional `worktree` parameter:

| Tool | Change |
|------|--------|
| `penpal_find_project` | Response adds `worktree` field |
| `penpal_list_threads` | Add optional `worktree` param |
| `penpal_read_thread` | Add optional `worktree` param |
| `penpal_reply` | Add optional `worktree` param |
| `penpal_create_thread` | Add optional `worktree` param |
| `penpal_files_in_review` | Add optional `worktree` param |
| `penpal_wait_for_changes` | Add optional `worktree` param; events scoped to worktree |

### REST API

| Endpoint | Change |
|----------|--------|
| `GET /api/projects` | Include `worktrees` array per project |
| `GET /api/files/{project}` | Add `?worktree=` query param |
| `GET /api/file/{project}/{path}` | Add `?worktree=` query param; serves file from worktree path |
| `GET /events` | Events include `worktree` field where applicable |

## Backward Compatibility & Migration

This section covers how pre-existing Penpal installs with existing data will behave after the worktree-awareness update.

### What Breaks (Acceptable)

**MCP tools and skills** — MCP tool schemas change (new `worktree` parameter, new fields in responses). Agents using the old schema will still work because the `worktree` parameter is optional and defaults to main. However, agent skills (like `penpal:monitor`) will be updated to pass worktree context. This is acceptable because Penpal offers to upgrade skills on update.

### What Must Not Break

**Existing comment data.** Today, comments are stored as sidecar JSON files at:
```
{project.Path}/.penpal/comments/{filePath}.json
```

After this change, main worktree comments stay at exactly this path — no migration needed. The main worktree's comment store IS the existing comment store. Worktree-specific comments go to a new location:
```
{worktree.Path}/.penpal/comments/{filePath}.json
```

Since worktrees are new, there is no existing data at worktree paths. No data migration required.

**Existing URLs and bookmarks.** Today, file URLs look like:
```
/file/Development/repo/thoughts/plan.md
```

After this change, the canonical URL format becomes:
```
/file/Development/repo/@main/thoughts/plan.md
```

Old URLs without the `@worktree` segment must be detected and **301-redirected** to the `@main` variant. Detection: if the path segment immediately after the qualified project name does not start with `@`, treat it as a legacy URL and redirect. This preserves bookmarks, shared links, and browser history.

**Existing REST API calls.** All REST endpoints that gain a `?worktree=` query parameter default to the main worktree when the parameter is omitted. Existing API consumers (e.g., the current frontend) will continue to work without modification until they opt in to worktree support.

**SSE event streams.** Existing events gain a new `worktree` field. Clients that don't read this field are unaffected — they'll receive events from all worktrees (same as today's behavior of receiving all events from the single main worktree).

### Migration Steps

1. **Automatic (no user action):**
   - Existing comment sidecar files are recognized as main-worktree comments. No file moves or renames.
   - Old-format URLs are redirected to `@main` URLs.
   - MCP tools accept calls with or without `worktree` parameter.

2. **On skill upgrade (prompted by Penpal):**
   - Updated skills pass `worktree` context from `penpal_find_project` response.
   - Old skill versions continue to work (they just always target main worktree).

3. **No manual migration:**
   - No database changes (comments are file-based).
   - No config file changes needed.
   - No re-indexing of existing projects.

### Edge Cases

- **Agent creates comments from a worktree path but doesn't pass `worktree` param:** Comments land in the main worktree's store. This is the safe default — worse case is comments appear on main instead of being lost.
- **Worktree deleted while comments exist:** Comment sidecar files inside the worktree directory are deleted with the worktree. This is intentional — worktree comments are ephemeral by design (they track agent work on a branch). If preservation is needed, the agent or user should resolve/export before cleanup.
- **Multiple worktrees on the same branch:** Each worktree has its own comment namespace keyed by worktree name (directory name), not branch. Two worktrees on `feature-x` have independent comment stores.

