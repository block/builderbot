# AGENTS.md

## Overview

**Staged** is a desktop git diff viewer. Tauri app with Rust backend (git2 for git ops) and Svelte/TypeScript frontend.

## Architecture

```
Frontend (Svelte/TS)           Backend (Rust/Tauri)
────────────────────           ────────────────────
src/                           src-tauri/src/
  App.svelte                     lib.rs (Tauri commands)
  lib/                           git/
    *.svelte (components)          mod.rs (re-exports)
    services/git.ts ──invoke──►    repo.rs (find_repo, get_branch_name)
    theme.ts (color theming)       status.rs (get_status)
    types.ts                       diff.rs (get_file_diff, parse diff)
                                   staging.rs (stage/unstage/discard)
                                   commit.rs (create/amend commit)
```

Frontend calls Rust via Tauri's `invoke()`. All git operations happen in Rust using libgit2.

### Design Principles

- **Stateless**: Git is the state. All Rust functions are pure - they discover the repo fresh each call.
- **Thin commands**: Tauri commands in `lib.rs` are one-liner wrappers around `git::*` functions.
- **Pure git module**: `src-tauri/src/git/` has no Tauri dependency - just git2 + serde.

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
