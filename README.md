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
just dev          # start Mark
just dev staged   # start Staged
```

## App Installation

For end-user installs:

- Mark (macOS): `curl -fsSL https://raw.githubusercontent.com/block/builderbot/main/apps/mark/install.sh | bash`
- Staged: no standalone installer yet; run from source with `just dev staged` or build with `just app staged build`

## Command Guide

The root `justfile` supports both styles:

```bash
just dev staged     # verb-first (recommended for humans)
just app staged dev # explicit delegation form
```

Useful commands:

```bash
just apps         # list app names
just setup        # first-time setup (hooks + JS deps)
just dev          # run Mark app
just dev staged   # run Staged app
just mark         # alias for `just app mark dev`
just staged       # alias for `just app staged dev`
just check        # full non-modifying checks
just ci           # alias of `just check`
just fmt          # format repo
just lint         # lint repo
just test         # run Rust workspace tests
```

## Repo Layout

- `apps/mark`: main desktop app.
- `apps/staged`: standalone staged/diff app.
- `packages/diff-viewer`: shared Svelte diff viewer package.
- `crates/`: shared Rust crates.
- `scripts/`: helper scripts used by app tooling.

## App-Level Commands

Each app has its own `justfile` for app-specific workflows:

- `apps/mark/justfile`
- `apps/staged/justfile`

Use root delegation to run any app recipe:

```bash
just app mark ci
just app staged build
```

## Troubleshooting

- `just: command not found`
  - Run `source ./bin/activate-hermit` in your shell.
- Frontend waiting on dev server / port issues
  - Check listeners: `lsof -nP -iTCP:<port> -sTCP:LISTEN`
- `node_modules` appears at repo root
  - Expected for this `pnpm` workspace monorepo; app `node_modules` entries are symlinked into the root store.

## More Docs

- Mark app docs: `apps/mark/README.md`
