# Changelog

## v0.2.0

- **In-app CLI and plugin installer** — Install the Claude Code plugin and penpal CLI tool directly from the app
- **Fix: files disappearing or duplicating in review lists** — Review lists now stay accurate as you add and remove files
- **Fix: backwards selection anchoring** — Comment anchors now work correctly when selecting text from bottom to top ([thanks @LindsayV!](https://github.com/block/builderbot/pull/314))

## Feb 25, 2026

- **CLI: `penpal open`** — New command opens files directly in the desktop app
- **Back/forward buttons** — Each tab maintains its own navigation history, with toolbar buttons and keyboard shortcuts (`Cmd+[` / `Cmd+]`), just like your browser
- **Resizable chat panel** — Drag to resize the comments panel ([thanks @donley!](https://github.com/squareup/personal-loganj-birdseye/pull/93))
- **Show file titles** — Penpal now parses files for their titles, instead of just showing filenames
- **Fix: agent reliability** — Agents now start reliably and SSE connections recover after disconnect
- **Fix: comment anchoring** — Comment highlights are more stable
- **Fix: open external links in browser** — Links to external sites open in your default browser instead of inside the app

## Feb 24, 2026

- **Desktop app** — Penpal has a macOS app with tabs, multiple window support, and Cmd+F find-in-page. Run `just install` to put the app in /Applications and install the Claude plugin.
- **Auto-discover Claude plans** — Automatically surfaces plan files from `~/.claude/plans/` ([thanks @donley!](https://github.com/squareup/personal-loganj-birdseye/pull/59))

## Feb 23, 2026

- **Comment on Mermaid diagrams** — Select an area on any Mermaid diagram to start a comment thread anchored to that region

## Feb 20, 2026

- **Renamed from Birdseye to Penpal** — The project, binary, MCP tools, plugin, and all UI branding are now "Penpal". Existing `.birdseye/` data directories and `~/.config/birdseye/` configs are automatically migrated on first run. Run `just install-claude` to switch to the new plugin (the old "birdseye" plugin is cleaned up automatically).
- **Recent page shows files on startup** — The Recent page now immediately shows recently modified files when the server starts, instead of starting empty and only tracking files changed at runtime

## Feb 18, 2026

- **Copy selection as markdown** — Select text in a document and click "Copy markdown" to copy the raw markdown source for your selection to the clipboard
- **Expand images to fullscreen** — Hover over any image or Mermaid diagram to reveal an "Expand" button that opens a near-fullscreen modal (thanks again @jstiefel! — [PR #33](https://github.com/squareup/personal-loganj-penpal/pull/33), [PR #34](https://github.com/squareup/personal-loganj-penpal/pull/34))

## Feb 17, 2026

- **Source detection improvements** — Auto-detect new source directories at runtime and de-duplicate files across overlapping sources

## Feb 13, 2026

- **Dark mode** — Theme toggle with system preference detection and dark Mermaid diagrams ([thanks @jstiefel!](https://github.com/squareup/personal-loganj-penpal/pull/28))

## Feb 11, 2026

- **In Review** — New global In Review page shows all files with open comments, and project pages surface files with active threads at the top, making it easier to track what you're working on
- **Suggested replies** — Agents can offer clickable reply options, so you can respond with one tap instead of typing "yes" or "do it" all the time
- **Agent presence improvements** — Agent busy indicators are more accurate and persistent, and agents always reply when they've finished work on a thread
- **Fix: hide actions during reply** — Reply/Resolve buttons hide while composing a reply

## Feb 10, 2026

- **[RP1](https://rp1.run) support** — Auto-detects `.rp1` trees and organizes files by type (features, issues, context, quick builds) with grouped display and badges
- **Bulk action improvements** — Select multiple files or entire sources to delete or manage in bulk, with a selection bar showing singular/plural counts
- **Fix: comment anchoring** — Comments on markdown with bold, italic, or link formatting now anchor correctly
- **Fix: stable dev port** — Dev server keeps a stable port instead of hopping on restart

## Feb 9, 2026

- **Publish to Blockcell** — Publish any document as a standalone HTML site with TOC sidebar, Mermaid diagram support, and a copy-markdown button
- **Activity feed** — New Recent page shows latest activity across all projects with labels like "edited" and "commented"
- **Agent context window usage** — See how much of the agent's context window is being used in real time
- **Fix: sidebar headings** — Headings with bold, italic, code, or links now appear correctly in the sidebar TOC
