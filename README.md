# Staged

A desktop diff viewer for git repositories. View any diff between refs, review changes, and prepare commits. Built with Tauri (Rust + libgit2) and Svelte.

## What It Does

**Staged** lets you view diffs between any two git refs—branches, commits, tags, or the working tree. Select a base and head ref, browse changed files, and see side-by-side diffs with syntax highlighting.

Key concepts:
- **Flexible diffs**: Compare any ref to any ref (e.g., `main..HEAD`, `HEAD..@` for uncommitted changes)
- **`@` = working tree**: Use `@` as a ref to include uncommitted changes
- **Review sessions**: Mark files as reviewed, add comments (coming soon)
- **File watching**: Auto-refresh when files change on disk

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (1.77.2+)
- [Node.js](https://nodejs.org/) (v18+)
- [just](https://github.com/casey/just) (command runner)

### Quick Start

```bash
just install   # Install npm + cargo dependencies
just dev       # Run in development mode (hot-reload)
```

### Commands

```bash
just dev        # Run app in dev mode with hot-reload
just build      # Build for production
just frontend   # Run just the frontend (quick UI iteration)

# Code quality
just fmt        # Format all code (Rust + TypeScript/Svelte)
just lint       # Lint Rust with clippy
just typecheck  # Type check TypeScript + Svelte + Rust
just check-all  # Run all checks (format, lint, typecheck)

# Maintenance
just install    # Install all dependencies
just clean      # Remove build artifacts
```

## Architecture

```
src-tauri/src/
├── diff/           # Core diff engine
│   ├── git.rs      # Git operations (libgit2)
│   ├── types.rs    # Data structures
│   ├── actions.rs  # File actions (stage, discard, etc.)
│   ├── review.rs   # Review session storage
│   └── watcher.rs  # File system watching
├── lib.rs          # Tauri commands (API surface)
└── refresh.rs      # Debounced refresh coordination

src/
├── App.svelte              # Main app shell
└── lib/
    ├── Sidebar.svelte      # File list with status indicators
    ├── DiffViewer.svelte   # Side-by-side diff display
    ├── DiffSelectorModal.svelte  # Ref picker UI
    └── services/           # Frontend services
```

## License

MIT
