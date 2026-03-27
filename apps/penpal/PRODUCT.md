---
scope: Product requirements — user-facing behavior, workflows, and experience.
see-also:
  - ERD.md — technical requirements derived from this document.
  - TESTING.md — testing strategy covering these requirements.
  - DEPENDENCIES.md — external dependencies the system cannot supply itself.
---

# Product Requirements

## Overview

Penpal is a desktop application and local web server for collaborative review of markdown documentation. It auto-discovers projects, renders markdown with full comment threading, and enables AI agents to participate in document review alongside humans. The core workflow: humans write markdown, open it in Penpal, select text to start review threads, and AI agents monitor and respond to those threads in real time.

---

## Project Discovery & Workspace Management

- <a id="P-PENPAL-WORKSPACE"></a>**P-PENPAL-WORKSPACE**: Users can register workspace directories. Every immediate non-hidden subdirectory of a workspace is shown as a project in the sidebar, with no manual configuration needed.

- <a id="P-PENPAL-AUTO-DETECT"></a>**P-PENPAL-AUTO-DETECT**: Projects are automatically scanned for recognized source types. Each detected source type gets a colored badge and determines which files are shown, how they are classified, and how they are grouped. See [Source Types](#source-types) for the full list.

- <a id="P-PENPAL-STANDALONE"></a>**P-PENPAL-STANDALONE**: Users can add standalone projects (directories or individual files) outside of any workspace, via the sidebar "Add" button or the `penpal open` CLI command.

- <a id="P-PENPAL-WORKTREE"></a>**P-PENPAL-WORKTREE**: Git worktrees for a project are discovered and shown as navigable sub-items in the sidebar. Each worktree has its own branch name and independent comment storage.

- <a id="P-PENPAL-DEDUP"></a>**P-PENPAL-DEDUP**: When multiple directories in a workspace share the same git repository (one is a worktree of the other), only the main worktree is shown as a project to avoid duplicates.

- <a id="P-PENPAL-GIT-INFO"></a>**P-PENPAL-GIT-INFO**: Project cards show the current git branch name and whether there are uncommitted changes (dirty status).

- <a id="P-PENPAL-WS-ROOT"></a>**P-PENPAL-WS-ROOT**: If the workspace directory itself has a `thoughts/` directory at its root, a synthetic "(root)" project is created.

- <a id="P-PENPAL-CLAUDE-PLANS"></a>**P-PENPAL-CLAUDE-PLANS**: If `~/.claude/plans/` exists and contains markdown files, it automatically appears as a standalone project called ".claude/plans".

- <a id="P-PENPAL-REMOVE-WORKSPACE"></a>**P-PENPAL-REMOVE-WORKSPACE**: Users can remove a workspace via a three-dot menu on the workspace sidebar entry. Removing a workspace takes it out of Penpal's view without deleting any files.

- <a id="P-PENPAL-CLOSE-PROJECT"></a>**P-PENPAL-CLOSE-PROJECT**: Users can close a standalone project via the sidebar or workspace page three-dot menu. Closing removes it from Penpal's view without deleting files.

- <a id="P-PENPAL-DELETE-PROJECT"></a>**P-PENPAL-DELETE-PROJECT**: Users can delete a workspace project from disk via the workspace page. A confirmation modal shows the file count, git dirty status, and unpushed commit count before deletion.

- <a id="P-PENPAL-PROJECT-CARD"></a>**P-PENPAL-PROJECT-CARD**: Project cards on the workspace page show: name, source type badges, branch name, relative age, review count badge (when files are in review), agent dot (when an agent is active), and worktree count (when additional worktrees exist). Cards also have a three-dot menu with copy-path and delete/close actions.

- <a id="P-PENPAL-STANDALONE-SECTION"></a>**P-PENPAL-STANDALONE-SECTION**: The workspace page shows a "Standalone Projects" section listing any standalone projects, regardless of which workspace is being viewed.

- <a id="P-PENPAL-HOME-REDIRECT"></a>**P-PENPAL-HOME-REDIRECT**: When navigating to the root URL, the app redirects to the most recently modified workspace project, or the most recently modified standalone project, or the Recent page if nothing is configured.

---

## Source Types

Source types are the pluggable system that determines how projects discover, classify, and organize their markdown files. Each source type defines its own auto-detection trigger, file classification rules, grouping logic, and which directories or files to skip.

### General Behavior

- <a id="P-PENPAL-SRC-DETECT"></a>**P-PENPAL-SRC-DETECT**: Each source type specifies how it is auto-detected — either by a directory name (e.g., `thoughts/`) or a file name (e.g., `ANCHORS.md`) at the project root. When the trigger is found, the source is activated automatically.

- <a id="P-PENPAL-SRC-CLASSIFY"></a>**P-PENPAL-SRC-CLASSIFY**: Each source type defines file classification rules that assign a type label (e.g., research, plan, prd, knowledge) to files based on their path within the source. The type label determines the badge shown next to the filename. Files that the source type does not recognize are hidden (not shown in the file list).

- <a id="P-PENPAL-SRC-GROUP"></a>**P-PENPAL-SRC-GROUP**: Each source type can define grouping logic to organize files into named sections with headers on the project page. Source types without custom grouping show files in a single flat list under the source name.

- <a id="P-PENPAL-SRC-BADGE"></a>**P-PENPAL-SRC-BADGE**: Each source type has a display name and badge color shown in the project page source header and on project cards (e.g., "RPI" grey, "RP1" purple, "ANCHORS" teal).

- <a id="P-PENPAL-SRC-SKIP"></a>**P-PENPAL-SRC-SKIP**: Source types can define directories to skip during scanning. Skipped directories and their contents are completely ignored.

- <a id="P-PENPAL-SRC-DEDUP"></a>**P-PENPAL-SRC-DEDUP**: When multiple sources in a project cover overlapping paths, files are de-duplicated by project-relative path. The first source (in the order sources appear on the project page) wins — a file that appears in an earlier source is not shown again in a later source.

### thoughts Source Type

- <a id="P-PENPAL-SRC-THOUGHTS"></a>**P-PENPAL-SRC-THOUGHTS**: Auto-detected by a `thoughts/` directory at the project root. Shows a grey "RPI" badge. Files are shown in a single flat list under the source name. Files whose path contains "research" are classified as `research`; files whose path contains "plan" are classified as `plan`; all others are classified as `other`. The first matching rule wins.

- <a id="P-PENPAL-SRC-THOUGHTS-WSROOT"></a>**P-PENPAL-SRC-THOUGHTS-WSROOT**: The thoughts source type can also be detected at the workspace root level. If the workspace directory itself contains a `thoughts/` directory, a synthetic "(root)" project is created for it.

### rp1 Source Type

- <a id="P-PENPAL-SRC-RP1"></a>**P-PENPAL-SRC-RP1**: Auto-detected by a `.rp1/` directory at the project root. Shows a purple "RP1" badge. Provides rich file classification and structured grouping.

- <a id="P-PENPAL-SRC-RP1-CLASSIFY"></a>**P-PENPAL-SRC-RP1-CLASSIFY**: Files are classified by path prefix within the `.rp1/` directory:
  - `context/` → knowledge
  - `work/prds/` → prd; `work/charter.md` → charter
  - `work/quick-builds/` → quick
  - `work/research/` → research
  - `work/pr-reviews/` → review
  - `work/content/` → content
  - `work/features/{id}/`: `requirements.md` → requirement, `design.md` → design, `tasks.md` → task, `field-notes.md` → field-notes, `hypotheses.md` → hypothesis, `test_report.md` → test-report, `verification-report.md` → verification
  - `work/issues/{id}/`: `investigation_report.md` → investigation, `root_cause_analysis.md` → analysis, `implementation_plan.md` → plan, `evidence/` → evidence
  - `work/*.md` matching known report names (e.g., `audit-report.md`, `investigation-report.md`) → report
  - `work/archives/`, `work/worktrees/`, `work/notes/` → hidden (not shown)

- <a id="P-PENPAL-SRC-RP1-GROUP"></a>**P-PENPAL-SRC-RP1-GROUP**: Files are grouped into named sections displayed in fixed order: Blueprint, Quick Builds, Research, Reviews, Content, Other, then Issues ("Issue: {id}"), then Features ("Feature: {id}"), then Context. Issues and Features are each sorted alphabetically.

### anchors Source Type

- <a id="P-PENPAL-SRC-ANCHORS"></a>**P-PENPAL-SRC-ANCHORS**: Auto-detected by an `ANCHORS.md` file at the project root. Shows a teal "ANCHORS" badge. Scans the full project tree but only shows the five recognized ANCHORS filenames: `ANCHORS.md`, `PRODUCT.md`, `ERD.md`, `TESTING.md`, `DEPENDENCIES.md`. All other files are hidden.

- <a id="P-PENPAL-SRC-ANCHORS-GROUP"></a>**P-PENPAL-SRC-ANCHORS-GROUP**: Files are grouped by module directory — a subdirectory that contains its own `ANCHORS.md`. The root module is shown as "(root)". Modules are sorted alphabetically. Within each module, files are sorted in canonical order: ANCHORS → PRODUCT → ERD → TESTING → DEPENDENCIES.

- <a id="P-PENPAL-SRC-ANCHORS-NESTED"></a>**P-PENPAL-SRC-ANCHORS-NESTED**: Supports nested modules in monorepos. Stray ANCHORS document files (e.g., a `PRODUCT.md` in a directory without a sibling `ANCHORS.md`) are excluded from the file list.

### claude-plans Source Type

- <a id="P-PENPAL-SRC-CLAUDE-PLANS"></a>**P-PENPAL-SRC-CLAUDE-PLANS**: Auto-detected by the presence of `~/.claude/plans/` containing at least one `.md` file. All files are classified as type `plan`. Files are shown in a single flat list. This source type is injected into a synthetic standalone project rather than being detected within an existing project.

### manual Source Type

- <a id="P-PENPAL-SRC-MANUAL"></a>**P-PENPAL-SRC-MANUAL**: Represents user-added sources (directories or individual files). Not auto-detected — created when a user adds a source via the "Add to project" UI or `penpal open`. Shows directory headings for subdirectory boundaries within the source, so files are visually organized by their parent directory.

---

## File Browsing

- <a id="P-PENPAL-FILE-LIST"></a>**P-PENPAL-FILE-LIST**: Navigating into a project shows a grouped list of markdown files organized by source. Each file shows its type badge, modification age, and an action menu. When a file has an H1 heading, it is shown as the primary label with the filename as a subtitle.

- <a id="P-PENPAL-FILE-TYPES"></a>**P-PENPAL-FILE-TYPES**: Files are classified by type (research, plan, knowledge, prd, design, task, etc.) based on their path within a source. Type badges appear next to the filename.

- <a id="P-PENPAL-IN-REVIEW-SECTION"></a>**P-PENPAL-IN-REVIEW-SECTION**: Files with open comment threads appear in an "In Review" section at the top of the project page, with an indicator when an agent is actively working.

- <a id="P-PENPAL-FILE-ACTIONS"></a>**P-PENPAL-FILE-ACTIONS**: Each file has an action menu with: copy markdown, copy relative path (with `@` prefix), copy absolute path, publish to Blockcell, remove from Penpal, and delete from disk. In the file viewer toolbar, "copy file" places the file on the clipboard as a file reference, so pasting in Finder or other apps inserts the file itself (macOS only).

- <a id="P-PENPAL-SOURCE-ACTIONS"></a>**P-PENPAL-SOURCE-ACTIONS**: Each source group header on the project page has a three-dot menu with: copy relative paths, copy absolute paths, publish all files, remove from Penpal (non-auto sources only), and delete from disk. Auto-detected sources show an "(auto)" label in the header.

- <a id="P-PENPAL-DELETE-FILE"></a>**P-PENPAL-DELETE-FILE**: Files and source groups can be deleted from disk via the action menu. A confirmation modal prevents accidental deletion. When a file is deleted, its associated comments are also deleted, and empty parent directories are cleaned up.

- <a id="P-PENPAL-BATCH-OPS"></a>**P-PENPAL-BATCH-OPS**: Users can select multiple files via checkboxes and perform batch operations: copy markdown (concatenated), copy paths, publish, or delete.

- <a id="P-PENPAL-SORT"></a>**P-PENPAL-SORT**: The workspace page supports toggling between alphabetical and recent-modification sort order. Projects with zero recognized files always sort last.

---

## Markdown Viewer

- <a id="P-PENPAL-RENDER"></a>**P-PENPAL-RENDER**: Clicking a file opens a two-pane view: rendered markdown on the left, comments panel on the right, with a draggable divider.

- <a id="P-PENPAL-GFM"></a>**P-PENPAL-GFM**: Markdown rendering supports GitHub Flavored Markdown: tables, task lists, strikethrough, autolinks, and syntax-highlighted code blocks with a dark color scheme (for fenced blocks with a language specifier).

- <a id="P-PENPAL-FRONTMATTER"></a>**P-PENPAL-FRONTMATTER**: YAML/TOML frontmatter is stripped from the rendered output so users see only the document content.

- <a id="P-PENPAL-MERMAID"></a>**P-PENPAL-MERMAID**: Mermaid diagram blocks are rendered as interactive diagrams within the document.

- <a id="P-PENPAL-TOC"></a>**P-PENPAL-TOC**: A table of contents derived from h1/h2/h3 headings is shown in the sidebar under "On this page" when viewing a file. Clicking a heading scrolls to it.

- <a id="P-PENPAL-LIVE-UPDATE"></a>**P-PENPAL-LIVE-UPDATE**: The rendered document updates automatically when the underlying file changes on disk, without manual refresh. Rapid successive file changes produce a single smooth update rather than visible flickering.

---

## Text-Based Comment Creation

- <a id="P-PENPAL-SELECT-COMMENT"></a>**P-PENPAL-SELECT-COMMENT**: Selecting text in a rendered document shows a floating toolbar with "Comment" and "Copy markdown" buttons. Clicking "Comment" opens a new thread form anchored to the selected text.

- <a id="P-PENPAL-COPY-MD-SELECTION"></a>**P-PENPAL-COPY-MD-SELECTION**: The "Copy markdown" button copies the complete markdown source lines that contain the selected rendered text to the clipboard. If the selection spans part of a line, the full source line is included.

- <a id="P-PENPAL-ANCHOR"></a>**P-PENPAL-ANCHOR**: Comment anchors are bound to the selected text in the document. Anchors remain correctly positioned when text is added, removed, or rearranged elsewhere in the document. An anchor becomes orphaned only when its specific anchored text is deleted or substantially rewritten.

- <a id="P-PENPAL-ANCHOR-RESOLVE"></a>**P-PENPAL-ANCHOR-RESOLVE**: Anchors track their position as the document changes. Threads are displayed sorted by their anchor's line number in the document.

- <a id="P-PENPAL-ORPHANED"></a>**P-PENPAL-ORPHANED**: When anchor text is no longer found in the document, the thread is shown as orphaned with a warning message.

- <a id="P-PENPAL-HIGHLIGHT"></a>**P-PENPAL-HIGHLIGHT**: Anchored text is highlighted in the rendered document. Clicking a thread card scrolls to and briefly activates the corresponding highlight.

---

## Mermaid Diagram Comments

- <a id="P-PENPAL-DIAGRAM-SELECT"></a>**P-PENPAL-DIAGRAM-SELECT**: Users can drag-select a rectangular region on any mermaid diagram to anchor a comment to that visual area.

- <a id="P-PENPAL-SVG-PREVIEW"></a>**P-PENPAL-SVG-PREVIEW**: The new-thread form shows a cropped preview of the selected diagram region.

- <a id="P-PENPAL-SVG-HIGHLIGHT"></a>**P-PENPAL-SVG-HIGHLIGHT**: Existing diagram-anchored threads show highlight rectangles on the live diagram. Clicking a thread card briefly activates the highlight.

---

## Comment Threads

- <a id="P-PENPAL-THREAD-PANEL"></a>**P-PENPAL-THREAD-PANEL**: The right-side panel shows all threads for the current file, sorted by anchor line number.

- <a id="P-PENPAL-THREAD-STATES"></a>**P-PENPAL-THREAD-STATES**: Threads have open and resolved states. Users can resolve and reopen threads. Resolved threads are hidden by default and shown via a toggle.

- <a id="P-PENPAL-REPLY"></a>**P-PENPAL-REPLY**: Users can reply to any thread. Each comment shows author, role badge (human or agent), and relative timestamp. When a comment is a non-sequential reply, an "in reply to @author" marker is shown.

- <a id="P-PENPAL-COMMENT-MD"></a>**P-PENPAL-COMMENT-MD**: Comment and reply bodies are rendered as GFM markdown, not plain text.

- <a id="P-PENPAL-COMMENT-KEYS"></a>**P-PENPAL-COMMENT-KEYS**: In the new-thread form and reply form, Cmd+Enter (or Ctrl+Enter) submits the form and Escape cancels it.

- <a id="P-PENPAL-AUTHOR-PERSIST"></a>**P-PENPAL-AUTHOR-PERSIST**: The author name is persisted locally so users don't re-enter it on every comment.

- <a id="P-PENPAL-SUGGESTED-REPLIES"></a>**P-PENPAL-SUGGESTED-REPLIES**: When the last comment in a thread is from an agent and includes suggested replies, clickable pill buttons appear. Clicking a pill submits that text as a human reply.

- <a id="P-PENPAL-WORKING"></a>**P-PENPAL-WORKING**: A pulsing dot animation shows when an agent has read a thread and is composing a reply. The dot appears after the specific comment the agent is responding to, not at the end of the thread. If a human adds a new comment while the agent is working, the dot stays in place. When the agent replies, its response is ordered after the comment it was replying to (before any comments added while it was working).

---

## AI Agent Collaboration

- <a id="P-PENPAL-MCP"></a>**P-PENPAL-MCP**: AI agents can participate in document review alongside humans. Agents can discover projects, read comment threads, post replies, create new threads, query which files are in review, and react to changes in real time.

- <a id="P-PENPAL-AGENT-LAUNCH"></a>**P-PENPAL-AGENT-LAUNCH**: Each project can have at most one agent running at a time. An agent is automatically launched when a human creates a comment and no agent is running for that project. If the human comments on another file in the same project, the existing agent automatically picks up the new comment. Users can also manually start and stop agents. Each agent run has a $5 USD budget cap. Agents self-terminate after approximately 5 minutes idle with no files in review.

- <a id="P-PENPAL-AGENT-STATUS"></a>**P-PENPAL-AGENT-STATUS**: When an agent is running, the UI shows a colored progress bar indicating how much of the agent's capacity has been used, cost in USD, and a stop button.

- <a id="P-PENPAL-AGENT-PRESENCE"></a>**P-PENPAL-AGENT-PRESENCE**: Running agents are automatically detected and mapped to their projects. Visual indicators (dots) appear on project cards and in the sidebar.

- <a id="P-PENPAL-WAIT-CHANGES"></a>**P-PENPAL-WAIT-CHANGES**: Agents respond to new comments and thread changes in near real-time, waiting idle until a human posts a comment.

---

## Review Workflow

- <a id="P-PENPAL-IN-REVIEW"></a>**P-PENPAL-IN-REVIEW**: The "In Review" page aggregates all files with open threads across all projects, grouped by workspace, project, and source. Each group shows a clickable breadcrumb path (workspace → project → source) for navigation.

- <a id="P-PENPAL-REVIEW-COUNT"></a>**P-PENPAL-REVIEW-COUNT**: The sidebar shows a count of all files currently in review across all projects.

---

## Publishing

- <a id="P-PENPAL-PUBLISH"></a>**P-PENPAL-PUBLISH**: Any markdown file can be published as a hosted web page (via the Blockcell service) — a self-contained HTML page with mermaid diagrams, syntax highlighting, and a table of contents. After publishing, a toast notification with a clickable URL appears briefly in the file viewer.

- <a id="P-PENPAL-PUBLISH-STATE"></a>**P-PENPAL-PUBLISH-STATE**: Publish state (site name, URL, timestamp) is persisted per file. Previously-published files show a "Copy Blockcell link" option in the action menu.

---

## Tab Navigation

- <a id="P-PENPAL-TABS"></a>**P-PENPAL-TABS**: The app has a browser-style tab bar. Multiple pages can be open simultaneously, each with independent back/forward history. Closing the last tab closes the window.

- <a id="P-PENPAL-TAB-KEYS"></a>**P-PENPAL-TAB-KEYS**: Keyboard shortcuts for tab management: Cmd+T (new tab), Cmd+W (close tab), Ctrl+Tab / Ctrl+Shift+Tab (next/previous tab). Back (Cmd+[) and forward (Cmd+]) navigate per-tab history. Middle-click on a tab closes it.

- <a id="P-PENPAL-CMD-CLICK"></a>**P-PENPAL-CMD-CLICK**: Cmd+Click on links opens in a new background tab (current tab stays active). Cmd+Shift+Click opens in a new window.

---

## Search

- <a id="P-PENPAL-SEARCH"></a>**P-PENPAL-SEARCH**: A search bar searches across all projects' markdown files as the user types, matching project names, filenames, and file content (case-insensitive). Results are capped at 100 files; when more matches exist, a message indicates results are truncated. Matching projects appear in a separate "Projects" section. Files that matched by name show a "name" badge distinct from content matches.

---

## Recent Files

- <a id="P-PENPAL-RECENT"></a>**P-PENPAL-RECENT**: The "Recent" page shows up to 50 recently active files across all projects, sorted by most recent activity first. Each entry shows an activity type label (viewed, modified, created, comment, published) and a relative timestamp.

---

## CLI

- <a id="P-PENPAL-CLI-OPEN"></a>**P-PENPAL-CLI-OPEN**: The `penpal open <path>...` command opens one or more files or directories in the Penpal app, launching the app if it's not running. Directories are resolved to their project; `.md` files are auto-added to their containing project if not already tracked (or a new standalone project is created). Non-`.md` files are rejected.

---

## Source Management

- <a id="P-PENPAL-ADD-SOURCE"></a>**P-PENPAL-ADD-SOURCE**: Users can add arbitrary directories or individual markdown files as sources to any project. Directories are scanned for all `.md` files; individual files are tracked directly.

- <a id="P-PENPAL-REMOVE-SOURCE"></a>**P-PENPAL-REMOVE-SOURCE**: User-added sources can be removed. Auto-detected sources (thoughts, rp1, anchors) cannot be removed.

---

## Sidebar Navigation

- <a id="P-PENPAL-SIDEBAR"></a>**P-PENPAL-SIDEBAR**: The sidebar shows workspaces, standalone projects, and global navigation links (In Review with count, Recent, Search). A "+ Add workspace or project" button opens a modal to register new paths.

- <a id="P-PENPAL-SIDEBAR-PROJECT"></a>**P-PENPAL-SIDEBAR-PROJECT**: When viewing a project or file, the sidebar switches to project mode showing a "← Home" back link, the workspace name (if applicable), worktree sub-items, and a "Sources" card listing each source as an in-page anchor link.

---

## Real-Time Updates

- <a id="P-PENPAL-REALTIME"></a>**P-PENPAL-REALTIME**: The UI updates automatically when files change, agents start or stop, comments are added, or navigation is triggered from the CLI. Updates resume seamlessly after a tab is hidden and made visible again, catching up on any missed events.

- <a id="P-PENPAL-FOCUS"></a>**P-PENPAL-FOCUS**: Live updates are scoped to what the user is viewing. Each window independently tracks which file or project is in focus, and updates are delivered for the focused content.

---

## Desktop App

- <a id="P-PENPAL-INSTALL"></a>**P-PENPAL-INSTALL**: On first launch, a modal prompts the user to install the `penpal` CLI (for command-line access) and the Claude Code plugin (which enables AI agents to participate in document review). The modal reappears after app updates until tools are current. If the install variant detects existing tools, it shows as "Update" instead of "Install".

- <a id="P-PENPAL-CLAUDE-PATH"></a>**P-PENPAL-CLAUDE-PATH**: If the `claude` binary cannot be found during install, the modal shows a text input for the user to manually provide the path. The path is validated and persisted for future use.

- <a id="P-PENPAL-NEW-WINDOW"></a>**P-PENPAL-NEW-WINDOW**: In the desktop app, Cmd+N opens a new window at the default route. The app stays running when all windows are closed (macOS dock behavior); clicking the dock icon reopens a window.

- <a id="P-PENPAL-FIND"></a>**P-PENPAL-FIND**: In the desktop app, Cmd+F opens a Find bar for in-page text search with match highlighting and navigation.

- <a id="P-PENPAL-THEME"></a>**P-PENPAL-THEME**: A toggle switches between light and dark color themes. The preference is persisted locally. On first launch, the theme defaults to the OS-level preference.

- <a id="P-PENPAL-EXTERNAL-LINKS"></a>**P-PENPAL-EXTERNAL-LINKS**: In the desktop app, external HTTP links open in the system browser.

---

## Open Questions

(none)

## Resolved Questions
