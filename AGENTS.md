# AGENTS.md

## Overview

**Staged** is a desktop git diff viewer. Tauri app with Rust backend (git2 for git ops) and Svelte/TypeScript frontend.

## Architecture

Frontend calls Rust via Tauri's `invoke()`. All git operations happen in Rust using libgit2.

### Design Principles

- **Stateless**: Git is the state. All Rust functions are pure - they discover the repo fresh each call.
- **Rebuildable features**: Design features as self-contained modules with clear boundaries and minimal tendrils into the rest of the codebase. If a feature needs to be completely rewritten, it should be possible to delete and rebuild it without surgery across multiple files. See `refresh.rs` as an example.

### Theming

Colors are centralized in `src/lib/theme.ts` and applied via CSS custom properties in `app.css`.
All components use `var(--*)` CSS variables for colors.

## Commands

Use `just` for all dev tasks:

```bash
just dev        # Run with hot-reload
just fmt        # Format all code (cargo fmt + prettier)
just lint       # Clippy for Rust
just typecheck  # Type check everything
just check-all  # All checks before submitting
```

## Code Quality

**Always format and lint your work before finishing:**
```bash
just fmt        # Auto-format Rust + TypeScript/Svelte
just check-all  # Verify everything passes
```

- Rust: `cargo fmt` + `cargo clippy`
- TypeScript/Svelte: `prettier`

## Git Workflow

**Do not** create branches, commit, or push unless explicitly asked. The human manages git operations.
