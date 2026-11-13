# Staged

A desktop app for reviewing git changes, managing branches, and running AI coding sessions. Built with Tauri (Rust + libgit2) and Svelte.

## What It Does

**Staged** is a visual workspace for git repositories. Browse diffs with syntax highlighting, manage branches and worktrees, and launch AI agent sessions to make changes — all from a single window.

- **Diff viewer** — Side-by-side diffs between any two refs (branches, commits, tags, or the working tree)
- **Project & branch management** — Track multiple projects, create branches, and view branch timelines
- **Rebase & squash** — Rebase a branch onto its base (or onto its own remote) and squash its commits from the branch timeline. A rebase linearises merge commits on the branch: pure automatic merges are dropped, while hand edits made inside a merge are kept as their own commit at the merge's position, or the rebase stops with a conflict that names the merge. The rebase step's output lists what happened to each merge. Rebasing a branch that has merge commits needs git 2.38 or newer (for `git merge-tree --write-tree`); an older git is refused before the branch is touched rather than risking dropped content, while branches without merges rebase on any git.
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
- Link the `stagedctl` control CLI and install the `staged` launcher in `/usr/local/bin`

**Note**: This builds from source, which takes a few minutes. It is meant for development and does not use the signed auto-updating release channel.

### Command Line Usage

Staged ships a control CLI, `stagedctl`, inside the app bundle. Its `project open` verb shows a project in the app from a terminal:

```bash
stagedctl project open "My Project"   # by name or id (see `stagedctl project list`)
stagedctl project open                # the project whose workspace the current directory is in
stagedctl project open --no-launch    # fail instead of starting Staged when it is not running
```

The Staged window you were last in comes to the front and switches to the project: the window that holds focus, or else the one that held it most recently and is still open. On success the command prints the project's name and id and the pid of the Staged instance that showed it. A project that is already on screen is a success too. No new window is opened, and a Staged that is already running is never joined by a second instance.

- **By name or id.** `<project>` is matched as an exact id, then an exact name, then a name that matches one project when case is ignored. A name that matches nothing, or several projects, fails before any window is touched.
- **From a project workspace.** With no argument, the project is the one whose workspace the current directory is in: any directory under `~/.staged/workspaces/<local|remote>/projects/<project-id>/`, where Staged keeps a project's worktrees. From any other directory the command fails with `not_in_workspace` without looking for the app, so pass a project there. It does not open arbitrary directories or repositories.
- **A busy window.** The window is raised whether or not it could switch, so you can see what is in the way. A dialog that is open, or a diff comment that could not be saved, is left alone and reported as `ui_busy`: close or finish it and run the command again.
- **When Staged is not running.** The command starts the `Staged.app` that `stagedctl` is bundled in, waits up to thirty seconds for it to come up, and then opens the project. The new window is not shown early: it comes to the front once it has finished loading, which can be shortly after the command has exited. A line on stderr says Staged is being started. This is macOS only.
- **`--no-launch`.** The command exits 3 instead of starting Staged.

Staged is also never started when you are not yet authorized (see below), when the directory names no project, when `--lock-path` or `$STAGEDCTL_LOCK` points at one particular instance, or from a development build, whose app needs its dev server (run `just dev` instead).

#### First run: `stagedctl connect`

Outside a Staged-spawned agent session, `stagedctl` needs your approval once per machine. With Staged running:

```bash
stagedctl connect --client "my terminal"
```

Staged shows a consent dialog naming the client. Approving it stores a grant under `~/.staged/control/grants/` that later commands use without asking again. It lasts thirty days from its last use and can be revoked under **Settings -> Connected apps** or with `stagedctl disconnect`.

Until then `stagedctl project open` exits 4 with `authorization_required`. It does not raise the consent dialog for you, and it does not start Staged to ask.

Two kinds of credential are refused with `scope_denied` (exit 4):

- **Read-only grants** (`stagedctl connect --read-only`). Showing a project marks it read and records it as the last one shown, which is a write.
- **Agent session credentials.** A session spawned by Staged cannot open a project, not even its own, so an agent cannot pull your window away. Run the command from your own terminal.

A grant confined to one project (`stagedctl connect --project`) opens that project only. Any other project reads `not_found`, because that grant does not list it.

#### The `staged` shorthand

`staged` is a small wrapper script around the same command:

```bash
staged "My Project"   # stagedctl project open "My Project"
staged                # stagedctl project open
staged --no-launch    # stagedctl project open --no-launch
```

It passes its arguments to `stagedctl project open` unchanged, so the output and exit codes are the ones described here. It does not take a directory to open: a path is read as a project name. The script never starts the app itself. If it cannot find `stagedctl` on your PATH, in `/Applications/Staged.app` or in `~/Applications/Staged.app`, it says so and exits 127 without starting anything.

#### Getting `stagedctl` on your PATH

- **Development install from source.** The install script links `/usr/local/bin/stagedctl` to the copy inside `/Applications/Staged.app` and installs `staged` beside it.
- **Release DMG.** Installing puts nothing on your PATH. In Staged, open **Settings -> Connected apps** and press **Add to PATH** under **Command line**.
  - **Where the link goes.** Staged links `/usr/local/bin/stagedctl` to the copy inside the app you are running. When `/usr/local/bin` cannot be written without an administrator password, it links `~/.local/bin/stagedctl` instead. Staged never asks for a password.
  - **If the directory is not on your PATH.** Staged says so after linking. For zsh, add this line to `~/.zshrc` and open a new terminal:

    ```bash
    export PATH="$HOME/.local/bin:$PATH"
    ```

  - **Nothing is overwritten unseen.** If a `stagedctl` is already at that path and points at another copy of Staged, the app shows where it points and replaces it only when you press **Replace link**. A file that is not a link is left alone: move or delete it, then press **Add to PATH** again.
  - **When it is refused.** When a downloaded app is opened before it has been moved, macOS runs it from a temporary, read-only location (App Translocation). A link to that location would stop working, so Staged refuses. Move Staged to your Applications folder, open it from there, and try again. A development build is refused too, because it has no app bundle to link to.
  - **Pressing it again is safe.** A link that is already right is reported and left as it is.

- **By hand.** The Settings action is the supported route. To make the link yourself instead, for example to put it in `/usr/local/bin` on a Mac where that needs `sudo`:

  ```bash
  sudo mkdir -p /usr/local/bin
  sudo ln -sfn /Applications/Staged.app/Contents/MacOS/stagedctl /usr/local/bin/stagedctl
  ```

  Drop `sudo` where `/usr/local/bin` is yours to write. `ln -sfn` replaces whatever link is already at that path without showing it. Use a link, not a copy. `stagedctl` finds the app to start from its own location inside the bundle, and a link stays in step with app updates.

- **The `staged` script.** If you installed manually, copy `scripts/staged` from the repository root to a directory on your PATH.
- **Agent sessions** spawned by Staged already have `stagedctl` on their PATH.

#### Exit codes

On failure, stderr carries one `code: message` line.

| Exit | Code                     | Meaning                                                                                                                                   |
| ---- | ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------- |
| 0    |                          | The project is on screen.                                                                                                                 |
| 1    | `not_found`              | No project matches; the message lists the project names. Also a workspace whose project was deleted.                                      |
| 1    | `ambiguous`              | The name matches several projects when case is ignored.                                                                                   |
| 1    | `not_in_workspace`       | No argument, and the current directory is not inside a Staged project workspace.                                                          |
| 1    | `ui_busy`                | The window was raised but did not switch: a dialog is open, or a diff comment was not saved. Or another `project open` replaced this one. |
| 1    | `ui_timeout`             | The window was raised but did not answer in time.                                                                                         |
| 1    | `app_error`              | The app could not make the request, for example a running Staged from before this command existed (update it).                            |
| 2    | `transport`              | A fault talking to the app.                                                                                                               |
| 3    | message only             | Staged is not running and was not started, a started Staged did not come up in time, or the running Staged cannot be used.                |
| 4    | `authorization_required` | No grant yet. Run `stagedctl connect`.                                                                                                    |
| 4    | `scope_denied`           | A read-only grant, or an agent session's credential.                                                                                      |
| 127  |                          | `staged` only: `stagedctl` was not found.                                                                                                 |

`stagedctl project open --help` is the full reference, and `stagedctl --help` lists everything else the CLI can do.

### AI Agent Setup (ACP)

Staged manages the Claude Code and Codex ACP bridges itself: on every launch
it installs (or upgrades) them into `~/.staged/packages` on a Staged-managed
Node.js runtime, so no host Node or manual `npm install` is needed. The first
launch downloads them, which requires network access; after that they work
offline. Other agents (Goose, Pi, copilot, amp) are discovered by CLI command
name on your PATH, and **Settings -> Doctor** can install the npm-based ones
into the same Staged-private prefix.

Open **Settings -> Doctor** to verify agent checks are detected as installed.
A Claude or Codex row that shows "Managed by Staged" is running Staged's copy.
Until the first launch install has completed, a row that found another copy
on your PATH (a Homebrew or npm global install, say) says Staged's managed
install "hasn't completed yet" — not a failure; it corrects itself when the
install lands, and a **Fix** installs the managed copy right away if you would
rather not wait. If the managed install failed, the row instead warns that the
managed copy is missing, with the error code the last install attempt
recorded; **Copy details** has the full error — which names the registry or
URL it used — a hint on what to change (a blocked `registry.npmjs.org`, a
mirror without the package, an unreachable host, a wrong Node.js download
URL) and the npm registry Doctor detected. The **Node.js Runtime** row
likewise reports a failed runtime download rather than staying hidden. When
managed installs are disabled (see `STAGED_ACP_TOOLS_DIR` below), the rows say
so instead.

To develop against a local bridge checkout, export `STAGED_ACP_TOOLS_DIR`
pointing at a directory with the bridge binaries — this disables the managed
installs and resolves bridges from that directory instead.

#### npm registry and Node.js download sources

Staged does not hardcode where packages come from:

- **npm registry.** Staged passes no `--registry` of its own, so npm uses
  its normal configuration (`NPM_CONFIG_REGISTRY`, `~/.npmrc` including
  scoped `@scope:registry` entries and auth tokens, then
  `registry.npmjs.org`). A corporate laptop whose `~/.npmrc` already points
  at an internal mirror keeps using it. Set `STAGED_NPM_REGISTRY` to force a
  registry for every Staged-run npm command (bridge installs, Doctor's
  npm-based installs, and update checks); it is passed as `--registry` and,
  for a scoped package such as `@agentclientprotocol/*`, as that scope's
  `--@scope:registry` too, so it also overrides a scoped `~/.npmrc` entry.
  Doctor's cached "latest version" answers are keyed by this configuration
  (override, environment, and `~/.npmrc` identity), so editing either is
  picked up on the next check rather than after the cache's hour.
- **Node.js runtime.** The pinned runtime (`node-runtime.lock.json`) is
  downloaded from `https://nodejs.org/dist` and verified against the lock's
  SHA-256. Set `STAGED_NODE_DIST_URL` to a mirror with the same path layout
  (`<base>/<version>/<tarball>`) if your network blocks `nodejs.org`.

A build can additionally compile in fallbacks for networks where the above
fail: `STAGED_FALLBACK_NPM_REGISTRY` is retried once when npm's configured
registry turns out to be `registry.npmjs.org` behind a network gateway that
answers with a block page (and no `STAGED_NPM_REGISTRY` was set), and
`STAGED_FALLBACK_NODE_DIST_URL` is retried once when the download from
`nodejs.org` fails to reach the host (and no `STAGED_NODE_DIST_URL` was set —
an explicit override is never second-guessed, for either). Both are read at
*compile time* and are unset for from-source builds. Block's release workflow
passes them from the
`STAGED_FALLBACK_NPM_REGISTRY` / `STAGED_FALLBACK_NODE_DIST_URL` repository
variables, which must be set for Block builds to keep working on managed
laptops that have no `~/.npmrc`.

## Development

### Prerequisites

This project uses [Hermit](https://github.com/cashapp/hermit) to manage development tools (Rust, Node.js, just). Hermit ensures everyone uses the same tool versions without global installs.

**First time setup:**

```bash
source bin/activate-hermit   # Activate hermit environment
```

After activation, `cargo`, `node`, `npm`, and `just` are all available from the hermit-managed versions. The Rust toolchain is pinned by the repo-root `rust-toolchain.toml`; hermit's `rustup` reads it and installs the pinned version on first use, so there is no need to run `rustup default`.

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
just ci         # Verify formatting, lint, typecheck, tests — what CI runs
just ci-local   # Pre-push hook when Staged changed: same checks, frontend and Rust in parallel; skips Rust if no Rust file changed

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
