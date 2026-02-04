# AGENTS.md

## Overview

**Staged** is a desktop git diff viewer. Tauri app with Rust backend (libgit2) and Svelte/TypeScript frontend.

## Architecture

Frontend calls Rust via Tauri's `invoke()`. All git operations happen in Rust using libgit2.

### Core Concepts

- **DiffId**: A diff is identified by `base..head` refs (e.g., `main..HEAD`, `HEAD..@`)
- **`@` = working tree**: Special ref representing uncommitted changes on disk
- **Review sessions**: Stored per DiffId, contain comments, edits, and reviewed file markers

### Backend Structure

```
src-tauri/src/
├── diff/           # Core module
│   ├── git.rs      # Git operations: compute_diff, get_refs, resolve_ref
│   ├── types.rs    # FileDiff, LinePair, Alignment, GitRef, etc.
│   ├── actions.rs  # stage_file, unstage_file, discard_file, etc.
│   ├── review.rs   # ReviewStore, Review, Comment, Edit persistence
│   └── watcher.rs  # File system watcher for auto-refresh
├── lib.rs          # Tauri command handlers (thin layer over diff module)
└── refresh.rs      # Debounced refresh controller
```

### Frontend Structure

```
src/
├── App.svelte                  # Main shell, state management
└── lib/
    ├── Sidebar.svelte          # File list with review status
    ├── DiffViewer.svelte       # Side-by-side diff with syntax highlighting
    ├── DiffSelectorModal.svelte # Ref autocomplete picker
    ├── types.ts                # TypeScript types mirroring Rust
    ├── theme.ts                # Color definitions
    └── services/
        ├── git.ts              # Tauri invoke wrappers
        ├── review.ts           # Review session API
        ├── highlighter.ts      # Syntax highlighting (Shiki)
        ├── scrollSync.ts       # Synchronized scroll for diff panels
        └── statusEvents.ts     # File watcher event handling
```

### Design Principles

- **Stateless backend**: Git is the source of truth. Rust functions discover repo state fresh each call—no cached state that can drift.
- **Rebuildable features**: Self-contained modules with clear boundaries. Features can be deleted and rebuilt without surgery across the codebase.
- **Horizontal space is precious**: Side-by-side diffs need width. Minimize chrome, prefer overlays, hide UI when possible.

### Theming

Colors defined in `src/lib/theme.ts`, applied via CSS custom properties in `app.css`.
All components use `var(--*)` for colors—no hardcoded values.

## Commands

```bash
just dev        # Run with hot-reload (human runs this)
just fmt        # Format all code
just lint       # Clippy for Rust
just typecheck  # Type check everything
just check-all  # All checks before submitting
```

**Note:** The human runs the dev server. Don't start it yourself.

## Code Quality

Before finishing work:
```bash
just fmt        # Auto-format Rust + TypeScript/Svelte
just check-all  # Verify everything passes
```

## Git Workflow

**Do not** create branches, commit, or push unless explicitly asked.

## Iterative Development Workflow

When working on complex changes with the human:

- **`.artifacts/` directory**: Store markdown artifacts (research, plans, design docs) here. Gitignored.
- **Documents**: When asked for a document, create/update markdown in `.artifacts/`
- **Code changes**: When asked for code changes, make a commit (after running `just fmt` and `just check-all`)
- **Amend vs new**: Keep amending the same commit / revising the same artifact until the human indicates we're moving to a new step
