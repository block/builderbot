---
scope: Engineering requirements — technical design, interfaces, and implementation constraints. Derived from product requirements.
see-also:
  - PRODUCT.md — product requirements that drive the technical decisions in this document.
  - TESTING.md — testing strategy covering these requirements.
  - DEPENDENCIES.md — external dependencies that the system cannot supply itself.
---

# Engineering Requirements

## Architecture

- <a id="E-PENPAL-ARCH"></a>**E-PENPAL-ARCH**: Penpal runs as three processes: a Rust/Tauri shell (desktop wrapper), a Go HTTP server (core runtime), and a React SPA (frontend). The Go server is the source of truth for all data; the SPA communicates via REST API; AI agents communicate via MCP.
  ← [P-PENPAL-RENDER](PRODUCT.md#P-PENPAL-RENDER), [P-PENPAL-MCP](PRODUCT.md#P-PENPAL-MCP)

- <a id="E-PENPAL-TAURI"></a>**E-PENPAL-TAURI**: The Tauri shell spawns the Go server as a sidecar process and polls `GET /api/ready` (up to 30s) before showing the webview. On exit, the sidecar is killed. On macOS dock-click, the main window is re-created without relaunching the sidecar.
  ← [P-PENPAL-RENDER](PRODUCT.md#P-PENPAL-RENDER)

- <a id="E-PENPAL-CORS"></a>**E-PENPAL-CORS**: CORS allows only `tauri://`, `https://tauri.*`, `http://localhost*`, and `http://127.0.0.1*` origins. All other origins receive no CORS headers.
  ← [P-PENPAL-RENDER](PRODUCT.md#P-PENPAL-RENDER)

- <a id="E-PENPAL-LOCAL-ONLY"></a>**E-PENPAL-LOCAL-ONLY**: The server binds to `127.0.0.1:{port}` only. There is no authentication layer; trust is based on local process access.
  ← [P-PENPAL-RENDER](PRODUCT.md#P-PENPAL-RENDER)

---

## Configuration & Storage

- <a id="E-PENPAL-CONFIG"></a>**E-PENPAL-CONFIG**: Configuration is stored at `~/.config/penpal/config.json`. Writes are atomic (write `.tmp`, rename). Contains workspaces, standalone projects, per-project source overrides, and the remembered claude binary path.
  ← [P-PENPAL-WORKSPACE](PRODUCT.md#P-PENPAL-WORKSPACE), [P-PENPAL-STANDALONE](PRODUCT.md#P-PENPAL-STANDALONE)

- <a id="E-PENPAL-PORT-FILE"></a>**E-PENPAL-PORT-FILE**: The server writes its port to `~/.config/penpal/server.port` on startup and removes it on shutdown. The CLI reads this file to locate a running server.
  ← [P-PENPAL-CLI-OPEN](PRODUCT.md#P-PENPAL-CLI-OPEN)

- <a id="E-PENPAL-COMMENT-STORAGE"></a>**E-PENPAL-COMMENT-STORAGE**: Comment threads are stored as JSON sidecar files at `{project}/.penpal/comments/{relative-path}.json`. Writes are atomic (temp file + rename). Path traversal is prevented by verifying the resolved path stays within the comments directory.
  ← [P-PENPAL-SELECT-COMMENT](PRODUCT.md#P-PENPAL-SELECT-COMMENT), [P-PENPAL-REPLY](PRODUCT.md#P-PENPAL-REPLY)

- <a id="E-PENPAL-COMMENT-WORKTREE"></a>**E-PENPAL-COMMENT-WORKTREE**: For named worktrees, comment sidecars are stored within the worktree's filesystem path, not the main worktree. This ensures full comment isolation between worktrees.
  ← [P-PENPAL-WORKTREE](PRODUCT.md#P-PENPAL-WORKTREE)

- <a id="E-PENPAL-GITIGNORE"></a>**E-PENPAL-GITIGNORE**: On startup, `config.EnsureGlobalGitignore()` ensures `.penpal/` is listed in the global gitignore so sidecar files are never committed.
  ← [P-PENPAL-SELECT-COMMENT](PRODUCT.md#P-PENPAL-SELECT-COMMENT)

- <a id="E-PENPAL-MCP-JSON"></a>**E-PENPAL-MCP-JSON**: On startup, the server writes `.mcp.json` to CWD so MCP clients (Claude Code) can auto-discover the server.
  ← [P-PENPAL-MCP](PRODUCT.md#P-PENPAL-MCP)

---

## Project Discovery

- <a id="E-PENPAL-DISCOVERY"></a>**E-PENPAL-DISCOVERY**: `DiscoverWorkspace()` reads all non-hidden immediate subdirectories as projects, calls `DetectSources()` on each, then `DiscoverWorktrees()`. Deduplicates projects that are git worktrees of each other.
  ← [P-PENPAL-WORKSPACE](PRODUCT.md#P-PENPAL-WORKSPACE), [P-PENPAL-DEDUP](PRODUCT.md#P-PENPAL-DEDUP)

- <a id="E-PENPAL-SOURCE-REGISTRY"></a>**E-PENPAL-SOURCE-REGISTRY**: Source types are registered in a pluggable `SourceType` registry via `init()`. Each `SourceType` entry defines: `AutoDetectDir` or `AutoDetectFile` (trigger), `DetectAtWSRoot` (workspace-level detection), `ScanMode` ("tree" or "files"), `SkipDirs`, `ClassifyFile()` (returns type string or `""` to hide), and optional `GroupFiles()` (returns named sections). `DetectSources()` iterates all registered types and checks for triggers at the project root.
  ← [P-PENPAL-SRC-DETECT](PRODUCT.md#P-PENPAL-SRC-DETECT), [P-PENPAL-SRC-CLASSIFY](PRODUCT.md#P-PENPAL-SRC-CLASSIFY), [P-PENPAL-SRC-GROUP](PRODUCT.md#P-PENPAL-SRC-GROUP), [P-PENPAL-SRC-SKIP](PRODUCT.md#P-PENPAL-SRC-SKIP)

- <a id="E-PENPAL-SRC-THOUGHTS"></a>**E-PENPAL-SRC-THOUGHTS**: The `thoughts` source type auto-detects `thoughts/` directory. `DetectAtWSRoot: true` enables workspace-root detection. Classifies files as `research`, `plan`, or `other`. No custom `GroupFiles` — files appear in a single flat group named after the source. No `SkipDirs`.
  ← [P-PENPAL-SRC-THOUGHTS](PRODUCT.md#P-PENPAL-SRC-THOUGHTS), [P-PENPAL-SRC-THOUGHTS-WSROOT](PRODUCT.md#P-PENPAL-SRC-THOUGHTS-WSROOT)

- <a id="E-PENPAL-SRC-RP1"></a>**E-PENPAL-SRC-RP1**: The `rp1` source type auto-detects `.rp1/` directory. `ClassifyFile()` maps path prefixes to types (context/ → knowledge, work/prds/ → prd, work/features/{id}/ → requirement/design/task/etc., work/issues/{id}/ → investigation/analysis/etc.). Returns `""` for `work/archives/`, `work/worktrees/`, `work/notes/` to hide them. `GroupFiles()` organizes files into fixed-order sections (Blueprint, Quick Builds, Research, Reviews, Content, Other) with dynamic Feature: and Issue: groups sorted alphabetically.
  ← [P-PENPAL-SRC-RP1](PRODUCT.md#P-PENPAL-SRC-RP1), [P-PENPAL-SRC-RP1-CLASSIFY](PRODUCT.md#P-PENPAL-SRC-RP1-CLASSIFY), [P-PENPAL-SRC-RP1-GROUP](PRODUCT.md#P-PENPAL-SRC-RP1-GROUP)

- <a id="E-PENPAL-SRC-ANCHORS"></a>**E-PENPAL-SRC-ANCHORS**: The `anchors` source type auto-detects `ANCHORS.md` file at project root. `ScanMode: "tree"`. `SkipDirs`: `.git`, `node_modules`, `.hg`, `.svn`. `ClassifyFile()` returns a type only for the five recognized filenames (`ANCHORS.md` → anchors, `PRODUCT.md` → product, `ERD.md` → engineering, `TESTING.md` → testing, `DEPENDENCIES.md` → dependencies); all others return `""`. `GroupFiles()` groups by module directory (any directory containing `ANCHORS.md`), root module shown as "(root)", modules sorted alphabetically, files within each module in canonical order (ANCHORS → PRODUCT → ERD → TESTING → DEPENDENCIES). Stray files without a sibling `ANCHORS.md` are dropped.
  ← [P-PENPAL-SRC-ANCHORS](PRODUCT.md#P-PENPAL-SRC-ANCHORS), [P-PENPAL-SRC-ANCHORS-GROUP](PRODUCT.md#P-PENPAL-SRC-ANCHORS-GROUP), [P-PENPAL-SRC-ANCHORS-NESTED](PRODUCT.md#P-PENPAL-SRC-ANCHORS-NESTED)

- <a id="E-PENPAL-SRC-CLAUDE-PLANS"></a>**E-PENPAL-SRC-CLAUDE-PLANS**: The `claude-plans` source type classifies all files as `plan`. No custom grouping. Injected via `DiscoverClaudePlans()` which creates a synthetic standalone project or injects a tree source into an existing manually-added project.
  ← [P-PENPAL-SRC-CLAUDE-PLANS](PRODUCT.md#P-PENPAL-SRC-CLAUDE-PLANS), [P-PENPAL-CLAUDE-PLANS](PRODUCT.md#P-PENPAL-CLAUDE-PLANS)

- <a id="E-PENPAL-SRC-MANUAL"></a>**E-PENPAL-SRC-MANUAL**: The `manual` source type is used for user-added sources. Directory sources create "tree" type entries; individual file sources create "files" type entries. `GroupFiles()` generates directory headings (`Dir`, `ShowDir` fields on `FileInfo`) for subdirectory boundaries. Configuration is persisted in `config.json` under `ProjectSources` (workspace projects) or inline with the `ProjectConfig` entry (standalone projects).
  ← [P-PENPAL-SRC-MANUAL](PRODUCT.md#P-PENPAL-SRC-MANUAL), [P-PENPAL-ADD-SOURCE](PRODUCT.md#P-PENPAL-ADD-SOURCE)

- <a id="E-PENPAL-WORKTREE-DISCOVERY"></a>**E-PENPAL-WORKTREE-DISCOVERY**: Worktrees are discovered by parsing `git worktree list --porcelain` output. Each worktree gets a name, path, branch, and `IsMain` flag. The `refs/heads/` prefix is stripped from branch names.
  ← [P-PENPAL-WORKTREE](PRODUCT.md#P-PENPAL-WORKTREE)

- <a id="E-PENPAL-CLAUDE-PLANS-DETECT"></a>**E-PENPAL-CLAUDE-PLANS-DETECT**: `DiscoverClaudePlans()` checks `~/.claude/plans/` for existence and at least one `.md` file. If found, a synthetic standalone project is injected. If the user already manually added the same path, a tree source is injected into the existing entry instead of duplicating.
  ← [P-PENPAL-CLAUDE-PLANS](PRODUCT.md#P-PENPAL-CLAUDE-PLANS)

---

## Cache

- <a id="E-PENPAL-CACHE"></a>**E-PENPAL-CACHE**: An in-memory cache (`sync.RWMutex`-protected) holds the full project list and per-project file lists. `RefreshProject()` walks the filesystem; `RefreshAllProjects()` runs in parallel with no concurrency limit. `RescanWith()` replaces the project list while preserving git enrichment.
  ← [P-PENPAL-FILE-LIST](PRODUCT.md#P-PENPAL-FILE-LIST)

- <a id="E-PENPAL-SCAN"></a>**E-PENPAL-SCAN**: `scanProjectSources()` walks `RootPath` recursively for tree sources, skipping `.git`-file directories (nested worktrees), source-type `SkipDirs`, and non-`.md` files. Files returning `""` from `ClassifyFile()` are hidden. Files are de-duplicated by project-relative path (first source wins) and sorted by `ModTime` descending.
  ← [P-PENPAL-FILE-LIST](PRODUCT.md#P-PENPAL-FILE-LIST), [P-PENPAL-FILE-TYPES](PRODUCT.md#P-PENPAL-FILE-TYPES), [P-PENPAL-SRC-DEDUP](PRODUCT.md#P-PENPAL-SRC-DEDUP)

- <a id="E-PENPAL-TITLE-EXTRACT"></a>**E-PENPAL-TITLE-EXTRACT**: `EnrichTitles()` reads the first 20 lines of each file to extract H1 headings. Titles are cached and shown as the primary display name when present.
  ← [P-PENPAL-FILE-LIST](PRODUCT.md#P-PENPAL-FILE-LIST)

- <a id="E-PENPAL-PATH-MATCH"></a>**E-PENPAL-PATH-MATCH**: `FindProjectByPath()` uses longest-prefix matching across all project root paths. `FindProjectByPathWithWorktree()` extends this to check non-main worktree paths and return the worktree name.
  ← [P-PENPAL-CLI-OPEN](PRODUCT.md#P-PENPAL-CLI-OPEN), [P-PENPAL-MCP](PRODUCT.md#P-PENPAL-MCP)

---

## Git Integration

- <a id="E-PENPAL-GIT-ENRICH"></a>**E-PENPAL-GIT-ENRICH**: Git enrichment runs in background after initial fast load (up to 8 concurrent goroutines). Branch name, dirty status, and unpushed commit count are read via `git` CLI calls. An SSE event pushes updates when complete.
  ← [P-PENPAL-GIT-INFO](PRODUCT.md#P-PENPAL-GIT-INFO)

---

## Anchor System

- <a id="E-PENPAL-ANCHOR-STRUCT"></a>**E-PENPAL-ANCHOR-STRUCT**: An `Anchor` contains: `SelectedText`, `Before` (~80 chars context), `After` (~80 chars context), `HeadingPath`, `StartLine` (1-indexed), `OccurrenceIndex`, and optional `SvgSnippet`/`SvgRect` for diagram selections. Anchors are immutable once created.
  ← [P-PENPAL-ANCHOR](PRODUCT.md#P-PENPAL-ANCHOR)

- <a id="E-PENPAL-ANCHOR-RESOLVE"></a>**E-PENPAL-ANCHOR-RESOLVE**: `ResolveAnchorsToLines()` maps threads to line numbers. Primary: use `StartLine` directly. Fallback: `ResolveAnchor()` does text matching — single match uses `strings.Index`; multiple matches disambiguate with `Before`/`After` context; no match returns -1 (orphaned).
  ← [P-PENPAL-ANCHOR-RESOLVE](PRODUCT.md#P-PENPAL-ANCHOR-RESOLVE), [P-PENPAL-ORPHANED](PRODUCT.md#P-PENPAL-ORPHANED)

- <a id="E-PENPAL-ANCHOR-COMPUTE"></a>**E-PENPAL-ANCHOR-COMPUTE**: Frontend `computeAnchor()` walks up the DOM to find `data-source-line`, computes `occurrenceIndex` within the block, extracts ±80 chars of raw markdown as `Before`/`After`, and walks backwards through DOM siblings to build `headingPath`.
  ← [P-PENPAL-SELECT-COMMENT](PRODUCT.md#P-PENPAL-SELECT-COMMENT), [P-PENPAL-ANCHOR](PRODUCT.md#P-PENPAL-ANCHOR)

- <a id="E-PENPAL-HIGHLIGHT-REHYPE"></a>**E-PENPAL-HIGHLIGHT-REHYPE**: A rehype plugin (`rehypeCommentHighlights`) injects `<mark>` elements into the rendered AST by matching `SelectedText` at the correct `startLine` and `occurrenceIndex`. Pending (unsaved) anchors get a `.pending` CSS class.
  ← [P-PENPAL-HIGHLIGHT](PRODUCT.md#P-PENPAL-HIGHLIGHT)

---

## Mermaid Diagram Anchoring

- <a id="E-PENPAL-SVG-DRAG"></a>**E-PENPAL-SVG-DRAG**: `MermaidSelection` handles drag on `.mermaid-container` elements. After a 5px movement threshold, a live `.penpal-pending-svg-highlight` rect tracks the selection. On mouseup, SVG coordinates are computed.
  ← [P-PENPAL-DIAGRAM-SELECT](PRODUCT.md#P-PENPAL-DIAGRAM-SELECT)

- <a id="E-PENPAL-SVG-EXTRACT"></a>**E-PENPAL-SVG-EXTRACT**: The SVG snippet is extracted by cloning the SVG, setting a cropped `viewBox`, scaling to max 300x200px, and re-IDing all elements with a random prefix to prevent DOM ID collisions. CSS `url(#id)` and `href="#id"` references are rewritten.
  ← [P-PENPAL-SVG-PREVIEW](PRODUCT.md#P-PENPAL-SVG-PREVIEW)

- <a id="E-PENPAL-SVG-STARTLINE"></a>**E-PENPAL-SVG-STARTLINE**: `startLine` for mermaid anchors is computed by counting ` ```mermaid` fence openings in the raw markdown to match the nth container.
  ← [P-PENPAL-DIAGRAM-SELECT](PRODUCT.md#P-PENPAL-DIAGRAM-SELECT)

---

## Comment Thread Operations

- <a id="E-PENPAL-THREAD-MODEL"></a>**E-PENPAL-THREAD-MODEL**: A `Thread` has ID, Status (`"open"`/`"resolved"`), Anchor, Comments, CreatedAt, ResolvedAt, ResolvedBy. A `Comment` has ID, Author, Role (`"human"`/`"agent"`), Body, CreatedAt, SuggestedReplies, InReplyTo.
  ← [P-PENPAL-THREAD-STATES](PRODUCT.md#P-PENPAL-THREAD-STATES), [P-PENPAL-REPLY](PRODUCT.md#P-PENPAL-REPLY)

- <a id="E-PENPAL-THREAD-MUTEX"></a>**E-PENPAL-THREAD-MUTEX**: All comment mutations are serialized per-project via `sync.Mutex` to prevent concurrent write corruption.
  ← [P-PENPAL-REPLY](PRODUCT.md#P-PENPAL-REPLY)

- <a id="E-PENPAL-INREPLYTO"></a>**E-PENPAL-INREPLYTO**: `AddComment()` sets `InReplyTo` to the previous comment's ID. `migrateInReplyTo()` backfills missing `InReplyTo` fields in legacy data on save.
  ← [P-PENPAL-REPLY](PRODUCT.md#P-PENPAL-REPLY)

- <a id="E-PENPAL-COMMENT-ORDER"></a>**E-PENPAL-COMMENT-ORDER**: `OrderComments()` arranges comments in tree order: root comments sorted by time, replies grouped under their parents, siblings sorted by time. Missing parents fall back to root level.
  ← [P-PENPAL-THREAD-PANEL](PRODUCT.md#P-PENPAL-THREAD-PANEL)

- <a id="E-PENPAL-CHANGE-SEQ"></a>**E-PENPAL-CHANGE-SEQ**: A global monotonic sequence number increments on every comment change. `WaitForChangeSince(ctx, sinceSeq)` blocks on a channel until `changeSeq` advances or context cancels.
  ← [P-PENPAL-WAIT-CHANGES](PRODUCT.md#P-PENPAL-WAIT-CHANGES)

---

## Working & Heartbeat Indicators

- <a id="E-PENPAL-WORKING"></a>**E-PENPAL-WORKING**: An in-memory `working` map (keyed by `"project:path:threadID"`) tracks which threads an agent is actively processing. Entries expire after 60s. `SetWorking()`/`ClearWorking()` trigger SSE `comments` events.
  ← [P-PENPAL-WORKING](PRODUCT.md#P-PENPAL-WORKING)

- <a id="E-PENPAL-HEARTBEAT"></a>**E-PENPAL-HEARTBEAT**: An in-memory `heartbeats` map (keyed by `"project:filePath"`) records agent activity. `IsAgentActive()` returns true if heartbeat is <60s old. MCP tool calls record heartbeats.
  ← [P-PENPAL-AGENT-PRESENCE](PRODUCT.md#P-PENPAL-AGENT-PRESENCE)

---

## MCP Server

- <a id="E-PENPAL-MCP-TRANSPORT"></a>**E-PENPAL-MCP-TRANSPORT**: The MCP server uses Streamable HTTP transport (`mcp.NewStreamableHTTPHandler`) at `/mcp` and `/mcp/`. All tool responses are JSON-encoded.
  ← [P-PENPAL-MCP](PRODUCT.md#P-PENPAL-MCP)

- <a id="E-PENPAL-MCP-TOOLS"></a>**E-PENPAL-MCP-TOOLS**: Registered tools: `penpal_find_project` (maps CWD to project), `penpal_list_threads` (by file or project-wide), `penpal_read_thread`, `penpal_reply` (agent role, clears working), `penpal_create_thread` (computes Before/After/StartLine from disk), `penpal_files_in_review` (enriched with threads and oldest pending), `penpal_wait_for_changes` (30s long-poll).
  ← [P-PENPAL-MCP](PRODUCT.md#P-PENPAL-MCP), [P-PENPAL-WAIT-CHANGES](PRODUCT.md#P-PENPAL-WAIT-CHANGES)

- <a id="E-PENPAL-MCP-WORKING"></a>**E-PENPAL-MCP-WORKING**: `penpal_list_threads`, `penpal_read_thread`, and `penpal_files_in_review` automatically set the `working` indicator for threads where the last comment is from a human. `penpal_reply` clears the indicator. `penpal_wait_for_changes` refreshes working timestamps during its 30s cycle to prevent expiry.
  ← [P-PENPAL-WORKING](PRODUCT.md#P-PENPAL-WORKING)

---

## Agent Management

- <a id="E-PENPAL-AGENT-SPAWN"></a>**E-PENPAL-AGENT-SPAWN**: `agents.Manager.Start()` writes a temporary MCP config pointing at the local server, builds a prompt, and runs `claude -p {prompt} --mcp-config {file} --dangerously-skip-permissions --output-format stream-json --max-budget-usd 5 --model opus` with CWD set to the project path. Agent log goes to `{project}/.penpal/agent.log`.
  ← [P-PENPAL-AGENT-LAUNCH](PRODUCT.md#P-PENPAL-AGENT-LAUNCH)

- <a id="E-PENPAL-AGENT-STREAM"></a>**E-PENPAL-AGENT-STREAM**: Agent stdout is parsed as NDJSON. `type: "assistant"` messages provide `contextUsed` (sum of input + cache tokens) and `numTurns`. `type: "result"` provides `totalCostUSD`, `numTurns`, and `contextWindow`.
  ← [P-PENPAL-AGENT-STATUS](PRODUCT.md#P-PENPAL-AGENT-STATUS)

- <a id="E-PENPAL-AGENT-PROMPT"></a>**E-PENPAL-AGENT-PROMPT**: The agent prompt instructs it to: call `penpal_files_in_review` first, reply to the `oldestPending` thread, then enter a long-poll loop via `penpal_wait_for_changes`. Exit condition: 10 consecutive timeouts with no files in review (~5 minutes idle).
  ← [P-PENPAL-AGENT-LAUNCH](PRODUCT.md#P-PENPAL-AGENT-LAUNCH)

- <a id="E-PENPAL-AGENT-DETECT"></a>**E-PENPAL-AGENT-DETECT**: Background polling every 5 seconds runs `ps -eo pid,args` to find processes ending with `/claude`, then `lsof -a -p {pid} -d cwd -Fn` to determine CWD. CWD is mapped to a project via `FindProjectByPath()`.
  ← [P-PENPAL-AGENT-PRESENCE](PRODUCT.md#P-PENPAL-AGENT-PRESENCE)

- <a id="E-PENPAL-AGENT-AUTOSTART"></a>**E-PENPAL-AGENT-AUTOSTART**: `maybeStartAgent()` is called after `handleCreateThread` and `handleAddComment`. If the new comment's Role is `"human"` and no agent is running, one is started.
  ← [P-PENPAL-AGENT-LAUNCH](PRODUCT.md#P-PENPAL-AGENT-LAUNCH)

- <a id="E-PENPAL-AGENT-CLEANUP"></a>**E-PENPAL-AGENT-CLEANUP**: On agent exit: temp MCP config is removed, project heartbeats and working indicators are cleared, and the `onChange` callback fires an SSE event.
  ← [P-PENPAL-AGENT-LAUNCH](PRODUCT.md#P-PENPAL-AGENT-LAUNCH)

---

## HTTP API

- <a id="E-PENPAL-API-ROUTES"></a>**E-PENPAL-API-ROUTES**: The server exposes REST endpoints: projects CRUD, project files (grouped), recent files, in-review, search, workspaces, sources, open/navigate, threads CRUD, reviews, focus, agents start/stop/status, raw file, view tracking, publish, publish-state, ready, install-tools, claude-path. SPA served from `/app/`. MCP at `/mcp`.
  ← [P-PENPAL-RENDER](PRODUCT.md#P-PENPAL-RENDER), [P-PENPAL-MCP](PRODUCT.md#P-PENPAL-MCP)

- <a id="E-PENPAL-LAZY-INIT"></a>**E-PENPAL-LAZY-INIT**: First HTTP request triggers `sync.Once` that discovers projects, starts the watcher, then runs `populateProjects()` in a background goroutine. `populateProjects()` refreshes the cache, seeds activity, closes `readyCh`, broadcasts events, then enriches git info.
  ← [P-PENPAL-WORKSPACE](PRODUCT.md#P-PENPAL-WORKSPACE)

- <a id="E-PENPAL-PATH-TRAVERSAL"></a>**E-PENPAL-PATH-TRAVERSAL**: Path traversal is prevented on comment storage paths, raw file paths, and source-add paths by verifying `filepath.Abs()` results stay within their respective base directories via `isSubpath()`.
  ← [P-PENPAL-SELECT-COMMENT](PRODUCT.md#P-PENPAL-SELECT-COMMENT), [P-PENPAL-FILE-ACTIONS](PRODUCT.md#P-PENPAL-FILE-ACTIONS)

- <a id="E-PENPAL-SPA-SERVE"></a>**E-PENPAL-SPA-SERVE**: The SPA is served from `frontend/dist/` at `/app/`. Unknown routes fall back to `index.html` for client-side routing. `/app` redirects to `/app/` (301). Path traversal is blocked.
  ← [P-PENPAL-RENDER](PRODUCT.md#P-PENPAL-RENDER)

---

## SSE & File Watching

- <a id="E-PENPAL-SSE"></a>**E-PENPAL-SSE**: `GET /events` is a long-lived SSE stream using `event: change` messages. Event types: `projects`, `files`, `comments`, `agents`, `navigate`. Each event carries optional `project`, `path`, `worktree` fields.
  ← [P-PENPAL-REALTIME](PRODUCT.md#P-PENPAL-REALTIME)

- <a id="E-PENPAL-WATCHER"></a>**E-PENPAL-WATCHER**: The file watcher bridges `fsnotify` to SSE. Two-tier watch strategy: base (shallow workspace + project root directories) and dynamic (deep, per-focus). Debounce at 100ms per event key.
  ← [P-PENPAL-REALTIME](PRODUCT.md#P-PENPAL-REALTIME), [P-PENPAL-LIVE-UPDATE](PRODUCT.md#P-PENPAL-LIVE-UPDATE)

- <a id="E-PENPAL-FOCUS"></a>**E-PENPAL-FOCUS**: `windowFocuses map[string]focusTarget` (one entry per browser window) drives dynamic watches. Union of all window focuses determines the watched set. File focus watches only the file's parent directory. Project focus watches all source directories + `.penpal/comments/`.
  ← [P-PENPAL-FOCUS](PRODUCT.md#P-PENPAL-FOCUS)

- <a id="E-PENPAL-SSE-RECONNECT"></a>**E-PENPAL-SSE-RECONNECT**: Frontend `useSSE` hook reconnects after 2s on error. On `visibilitychange`: closes when tab hidden, reconnects when visible. `onReconnect` callback re-fetches data and polls `/api/navigate` for missed navigations.
  ← [P-PENPAL-REALTIME](PRODUCT.md#P-PENPAL-REALTIME)

---

## Frontend

- <a id="E-PENPAL-FRONTEND-STACK"></a>**E-PENPAL-FRONTEND-STACK**: React 19 + TypeScript + Vite + Tailwind v4. Router: react-router-dom v7 (browser router with `/app/` basename in production). Markdown: react-markdown v10 with remark-gfm, rehype-raw. Diagrams: mermaid v11. Desktop: Tauri v2.
  ← [P-PENPAL-RENDER](PRODUCT.md#P-PENPAL-RENDER)

- <a id="E-PENPAL-TABS"></a>**E-PENPAL-TABS**: `useTabs` hook maintains per-tab history stacks. `PUSH` navigation truncates forward history. `REPLACE` replaces current entry. `POP` events (browser back/forward) are matched against tab history. Each tab derives its title from the current URL path.
  ← [P-PENPAL-TABS](PRODUCT.md#P-PENPAL-TABS)

- <a id="E-PENPAL-WINDOW-ID"></a>**E-PENPAL-WINDOW-ID**: Each browser window gets a unique ID: in browser mode via `sessionStorage` (UUID), in desktop mode via Tauri window label. Sent as `?window=` param on all `/api/focus` calls.
  ← [P-PENPAL-FOCUS](PRODUCT.md#P-PENPAL-FOCUS)

- <a id="E-PENPAL-MD-RENDER"></a>**E-PENPAL-MD-RENDER**: Each rendered block is tagged with `data-source-line` (1-indexed). Heading IDs use the same slugification algorithm as Go's goldmark renderer. Mermaid blocks produce `.mermaid-container` divs with `data-mermaid-source`.
  ← [P-PENPAL-GFM](PRODUCT.md#P-PENPAL-GFM), [P-PENPAL-MERMAID](PRODUCT.md#P-PENPAL-MERMAID)

- <a id="E-PENPAL-FIND-BAR"></a>**E-PENPAL-FIND-BAR**: The Find bar uses the CSS Custom Highlight API (`CSS.highlights`) for non-destructive highlighting. Case-insensitive substring matching via TreeWalker over `.main-content`. Two highlight groups: `find-matches` (all) and `find-active` (current).
  ← [P-PENPAL-FIND](PRODUCT.md#P-PENPAL-FIND)

---

## Search

- <a id="E-PENPAL-SEARCH"></a>**E-PENPAL-SEARCH**: Search matches project names (substring), filenames, and file content (case-insensitive line scan). Results capped at 100 files. Name matches sort before content matches within a project. Files not recognized by a source type's classifier are excluded.
  ← [P-PENPAL-SEARCH](PRODUCT.md#P-PENPAL-SEARCH)

---

## Activity Tracking

- <a id="E-PENPAL-ACTIVITY"></a>**E-PENPAL-ACTIVITY**: In-memory `activity.Tracker` stores one event per file (keyed by project + path). Event types: `viewed`, `modified`, `created`, `comment`, `published`. `Record()` always overwrites; `RecordAt()` (for seeding from mtime) does not overwrite.
  ← [P-PENPAL-RECENT](PRODUCT.md#P-PENPAL-RECENT)

---

## Publishing

- <a id="E-PENPAL-PUBLISH-RENDER"></a>**E-PENPAL-PUBLISH-RENDER**: `RenderHTML()` strips frontmatter, renders markdown via goldmark with syntax highlighting, extracts headings for TOC, and produces a self-contained HTML page with embedded mermaid.min.js, copy-markdown button, and raw markdown in a `<template>` element.
  ← [P-PENPAL-PUBLISH](PRODUCT.md#P-PENPAL-PUBLISH)

- <a id="E-PENPAL-PUBLISH-UPLOAD"></a>**E-PENPAL-PUBLISH-UPLOAD**: HTML is zipped and uploaded via multipart POST to `https://blockcell.sqprod.co/api/v1/sites/{siteName}/upload`. Site name is deterministic: `penpal-{project-slug}-{path-slug}`, max 63 chars.
  ← [P-PENPAL-PUBLISH](PRODUCT.md#P-PENPAL-PUBLISH)

- <a id="E-PENPAL-PUBLISH-STATE"></a>**E-PENPAL-PUBLISH-STATE**: Publish state is stored at `{project}/.penpal/publish.json` as a map of filePath to `{SiteName, URL, LastPublished}`. Protected by a package-level `sync.Mutex`.
  ← [P-PENPAL-PUBLISH-STATE](PRODUCT.md#P-PENPAL-PUBLISH-STATE)

---

## CLI

- <a id="E-PENPAL-CLI"></a>**E-PENPAL-CLI**: `penpal open <path>` reads the port file, checks server health, launches the app if not running (polling up to 10s), then calls `POST /api/open` for each path. The server resolves directories to projects (longest-prefix match) and files to their containing project. Navigation is handed off via `pendingNav` + SSE `navigate` event.
  ← [P-PENPAL-CLI-OPEN](PRODUCT.md#P-PENPAL-CLI-OPEN)

---

## Source Management

- <a id="E-PENPAL-ADD-SOURCE"></a>**E-PENPAL-ADD-SOURCE**: `POST /api/sources` accepts a relative path. Directories create "tree" sources; `.md` files are added to a "files" source. Duplicate detection refuses paths already covered by an existing source. Only `.md` files accepted for individual file sources.
  ← [P-PENPAL-ADD-SOURCE](PRODUCT.md#P-PENPAL-ADD-SOURCE)

- <a id="E-PENPAL-REMOVE-SOURCE"></a>**E-PENPAL-REMOVE-SOURCE**: `DELETE /api/sources` removes user-added sources. Auto-detected sources cannot be removed. For "files" sources, individual files can be removed; the source entry is deleted when empty.
  ← [P-PENPAL-REMOVE-SOURCE](PRODUCT.md#P-PENPAL-REMOVE-SOURCE)

---

## Install Tools

- <a id="E-PENPAL-INSTALL-CLI"></a>**E-PENPAL-INSTALL-CLI**: CLI install creates a symlink at `$(brew --prefix)/bin/penpal` pointing to `<app>/Contents/MacOS/penpal-cli`. Falls back to `/usr/local/bin` if Homebrew is not found.
  ← [P-PENPAL-INSTALL](PRODUCT.md#P-PENPAL-INSTALL)

- <a id="E-PENPAL-INSTALL-PLUGIN"></a>**E-PENPAL-INSTALL-PLUGIN**: Plugin install runs `claude plugin marketplace add <resources-dir>` then `claude plugin install penpal`. The claude binary path is resolved from config, PATH, or well-known locations.
  ← [P-PENPAL-INSTALL](PRODUCT.md#P-PENPAL-INSTALL)

- <a id="E-PENPAL-INSTALL-DISMISS"></a>**E-PENPAL-INSTALL-DISMISS**: The install modal dismiss flag is keyed to `BUILD_ID`, so it reappears on every new build until tools are installed and current.
  ← [P-PENPAL-INSTALL](PRODUCT.md#P-PENPAL-INSTALL)

---

## Open Questions

(none)

## Resolved Questions
