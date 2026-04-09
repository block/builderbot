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

- <a id="P-PENPAL-WORKSPACE"></a>**P-PENPAL-WORKSPACE**: Users can register workspace directories. Every immediate non-hidden subdirectory of a workspace is shown as a project, with no manual configuration needed.

- <a id="P-PENPAL-AUTO-DETECT"></a>**P-PENPAL-AUTO-DETECT**: Projects are automatically scanned for recognized source types. Each detected source type gets a colored badge and determines which files are shown, how they are classified, and how they are grouped. See [Source Types](#source-types) for the full list.

- <a id="P-PENPAL-STANDALONE"></a>**P-PENPAL-STANDALONE**: Users can add standalone projects (directories or individual files) outside of any workspace, via the home view "+" button or the `penpal open` CLI command.

- <a id="P-PENPAL-WORKTREE"></a>**P-PENPAL-WORKTREE**: Git worktrees for a project are discovered automatically. In the home view, multi-worktree projects expand to show each worktree as a child item with its branch name. In the project view, a worktree dropdown in the breadcrumb bar lets the user switch between worktrees. Each worktree has its own branch name and independent comment storage. When worktrees are added or removed (via `git worktree add`/`remove`), the worktree list updates without restarting the server.

- <a id="P-PENPAL-DEDUP"></a>**P-PENPAL-DEDUP**: When multiple directories in a workspace share the same git repository (one is a worktree of the other), only the main worktree is shown as a project to avoid duplicates.

- <a id="P-PENPAL-GIT-INFO"></a>**P-PENPAL-GIT-INFO**: In the home view, each project or worktree tree item shows its git branch as a tooltip prefixed with "branch: " (e.g. `branch: main`), not inline. In the project view, the worktree dropdown shows the current worktree name; the dropdown menu lists each worktree by name with a worktree icon (non-main only), matching the home tree format.

- <a id="P-PENPAL-WS-ROOT"></a>**P-PENPAL-WS-ROOT**: If the workspace directory itself has a `thoughts/` directory at its root, a synthetic "(root)" project is created.

- <a id="P-PENPAL-CLAUDE-PLANS"></a>**P-PENPAL-CLAUDE-PLANS**: If `~/.claude/plans/` exists and contains markdown files, it automatically appears as a standalone project called ".claude/plans".

- <a id="P-PENPAL-REMOVE-WORKSPACE"></a>**P-PENPAL-REMOVE-WORKSPACE**: Users can remove a workspace via a right-click context menu on the workspace tree item. Removing a workspace takes it out of Penpal's view without deleting any files.

- <a id="P-PENPAL-CLOSE-PROJECT"></a>**P-PENPAL-CLOSE-PROJECT**: Users can close a standalone project via a right-click context menu. Closing removes it from Penpal's view without deleting files.

- <a id="P-PENPAL-DELETE-PROJECT"></a>**P-PENPAL-DELETE-PROJECT**: Users can delete a project from disk via a context menu. A confirmation modal shows the file count, git dirty status, and unpushed commit count before deletion.

---

## Source Types

Source types are the pluggable system that determines how projects discover, classify, and organize their markdown files. Each source type defines its own auto-detection trigger, file classification rules, grouping logic, and which directories or files to skip.

### General Behavior

- <a id="P-PENPAL-SRC-DETECT"></a>**P-PENPAL-SRC-DETECT**: Each source type specifies how it is auto-detected — either by a directory name (e.g., `thoughts/`) or a file name (e.g., `ANCHORS.md`) at the project root. When the trigger is found, the source is activated automatically.

- <a id="P-PENPAL-SRC-CLASSIFY"></a>**P-PENPAL-SRC-CLASSIFY**: Each source type defines file classification rules that assign a type label (e.g., research, plan, prd, knowledge) to files based on their path within the source. The type label determines the badge shown next to the filename. Files that the source type does not recognize are hidden (not shown in the source's file tree).

- <a id="P-PENPAL-SRC-GROUP"></a>**P-PENPAL-SRC-GROUP**: Each source type can define grouping logic to organize files into named sections within the sidebar tree. Source types without custom grouping show files in a directory tree under the source header.

- <a id="P-PENPAL-SRC-BADGE"></a>**P-PENPAL-SRC-BADGE**: Each source type has a display name and badge color shown in the sidebar source header (e.g., "RPI" grey, "RP1" purple, "ANCHORS" teal).

- <a id="P-PENPAL-SRC-SKIP"></a>**P-PENPAL-SRC-SKIP**: Source types can define directories to skip during scanning. Skipped directories and their contents are completely ignored.

- <a id="P-PENPAL-SRC-GITIGNORE"></a>**P-PENPAL-SRC-GITIGNORE**: Scanning respects `.gitignore` rules. Directories that are gitignored are skipped during the walk, preventing build output, dependency caches, and other ignored content from appearing in the file tree. A registered source's own root path is always scanned even if it falls within a gitignored directory — explicit source configuration overrides gitignore.

- <a id="P-PENPAL-SRC-DEDUP"></a>**P-PENPAL-SRC-DEDUP**: When multiple sources in a project cover overlapping paths, files are de-duplicated by project-relative path. The first source (in the order sources appear in the sidebar) wins — a file that appears in an earlier source is not shown again in a later source.

### thoughts Source Type

- <a id="P-PENPAL-SRC-THOUGHTS"></a>**P-PENPAL-SRC-THOUGHTS**: Auto-detected by a `thoughts/` directory at the project root. Shows a grey "RPI" badge. Files are shown in a directory tree under the source header. Files whose path contains "research" are classified as `research`; files whose path contains "plan" are classified as `plan`; all others are classified as `other`. The first matching rule wins.

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

- <a id="P-PENPAL-SRC-ANCHORS"></a>**P-PENPAL-SRC-ANCHORS**: Auto-detected by an `ANCHORS.md` file at the project root. Shows a teal "ANCHORS" badge. Scans the full project tree but only includes files from directories that contain an `ANCHORS.md` sibling. Only four content filenames are shown: `PRODUCT.md`, `ERD.md`, `TESTING.md`, `DEPENDENCIES.md`. `ANCHORS.md` itself and all other files are hidden. A directory with only `ANCHORS.md` and no content files produces no visible output.

- <a id="P-PENPAL-SRC-ANCHORS-GROUP"></a>**P-PENPAL-SRC-ANCHORS-GROUP**: Files are grouped by module directory. The root module is shown as "(root)". Modules are sorted alphabetically. Within each module, files are sorted in canonical order: PRODUCT → ERD → TESTING → DEPENDENCIES.

- <a id="P-PENPAL-SRC-ANCHORS-NESTED"></a>**P-PENPAL-SRC-ANCHORS-NESTED**: Supports nested modules in monorepos. Stray ANCHORS document files (e.g., a `PRODUCT.md` in a directory without a sibling `ANCHORS.md`) are excluded by the sibling requirement at scan time.

### claude-plans Source Type

- <a id="P-PENPAL-SRC-CLAUDE-PLANS"></a>**P-PENPAL-SRC-CLAUDE-PLANS**: Auto-detected by the presence of `~/.claude/plans/` containing at least one `.md` file. All files are classified as type `plan`. Files are shown in a single flat list. This source type is injected into a synthetic standalone project rather than being detected within an existing project.

### All Markdown Virtual Source

- <a id="P-PENPAL-SRC-ALL-MD"></a>**P-PENPAL-SRC-ALL-MD**: Every project includes an "All Markdown" section in the sidebar that shows all `.md` files in the project directory tree, organized by directory structure. This is a virtual source — not auto-detected, always present. Files that also appear in a typed source (RPI, RP1, ANCHORS) are shown in both places. The All Markdown section has no badge and no file count deduplication against other sources.

### Source Disambiguation

- <a id="P-PENPAL-SRC-DISAMBIG"></a>**P-PENPAL-SRC-DISAMBIG**: When multiple sources share the same type badge in the sidebar (e.g., multiple Anchors modules), the source path is shown to the right of the badge label to disambiguate them. The path text is deemphasized and ellipsized if too long.

---

## General UX

- <a id="P-PENPAL-NO-SELECT-CHROME"></a>**P-PENPAL-NO-SELECT-CHROME**: Text in application chrome (sidebar, tab bar, topbar, breadcrumbs, menus) is not user-selectable. Only document content in the main viewer area is selectable.

- <a id="P-PENPAL-SIDEBAR-RESIZE"></a>**P-PENPAL-SIDEBAR-RESIZE**: The left sidebar is resizable via a drag handle on its right edge. The width range matches the right comments panel (200-700px). The user's preferred width is persisted across sessions. Works on all pages/views.

---

## Navigation — Home View

The home view is the top-level navigation state. It shows all workspaces and standalone projects in the sidebar.

- <a id="P-PENPAL-HOME"></a>**P-PENPAL-HOME**: The home view sidebar shows a flat list of workspaces, standalone projects, and global navigation links (In Review, Recent). A ⌂ icon header identifies the view. A "+" button allows adding new workspaces or standalone projects.

- <a id="P-PENPAL-HOME-LABEL"></a>**P-PENPAL-HOME-LABEL**: When on the home screen, the word "Home" appears next to the ⌂ icon at the top of the sidebar.

- <a id="P-PENPAL-HOME-TREE"></a>**P-PENPAL-HOME-TREE**: Workspaces are expandable tree items. Expanding a workspace reveals its child projects. Standalone projects appear as top-level peers of workspaces, not nested under any workspace. Visual dividers separate the workspaces section, standalone projects section, and global navigation section. Sections with no items are not shown (and their dividers are omitted). Global In Review and Recent links appear in the global navigation section. The In Review link shows a right-aligned count (matching the project view's source header count style) of all files currently in review across all projects; the link is dimmed when the count is zero.

- <a id="P-PENPAL-HOME-PROJECT-ITEM"></a>**P-PENPAL-HOME-PROJECT-ITEM**: Each project in the home tree shows: project name and agent dot (when an agent is active). No file counts or review counts are shown on individual project items. Git branch is available as a tooltip. Multi-worktree projects are expandable, showing each worktree as a child item. Non-main worktrees display a worktree icon; the main worktree does not. Git branch is shown as a tooltip on each worktree. The tree does not use vertical guide lines or dirty status indicators.

- <a id="P-PENPAL-HOME-NAVIGATE"></a>**P-PENPAL-HOME-NAVIGATE**: Clicking a project in the home tree navigates the current tab to the project view for that project. The sidebar switches to show the project's sources.

- <a id="P-PENPAL-VIEW-OPTIONS"></a>**P-PENPAL-VIEW-OPTIONS**: The home sidebar header includes a view options button (filter icon) just to the left of the "+" button. Clicking it opens a floating panel. The panel stays open while the user changes settings and dismisses only when clicking outside it (including clicking the view options button again).

- <a id="P-PENPAL-VIEW-OPTIONS-SORT"></a>**P-PENPAL-VIEW-OPTIONS-SORT**: The view options panel contains a "Project order" dropdown with two options: A->Z (default) and Most Recent. Selecting an option reorders the project list immediately.

- <a id="P-PENPAL-VIEW-OPTIONS-EMPTY"></a>**P-PENPAL-VIEW-OPTIONS-EMPTY**: The view options panel contains a "Show empty projects" toggle that defaults to on. When off, projects with zero recognized files are hidden entirely. When on, they appear sorted last and visually dimmed.

- <a id="P-PENPAL-SORT"></a>**P-PENPAL-SORT**: The selected sort order and show-empty preference persist in localStorage and sync across tabs/windows via storage events.

- <a id="P-PENPAL-HOME-DEFAULT"></a>**P-PENPAL-HOME-DEFAULT**: Opening a new tab (via + button or Cmd+T) or a new window with no target lands on the home view. The main content area shows a welcome message with the app name and a hint to select a project.

---

## Navigation — Project View

The project view shows the contents of a single project in the sidebar, organized by source.

- <a id="P-PENPAL-PROJECT-BREADCRUMB"></a>**P-PENPAL-PROJECT-BREADCRUMB**: A breadcrumb bar at the top of the sidebar shows the navigation path: ⌂ / workspace name / project name. The ⌂ icon navigates back to home. The project name is a single clickable segment (including workspace prefix when present) that navigates to the project view. For standalone projects without a workspace, the breadcrumb is ⌂ / project name. An agent dot appears in the breadcrumb when an agent is active for this project.

- <a id="P-PENPAL-PROJECT-WORKTREE-DROPDOWN"></a>**P-PENPAL-PROJECT-WORKTREE-DROPDOWN**: When a project has multiple worktrees, a worktree selector row spans the full width of the sidebar below the breadcrumb bar. It shows the current worktree: "main repo" when viewing the main worktree, or the worktree icon and worktree name when viewing a non-main worktree. Content is left-aligned. Clicking the selector shows all available worktrees; each item shows its name with a worktree icon (non-main only), matching the home tree format. Selecting a worktree switches to that worktree's view. For single-worktree projects (no additional worktrees), the row shows dimmed static text "no worktrees" (no dropdown).

- <a id="P-PENPAL-PROJECT-SOURCES"></a>**P-PENPAL-PROJECT-SOURCES**: The project sidebar shows each detected source type as a collapsible top-level section with its badge and file count. Sources appear in this order: typed sources (RPI, RP1, ANCHORS), then All Markdown, then In Review, then Recent. Each source expands to show its files in a tree view. Virtual sources with no files are shown with an alternate label and dimmed styling: "All Markdown" becomes "No Markdown Found", "In Review" becomes "Nothing in Review", "Recent" becomes "Nothing Recent". These empty virtual sources are not expandable.

- <a id="P-PENPAL-PROJECT-FILE-TREE"></a>**P-PENPAL-PROJECT-FILE-TREE**: Within each source section, files are organized in a tree view that mirrors the directory structure. Directories are collapsible. Single-child directory chains are compacted into one tree item with a combined path (e.g., `a/b/c/` instead of three nested levels), similar to IntelliJ's "compact middle packages" behavior. Files show their name (or H1 heading when available), type badge, and an "in review" badge when they have open threads.

- <a id="P-PENPAL-PROJECT-IN-REVIEW"></a>**P-PENPAL-PROJECT-IN-REVIEW**: Each project view includes a collapsible "In Review" section in the sidebar showing files with open comment threads in this project, with a count.

- <a id="P-PENPAL-PROJECT-RECENT"></a>**P-PENPAL-PROJECT-RECENT**: Each project view includes a collapsible "Recent" section in the sidebar showing recently accessed files in this project, with a count.

- <a id="P-PENPAL-PROJECT-BROWSE"></a>**P-PENPAL-PROJECT-BROWSE**: When viewing a project with no file open, the main content area shows a welcome message with the project name and a hint to expand a source in the sidebar.

---

## Navigation — Tabs & Windows

- <a id="P-PENPAL-TABS"></a>**P-PENPAL-TABS**: Each tab is a navigation context with its own back/forward history. The tab bar shows all open tabs, labeled by their current page (file name, project name, "Home", etc.). The close button (×) appears on each tab only when more than one tab is open. The sidebar follows the active tab's context: on the home view it shows the home tree; on a project view it shows the project's source tree; on a file view it shows the breadcrumb bar, worktree picker, and the file's table of contents (not the source tree).

- <a id="P-PENPAL-TAB-NAVIGATE"></a>**P-PENPAL-TAB-NAVIGATE**: Clicking a file in the sidebar navigates the current tab to that file. If no tabs are open, a new tab is created. Navigating within a tab builds per-tab back/forward history (including project-to-project and home-to-project transitions).

- <a id="P-PENPAL-TAB-NEW"></a>**P-PENPAL-TAB-NEW**: New tabs are created explicitly: via the + button in the tab bar, Cmd+T, or Cmd+Click on a navigable item. A new tab with no target opens on the home view. Cmd+Click opens the target in a new background tab (current tab stays active).

- <a id="P-PENPAL-TAB-KEYS"></a>**P-PENPAL-TAB-KEYS**: Keyboard shortcuts for tab management: Cmd+T (new tab on home), Cmd+W (close tab), Ctrl+Tab / Ctrl+Shift+Tab (next/previous tab). Back (Cmd+[) and forward (Cmd+]) navigate per-tab history. Middle-click on a tab closes it.

- <a id="P-PENPAL-WINDOW-LIFECYCLE"></a>**P-PENPAL-WINDOW-LIFECYCLE**: Closing the last tab in a window closes the window. Opening a new window (Cmd+N) creates a window with a single tab on the home view. The app stays running when all windows are closed (macOS dock behavior); clicking the dock icon reopens a window on home.

- <a id="P-PENPAL-CMD-CLICK"></a>**P-PENPAL-CMD-CLICK**: Cmd+Click on any navigable item (file, project) opens it in a new background tab. Cmd+Shift+Click opens in a new window.

---

## Navigation — Global Views

Global views aggregate content across all projects. They appear as top-level items in the home view sidebar, alongside workspaces and standalone projects.

- <a id="P-PENPAL-GLOBAL-IN-REVIEW"></a>**P-PENPAL-GLOBAL-IN-REVIEW**: The global In Review view shows all files with open comment threads across all projects. The sidebar shows a breadcrumb (⌂ / In Review) and a flat list of two-line file rows. Each row shows the filename on the first line and workspace / project / source path context on the second line.

- <a id="P-PENPAL-GLOBAL-RECENT"></a>**P-PENPAL-GLOBAL-RECENT**: The global Recent view shows up to 50 recently active files across all projects, sorted by most recent activity first. The sidebar shows a breadcrumb (⌂ / Recent) and two-line file rows with filename, context path, and relative timestamp. Each entry shows an activity type label (viewed, modified, created, comment, published). Activity history persists across app launches so that recent files are not lost when the server restarts.

- <a id="P-PENPAL-GLOBAL-ROW-NAVIGATE"></a>**P-PENPAL-GLOBAL-ROW-NAVIGATE**: Clicking a file in a global view navigates the current tab to that file and switches the sidebar to the file's project context. Cmd+Click opens in a new tab.

---

## File Actions

- <a id="P-PENPAL-FILE-TYPES"></a>**P-PENPAL-FILE-TYPES**: Files are classified by type (research, plan, knowledge, prd, design, task, etc.) based on their path within a source. Type badges appear next to the filename in the sidebar tree and file viewer.

- <a id="P-PENPAL-FILE-ACTIONS"></a>**P-PENPAL-FILE-ACTIONS**: Each file has a right-click context menu (in the sidebar tree or file viewer toolbar) with: copy markdown, copy relative path (with `@` prefix), copy absolute path, publish to Blockcell, remove from Penpal, and delete from disk. In the file viewer toolbar, "copy file" places the file on the clipboard as a file reference, so pasting in Finder or other apps inserts the file itself (macOS only).

- <a id="P-PENPAL-SOURCE-ACTIONS"></a>**P-PENPAL-SOURCE-ACTIONS**: Each source section header in the sidebar has a right-click context menu with: copy relative paths, copy absolute paths, publish all files, remove source from Penpal (non-auto sources only), and delete from disk.

- <a id="P-PENPAL-BATCH-OPS"></a>**P-PENPAL-BATCH-OPS**: Users can select multiple files in the sidebar via shift-click (extends selection from last-clicked file to shift-clicked file within the same source). A floating selection bar appears at the bottom of the sidebar showing the count and batch actions: copy markdown (concatenated), copy paths, publish, and delete. A "Clear" button dismisses the selection.

- <a id="P-PENPAL-DELETE-FILE"></a>**P-PENPAL-DELETE-FILE**: Files and source groups can be deleted from disk via the action menu. A confirmation modal prevents accidental deletion. When a file is deleted, its associated comments are also deleted, and empty parent directories are cleaned up.

---

## Markdown Viewer

- <a id="P-PENPAL-RENDER"></a>**P-PENPAL-RENDER**: Clicking a file opens a two-pane view: rendered markdown on the left, comments panel on the right, with a draggable divider.

- <a id="P-PENPAL-VIEW-MARGINS"></a>**P-PENPAL-VIEW-MARGINS**: The file view maintains equal margins between the content and each side boundary (left sidebar and right comments panel). Content expands to fill available space between the margins.

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

- <a id="P-PENPAL-HIGHLIGHT-CROSS"></a>**P-PENPAL-HIGHLIGHT-CROSS**: A single highlight can span across formatting boundaries — bold, italic, code, links, and other inline markup — as well as cross between code blocks and surrounding prose.

- <a id="P-PENPAL-HIGHLIGHT-MEDIA"></a>**P-PENPAL-HIGHLIGHT-MEDIA**: A highlight that intersects an image or mermaid diagram expands to encompass the entire media element. Selecting text that spans into, through, or starting within a diagram highlights the diagram with a visible yellow border and highlights the adjacent text normally. Dragging a selection out of a mermaid diagram transitions from rectangle selection to text selection, including the whole diagram in the resulting highlight.

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

- <a id="P-PENPAL-AGENT-PRESENCE"></a>**P-PENPAL-AGENT-PRESENCE**: Running agents are automatically detected and mapped to their projects. Visual indicators (dots) appear on project tree items in the home view and in the project breadcrumb bar.

- <a id="P-PENPAL-WAIT-CHANGES"></a>**P-PENPAL-WAIT-CHANGES**: Agents respond to new comments and thread changes in near real-time, waiting idle until a human posts a comment.

- <a id="P-PENPAL-INCORPORATE-ANSWERS"></a>**P-PENPAL-INCORPORATE-ANSWERS**: When a reviewer answers an open question, the agent incorporates the answer into the relevant section of the document and removes the question from the open questions list. The agent does not strikethrough the question or add the answer inside the open questions list.

---

## Publishing

- <a id="P-PENPAL-PUBLISH"></a>**P-PENPAL-PUBLISH**: Any markdown file can be published as a hosted web page (via the Blockcell service) — a self-contained HTML page with mermaid diagrams, syntax highlighting, and a table of contents. After publishing, a toast notification with a clickable URL appears briefly in the file viewer.

- <a id="P-PENPAL-PUBLISH-STATE"></a>**P-PENPAL-PUBLISH-STATE**: Publish state (site name, URL, timestamp) is persisted per file. Previously-published files show a "Copy Blockcell link" option in the action menu.

---

## CLI

- <a id="P-PENPAL-CLI-OPEN"></a>**P-PENPAL-CLI-OPEN**: The `penpal open <path>...` command opens one or more files or directories in the Penpal app, launching the app if it's not running. Directories are resolved to their project; `.md` files are auto-added to their containing project if not already tracked (or a new standalone project is created). Non-`.md` files are rejected.

---

## Real-Time Updates

- <a id="P-PENPAL-REALTIME"></a>**P-PENPAL-REALTIME**: The UI updates automatically when files change, agents start or stop, comments are added, or navigation is triggered from the CLI. Updates resume seamlessly after a tab is hidden and made visible again, catching up on any missed events.

- <a id="P-PENPAL-FOCUS"></a>**P-PENPAL-FOCUS**: Live updates are scoped to what the user is viewing. Each window independently tracks which file or project is in focus, and updates are delivered for the focused content.

---

## Desktop App

- <a id="P-PENPAL-INSTALL"></a>**P-PENPAL-INSTALL**: On first launch, a modal prompts the user to install the `penpal` CLI (for command-line access) and the Claude Code plugin (which enables AI agents to participate in document review). The modal reappears after app updates until tools are current. If the install variant detects existing tools, it shows as "Update" instead of "Install".

- <a id="P-PENPAL-CLAUDE-PATH"></a>**P-PENPAL-CLAUDE-PATH**: If the `claude` binary cannot be found during install, the modal shows a text input for the user to manually provide the path. The path is validated and persisted for future use.

- <a id="P-PENPAL-NEW-WINDOW"></a>**P-PENPAL-NEW-WINDOW**: In the desktop app, Cmd+N opens a new window with a single tab on the home view. The app stays running when all windows are closed (macOS dock behavior); clicking the dock icon reopens a window on home.

- <a id="P-PENPAL-FIND"></a>**P-PENPAL-FIND**: In the desktop app, Cmd+F opens a Find bar for in-page text search with match highlighting and navigation.

- <a id="P-PENPAL-THEME"></a>**P-PENPAL-THEME**: A toggle switches between light and dark color themes. The preference is persisted locally. On first launch, the theme defaults to the OS-level preference.

- <a id="P-PENPAL-EXTERNAL-LINKS"></a>**P-PENPAL-EXTERNAL-LINKS**: In the desktop app, external HTTP links open in the system browser.

- <a id="P-PENPAL-FILE-HANDLER"></a>**P-PENPAL-FILE-HANDLER**: The macOS app registers as an alternate handler for markdown files (`.md`, `.markdown`). Penpal appears in Finder's "Open With" menu and can be selected via `open -a Penpal file.md`. Opening a markdown file this way behaves identically to `penpal open <path>` — the file is shown in the app, auto-added to its project if applicable, or opened as a standalone file.

---

## Session Persistence

- <a id="P-PENPAL-PERSIST-TABS"></a>**P-PENPAL-PERSIST-TABS**: When the user quits and relaunches Penpal, all previously open windows reappear with the same tabs (paths, titles, and per-tab back/forward history) they had when the app was closed.

- <a id="P-PENPAL-PERSIST-GEO"></a>**P-PENPAL-PERSIST-GEO**: Each window's position and size are restored on relaunch, so the user's multi-window layout is preserved.

- <a id="P-PENPAL-PERSIST-FALLBACK"></a>**P-PENPAL-PERSIST-FALLBACK**: If the saved session is missing or corrupt, the app launches a single default window (1200×800, centered) with one tab on Home — the same behavior as a fresh install.

---

## Open Questions

(none)

## Resolved Questions
