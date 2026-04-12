# Fix: `penpal open` Command Hanging

## Problem

Running `penpal open <path>` would hang indefinitely instead of opening the file in the Penpal app.

The root cause was in the `/api/open` HTTP handler. When adding a new project, the handler called `refreshAfterConfigChange()` **synchronously**, which runs full project re-discovery — including spawning `git worktree list --porcelain` as a subprocess for **every project** in every workspace. With ~100 projects, that's ~100 sequential git calls blocking the HTTP response. The CLI, using Go's default `http.Client` (no timeout), waited forever.

### Goroutine dump evidence

```
goroutine 38 [chan receive]:    ← HTTP handler goroutine
  os/exec.(*Cmd).Wait
  discovery.DiscoverWorktrees    ← blocked on git subprocess
  server.discoverAllProjects
  server.refreshAfterConfigChange
  server.resolveOpenDirectory    ← /api/open handler
```

Meanwhile, ~90 goroutines were queued on the background enrichment semaphore, and the handler couldn't return until all discovery completed.

## Solution

**Make the `/api/open` handler non-blocking** by separating "register the project" (fast) from "re-discover everything" (slow).

### Key changes

1. **`Cache.AddProject()`** — New method to register a single project in the cache without full re-discovery. The handler loads just the requested project via `LoadStandaloneProject` and adds it immediately.

2. **`refreshAfterConfigChangeAsync()`** — New async variant that saves config synchronously but runs the heavy re-discovery (git worktree detection, file scanning, watcher updates) in a background goroutine. The HTTP response returns immediately.

3. **Data race fix** — The async goroutine acquires `cfgMu` during `discoverAllProjects()` to avoid racing with handlers that mutate `s.cfg`.

4. **Stale config rollback** — If `LoadStandaloneProject` fails, the just-appended `ProjectConfig` entry is removed instead of persisting to disk.

5. **Deduplication** — Extracted `applyDiscoveredProjects()` so the sync and async refresh paths share logic instead of drifting.

### Before → After

| | Before | After |
|---|--------|-------|
| `/api/open` latency | **Hangs** (~100 git subprocesses) | **< 1 second** (single project load) |
| Background refresh | Blocking in handler | Async goroutine |
| Config on failure | Stale entry persisted | Rolled back |
