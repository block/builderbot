---
scope: Automated testing strategy — test pyramid, tooling, and coverage targets.
see-also:
  - PRODUCT.md — product requirements that define acceptance criteria.
  - ERD.md — technical requirements and interfaces under test.
---

# Testing Strategy

## Test Pyramid

```
         ┌─────────┐
         │   E2E   │  Playwright (Chromium), real Go server + Vite dev server
         ├─────────┤
         │  Integ  │  Go httptest (in-process), real filesystem (t.TempDir())
         ├─────────┤
         │  Unit   │  Go testing, Vitest + testing-library/react (jsdom)
         └─────────┘
```

### Unit Tests (Go)

- **Framework:** Standard `testing` package, no third-party test frameworks.
- **File I/O:** Real filesystem via `t.TempDir()` — no mocking of file operations.
- **Scope:** Individual modules in isolation: comments store, cache, discovery, config, activity, agents stream parser, publish rendering, path utilities, claudepath resolution.

### Unit Tests (Frontend)

- **Framework:** Vitest v4 with `globals: true`, `environment: jsdom`.
- **Rendering:** `@testing-library/react` v16 with `render`, `screen`, `fireEvent`, `waitFor`, `act`.
- **Mocking:** All API calls mocked via `vi.mock('../api')`. `useSSE` always mocked. `localStorage` polyfilled in-memory.
- **Scope:** Hooks (useSSE, useTheme, useTabs, useProjectSort), utilities (comment ordering, time formatting, rehype plugin), all page components, all UI components.

### Integration Tests (Go)

- **Framework:** `net/http/httptest` in-process servers, same package access to unexported types.
- **Test helper:** `testServer(t)` constructs the full server stack (cache, activity, watcher, comments store, server). `seedProject()` populates the cache.
- **Scope:** HTTP API endpoints exercised end-to-end through the handler chain: projects, threads, agents, focus, workspaces, sources, open/navigate, file operations, install tools, SPA serving.
- **MCP integration tests:** Real in-process HTTP test server with actual `go-sdk/mcp` client. Full JSON-RPC roundtrip through the Streamable HTTP transport.

### End-to-End Tests

- **Framework:** Playwright v1.58, Chromium only, no retries.
- **Architecture:** Real compiled Go server at port 18923 + Vite dev server at 18924. Isolated config via `PENPAL_CONFIG` in `os.tmpdir()`.
- **Helper:** Custom `MCPClient` class for JSON-RPC 2.0 over HTTP, managing `mcp-session-id` headers and handling both JSON and SSE response formats.
- **Scope:** Navigation, SPA routing, comments API, CLI open flow, view tracking, tab navigation with keyboard shortcuts, full review workflow (select text → create thread → MCP agent reply → working indicators → suggested replies), mermaid diagram commenting.

## CI Pipeline

- **Entry:** `just ci` chains `fmt-check`, `lint` (Rust clippy), `typecheck` (TypeScript tsc), `test` (`go test ./...` + `vitest run`).
- **E2E:** Separate `just test-e2e` command, not part of standard CI. Requires the built binary.
- **Go linting:** `go vet ./...` in the `lint` recipe.
- **Frontend typecheck:** `tsc --noEmit` in the `typecheck` recipe.

## Coverage Mapping

| Requirement Area | Unit (Go) | Unit (Frontend) | Integration (Go) | E2E |
|---|---|---|---|---|
| Project Discovery (P-PENPAL-WORKSPACE, AUTO-DETECT, DEDUP) | discovery_test.go | — | api_projects_test.go | navigation.spec.ts |
| Source Types — General (P-PENPAL-SRC-DETECT, SRC-CLASSIFY, SRC-GROUP) | discovery_test.go | — | integration_test.go (buildFileGroups) | — |
| Source Types — thoughts (P-PENPAL-SRC-THOUGHTS) | discovery_test.go (classify) | — | grouping_test.go (TestBuildFileGroups_ThoughtsFlat) | — |
| Source Types — rp1 (P-PENPAL-SRC-RP1, SRC-RP1-CLASSIFY, SRC-RP1-GROUP) | discovery_test.go (TestClassifyRP1File, TestGroupRP1Paths) | — | grouping_test.go (TestBuildFileGroups_RP1Grouped) | — |
| Source Types — anchors (P-PENPAL-SRC-ANCHORS, SRC-ANCHORS-GROUP) | discovery_test.go (TestClassifyAnchorsFile, TestGroupAnchorsPaths) | — | — | — |
| Source Types — claude-plans (P-PENPAL-SRC-CLAUDE-PLANS) | — | — | — | — |
| Source Types — manual (P-PENPAL-SRC-MANUAL) | — | — | grouping_test.go (TestBuildFileGroups_ManualSourceDirHeadings) | — |
| Cache & File Scanning (E-PENPAL-CACHE, SCAN) | cache_test.go | — | — | — |
| Worktree Support (P-PENPAL-WORKTREE) | discovery/worktree_test.go, cache/worktree_test.go | Layout.test.tsx | worktree_test.go (API + MCP) | — |
| Worktree Dropdown (P-PENPAL-PROJECT-WORKTREE-DROPDOWN) | — | Layout.test.tsx | — | — |
| Git Integration (P-PENPAL-GIT-INFO) | — | — | — | — |
| File List & Grouping (P-PENPAL-FILE-LIST) | — | ProjectPage.test.tsx | grouping_test.go, integration_test.go | — |
| Markdown Rendering (P-PENPAL-GFM, MERMAID) | — | MarkdownViewer.test.tsx | — | mermaid-comments.spec.ts |
| Text Selection & Anchors (P-PENPAL-SELECT-COMMENT, ANCHOR) | — | SelectionToolbar.test.tsx | — | review-workflow.spec.ts |
| Anchor Resolution (P-PENPAL-ANCHOR-RESOLVE) | comments_test.go (implicit via round-trip) | rehypeCommentHighlights.test.ts | — | — |
| Comment Highlights (P-PENPAL-HIGHLIGHT) | — | rehypeCommentHighlights.test.ts, MarkdownViewer.test.tsx | — | review-workflow.spec.ts |
| Mermaid Diagram Comments (P-PENPAL-DIAGRAM-SELECT) | — | — | — | mermaid-comments.spec.ts |
| Comment Threads (P-PENPAL-THREAD-PANEL, REPLY, STATES) | comments_test.go | CommentsPanel.test.tsx | api_threads_test.go | review-workflow.spec.ts |
| Comment Ordering (E-PENPAL-COMMENT-ORDER) | ordering_test.go | utils/comments.test.ts | — | — |
| InReplyTo Migration (E-PENPAL-INREPLYTO) | comments_test.go (4 migration tests) | — | — | — |
| Working & Heartbeat (P-PENPAL-WORKING) | — | CommentsPanel.test.tsx | mcpserver/tools_test.go | review-workflow.spec.ts |
| Suggested Replies (P-PENPAL-SUGGESTED-REPLIES) | — | CommentsPanel.test.tsx | — | review-workflow.spec.ts |
| MCP Tools (P-PENPAL-MCP) | — | — | mcpserver/tools_test.go, transport_test.go, worktree_test.go | review-workflow.spec.ts |
| Wait for Changes (P-PENPAL-WAIT-CHANGES) | — | — | tools_test.go (TestWaitForChanges_Triggered) | — |
| Agent Management (P-PENPAL-AGENT-LAUNCH, STATUS) | stream_test.go | FilePage.test.tsx (auto-start) | api_agents_test.go | — |
| Agent Detection (P-PENPAL-AGENT-PRESENCE) | — | — | — | — |
| Review Workflow (P-PENPAL-IN-REVIEW) | — | InReviewPage.test.tsx | api_projects_test.go (TestAPIInReview) | — |
| Publishing (P-PENPAL-PUBLISH) | blockcell_test.go, render_test.go, state_test.go | — | — | — |
| Tabs (P-PENPAL-TABS) | — | useTabs.test.ts, Layout.test.tsx | — | tab-navigation.spec.ts |
| Search (P-PENPAL-SEARCH) | — | SearchPage.test.tsx | — | react-app.spec.ts |
| Recent Files (P-PENPAL-RECENT) | activity_test.go | RecentPage.test.tsx | integration_test.go | — |
| CLI Open (P-PENPAL-CLI-OPEN) | — | — | api_manage_test.go | cli-open.spec.ts |
| Source Management (P-PENPAL-ADD-SOURCE, REMOVE-SOURCE) | — | — | api_manage_test.go | — |
| Real-Time Updates (P-PENPAL-REALTIME, FOCUS) | watcher_test.go | useSSE.test.ts | api_focus_test.go | — |
| Config & Migration (E-PENPAL-CONFIG) | config_test.go, migrate_test.go | — | — | — |
| Install Tools (P-PENPAL-INSTALL) | — | InstallStartup.test.tsx, InstallToolsModal.test.tsx | install_test.go | — |
| Theme (P-PENPAL-THEME) | — | useTheme.test.ts | — | — |
| Sidebar Resize (P-PENPAL-SIDEBAR-RESIZE) | — | Layout.test.tsx | — | — |
| SPA Serving (E-PENPAL-SPA-SERVE) | — | — | spa_test.go | react-app.spec.ts |
| Path Traversal (E-PENPAL-PATH-TRAVERSAL) | — | — | pathutil_test.go, spa_test.go | — |
| View Tracking (E-PENPAL-ACTIVITY) | — | — | — | view-tracking.spec.ts |
| File Handler (P-PENPAL-FILE-HANDLER) | — | — | api_manage_test.go | — |
| Source Disambiguation (P-PENPAL-SRC-DISAMBIG) | — | Layout.test.tsx | — | — |
| Home Label (P-PENPAL-HOME-LABEL) | — | Layout.test.tsx | — | — |
| File View Margins (P-PENPAL-VIEW-MARGINS) | — | file-view-layout.test.ts | — | — |
| View Options Panel (P-PENPAL-VIEW-OPTIONS) | — | Layout.test.tsx | — | — |
| View Options — Sort (P-PENPAL-VIEW-OPTIONS-SORT) | — | useProjectSort.test.ts, Layout.test.tsx | — | — |
| View Options — Empty Projects (P-PENPAL-VIEW-OPTIONS-EMPTY) | — | useProjectSort.test.ts, Layout.test.tsx | — | — |
| Sort Persistence (P-PENPAL-SORT) | — | useProjectSort.test.ts | — | — |

## Known Coverage Gaps

- **SSE `/events` endpoint:** No Go unit tests for the streaming mechanism itself; only tested indirectly via `penpal_wait_for_changes`.
- **Agent lifecycle:** `agents.Manager.Start()`, kill, and log file writing are not unit-tested (only stream parsing and status reporting are).
- **Agent detection (ps/lsof):** Not tested due to dependency on external processes.
- **Git CLI calls:** Not tested at the unit level; would require injecting a git interface.
- **SelectionToolbar:** Only one backwards-selection edge case is tested; forward selection, multi-line selection, and copy-markdown are not covered.
- **MermaidRenderer:** Mocked in component tests; only e2e tests exercise real SVG rendering.
- **Publish end-to-end:** No e2e test covers the full publish flow through the UI.
- **Workspace filesystem scan:** Integration tests seed the cache directly rather than scanning real project directories.

## Test Patterns

- **Table-driven tests (Go):** Used extensively in discovery, cache, path traversal, and comment ordering tests.
- **Real filesystem (Go):** `t.TempDir()` everywhere — no file system mocks.
- **Atomic write verification:** Config and publish state tests verify no `.tmp` files remain.
- **Concurrent access tests:** Activity tracker tested with 50 concurrent writers + 50 readers.
- **API mocking (Frontend):** `vi.mock('../api')` in every component test isolates from the backend.
- **SSE mocking (Frontend):** Custom `MockEventSource` class tracks instances and simulates events.
- **MCP client (E2E):** Hand-rolled JSON-RPC 2.0 HTTP client handles both JSON and SSE response formats.
