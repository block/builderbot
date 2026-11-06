# Builderbot Monorepo

This repository contains Builderbot apps, shared UI packages, and Rust crates.

## Quick Start

```bash
# from repo root
source ./bin/activate-hermit
just setup
```

`just setup` runs `lefthook install` and `pnpm install`.

Then run an app:

```bash
just dev          # start Staged
just dev differ   # start Differ
```

## App Installation

For end-user installs:

- Staged (macOS): `curl -fsSL https://raw.githubusercontent.com/block/builderbot/main/apps/staged/install.sh | bash`
- Differ: no standalone installer yet; run from source with `just dev differ` or build with `just app differ build`

## Command Guide

The root `justfile` supports both styles:

```bash
just dev differ     # verb-first (recommended for humans)
just app differ dev # explicit delegation form
```

Useful commands:

```bash
just apps         # list app names
just setup        # first-time setup (hooks + JS deps)
just dev          # run Staged app
just dev differ   # run Differ app
just staged       # alias for `just app staged dev`
just differ       # alias for `just app differ dev`
just check        # full non-modifying checks
just ci           # alias of `just check`
just fmt          # format repo
just lint         # lint repo
just test         # run Rust workspace tests
```

## Git Hooks

`just setup` installs the hooks in `lefthook.yml`. Like CI's path filters, they
skip the areas a change leaves alone, and print `(skip)` for them:

- pre-commit formats each area (shared crates, Staged, Differ, Penpal) that has
  staged files, then re-stages them.
- pre-push runs the checks for shared crates, Staged and Differ, each only when
  the branch changes a path that area's CI workflow watches. It compares the
  working tree, including uncommitted and untracked files, with where the branch
  forked from `origin/main` (see `scripts/unchanged`). Like the checks
  themselves, it looks at the checked-out branch, not the refs being pushed.
  Penpal's checks run in its CI only.

On Blox workstations, lhm merges a global lefthook config into these hooks. Its
pre-push used to run the full `just lint` (clippy and every app's lint, Penpal's
included) and `just test` on every push. The repo's pre-push `lint` and `test`
jobs now replace them, so pushing on Blox no longer lints Penpal. Penpal's CI
covers it.

The `fmt`, `lint` and `test` jobs are groups. A personal `lefthook-local.yml`
override for a job inside one, such as `crates-lint`, must use
`jobs: - name: crates-lint`, not `commands:`.

`just check` runs everything.

## Repo Layout

- `apps/staged`: main desktop app.
- `apps/differ`: standalone diff app.
- `packages/diff-viewer`: shared Svelte diff viewer package.
- `crates/`: shared Rust crates.
- `scripts/`: helper scripts used by app tooling.

## App-Level Commands

Each app has its own `justfile` for app-specific workflows:

- `apps/staged/justfile`
- `apps/differ/justfile`

Use root delegation to run any app recipe:

```bash
just app staged ci
just app differ build
```

## Troubleshooting

- `just: command not found`
  - Run `source ./bin/activate-hermit` in your shell.
- Frontend waiting on dev server / port issues
  - Check listeners: `lsof -nP -iTCP:<port> -sTCP:LISTEN`
- `node_modules` appears at repo root
  - Expected for this `pnpm` workspace monorepo; app `node_modules` entries are symlinked into the root store.

## More Docs

- Staged app docs: `apps/staged/README.md`
