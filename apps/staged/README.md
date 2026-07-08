# Staged

A desktop app for reviewing git changes, managing branches, and running AI coding sessions. Built with Tauri (Rust + libgit2) and Svelte.

## What It Does

**Staged** is a visual workspace for git repositories. Browse diffs with syntax highlighting, manage branches and worktrees, and launch AI agent sessions to make changes — all from a single window.

- **Diff viewer** — Side-by-side diffs between any two refs (branches, commits, tags, or the working tree)
- **Project & branch management** — Track multiple projects, create branches, and view branch timelines
- **AI agent sessions** — Launch coding sessions with ACP-compatible agents (Goose, Claude Code, Codex, Pi) and watch changes stream in
- **Review workflow** — Mark files as reviewed, add notes and annotations
- **File watching** — Auto-refresh when files change on disk

## Installation

### Stable Release (macOS arm64)

Download the signed DMG from GitHub Releases:

```text
https://github.com/block/builderbot/releases/download/staged-latest/Staged-latest-aarch64.dmg
```

Install `Staged.app` from that DMG if you want the auto-updating release channel. In-app updates are driven by the matching updater feed at:

```text
https://github.com/block/builderbot/releases/download/staged-latest/latest.json
```

Release and signing details live in [`docs/release-and-updates.md`](./docs/release-and-updates.md).

### Development Install From Source

Install with a single command:

```bash
curl -fsSL https://raw.githubusercontent.com/block/builderbot/main/apps/staged/install.sh | bash
```

The installer will:

- Clone the repository
- Set up the Hermit development environment
- Install dependencies
- Build the application
- Install to `/Applications/Staged.app`
- Install the `staged` CLI to `/usr/local/bin`

**Note**: This builds from source, which takes a few minutes. It is meant for development and does not use the signed auto-updating release channel.

### Command Line Usage

After installation, launch Staged from the terminal:

```bash
staged                # Open in current directory
staged /path/to/repo  # Open in specified directory
```

Each invocation opens a new window, so you can have multiple repos open simultaneously.

If you installed manually (not via the install script), copy `scripts/staged` to somewhere in your PATH (e.g., `/usr/local/bin`).

### AI Agent Setup (ACP)

Staged discovers ACP providers by CLI command name. For Claude Code ACP:

```bash
npm install -g @zed-industries/claude-agent-acp
```

This package installs the `claude-agent-acp` executable.

After installing, open Staged and run **Settings -> Doctor** to verify the Claude check is detected as installed.

## Development

### Prerequisites

This project uses [Hermit](https://github.com/cashapp/hermit) to manage development tools (Rust, Node.js, just). Hermit ensures everyone uses the same tool versions without global installs.

**First time setup:**

```bash
source bin/activate-hermit   # Activate hermit environment
rustup default stable        # Set the default Rust toolchain
```

After activation, `cargo`, `node`, `npm`, and `just` are all available from the hermit-managed versions.

### Quick Start

```bash
just install   # Install all dependencies (first time)
just dev       # Run in development mode (hot-reload)
```

### Commands

```bash
just install    # First-time setup: install all dependencies
just dev        # Run app in dev mode with hot-reload
just build      # Build for production
just prepare 0.1.1  # Create release branch, bump versions, push PR
just release 0.1.1  # Tag merged release commit and trigger release CI

# Code quality
just fmt        # Format all code (Rust + TypeScript/Svelte)
just lint       # Lint Rust with clippy
just typecheck  # Type check TypeScript + Svelte + Rust
just check-all  # Format, lint, typecheck — run before pushing

# CI (non-modifying)
just ci         # Verify formatting, lint, typecheck — for CI/hooks

# Maintenance
just clean      # Remove build artifacts
```

## Architecture

### Backend (Rust / Tauri)

```
src-tauri/src/
├── agent/                  # AI agent integration (ACP protocol)
│   ├── acp.rs              # ACP driver — spawns and communicates with agents
│   ├── writer.rs           # Streams agent messages to the frontend
│   └── mod.rs              # AgentDriver trait
├── git/                    # Git operations (libgit2 + CLI fallback)
│   ├── diff.rs             # Diff computation
│   ├── commit.rs           # Commit creation
│   ├── refs.rs             # Ref resolution
│   ├── files.rs            # File listing and status
│   ├── worktree.rs         # Worktree management
│   ├── github.rs           # GitHub API (PRs)
│   ├── cli.rs              # Git CLI fallback for operations libgit2 can't do
│   └── types.rs            # Shared data structures
├── store/                  # SQLite persistence layer
│   ├── projects.rs         # Project management
│   ├── branches.rs         # Branch tracking
│   ├── sessions.rs         # Agent session records
│   ├── commits.rs          # Commit metadata
│   ├── messages.rs         # Agent message history
│   ├── reviews.rs          # Review session storage
│   ├── notes.rs            # Notes and annotations
│   └── models.rs           # Database models
├── lib.rs                  # Tauri command definitions (API surface)
├── session_commands.rs     # Session management commands
├── session_runner.rs       # Agent session execution
└── recent_repos.rs         # Recent repository tracking
```

### Frontend (Svelte + TypeScript)

```
src/
├── App.svelte              # Main app shell and routing
└── lib/
    ├── DiffViewer.svelte   # Side-by-side diff display with syntax highlighting
    ├── ProjectHome.svelte  # Project dashboard
    ├── BranchTimeline.svelte   # Branch history and commit timeline
    ├── SessionLauncher.svelte  # AI session creation and management
    ├── AcpConfigPicker.svelte  # Agent provider/model/effort picker
    ├── TopBar.svelte       # Navigation and project controls
    ├── commands.ts         # Tauri command bindings
    ├── types.ts            # Shared TypeScript types
    ├── theme.ts            # Theme definitions (CSS custom properties)
    └── ...                 # ~30 components total
```

### Key dependencies

| Layer    | Dependency              | Purpose                       |
| -------- | ----------------------- | ----------------------------- |
| Backend  | `git2`                  | libgit2 bindings for git ops  |
| Backend  | `agent-client-protocol` | ACP agent communication       |
| Backend  | `rusqlite`              | SQLite persistence            |
| Backend  | `notify`                | File system watching          |
| Backend  | `syntect`               | Syntax highlighting           |
| Frontend | `shiki`                 | Syntax highlighting (browser) |
| Frontend | `marked`                | Markdown rendering            |
| Frontend | `lucide-svelte`         | Icons                         |

## License

MIT
