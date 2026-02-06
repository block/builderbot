# AGENTS.md

## Overview

**builderbot** is a monorepo containing multiple independent projects. Each project lives in its own directory with self-contained tooling, dependencies, and documentation.

## Repository Structure

```
builderbot/
├── AGENTS.md           # This file - monorepo-level guidance
├── README.md           # Repo overview
├── CODEOWNERS          # Project ownership
├── GOVERNANCE.md       # Project governance
├── LICENSE             # Apache 2.0
├── renovate.json       # Dependency updates
│
└── <project>/          # Each project is a self-contained directory
    ├── AGENTS.md       # Project-specific AI guidance
    ├── README.md       # Project documentation
    ├── justfile        # Project build commands
    ├── bin/            # Hermit binaries (project-specific toolchain)
    └── ...             # Project source code
```

## Projects

| Directory | Description |
|-----------|-------------|
| `staged/` | Desktop git diff viewer (Tauri + Svelte) |

## Working in This Monorepo

### Navigation

Always `cd` into a project directory before running commands:

```bash
cd staged
just dev
```

### Tooling Philosophy

**Each project is self-contained:**

- **Justfiles**: Per-project. Run `just <command>` from within the project directory.
- **Hermit**: Per-project. Each project has its own `bin/` with pinned tool versions.
- **Dependencies**: Per-project. No shared node_modules or cargo workspaces across projects.

This allows projects to:
- Use different versions of tools (Node, Rust, etc.)
- Evolve independently without breaking siblings
- Be extracted to their own repo if needed

### Hermit Activation

Each project manages its own toolchain via Hermit:

```bash
cd staged
source bin/activate-hermit  # Or just run commands via bin/
bin/just dev                 # Works without activation
```

### Project-Specific Guidance

**Always read the project's `AGENTS.md`** before working on it. Each project has its own:
- Architecture overview
- Available commands
- Code quality requirements
- Git workflow preferences

## Adding a New Project

1. Create a new directory at the repo root
2. Initialize Hermit: `hermit init` (creates `bin/` directory)
3. Add required tools: `hermit install node just` etc.
4. Create a `justfile` with standard commands (`dev`, `fmt`, `lint`, `check-all`)
5. Create an `AGENTS.md` describing the project
6. Add the project to the table above

## Code Quality

Each project defines its own quality checks. The standard pattern is:

```bash
just fmt        # Format code
just lint       # Run linters
just typecheck  # Type checking
just check-all  # All of the above
```

Run these from within the project directory before submitting work.

## Git Workflow

- Work happens in project directories, but commits are at the repo root
- PRs can touch multiple projects if needed
- Each project may have its own CI workflows in `.github/workflows/`
