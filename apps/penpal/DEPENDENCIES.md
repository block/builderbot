---
scope: External dependencies — tools and services that must be present in the environment because the system cannot supply them itself.
see-also:
  - ERD.md — engineering requirements, including managed toolchains that eliminate would-be dependencies.
  - PRODUCT.md — product requirements that drive deployment modes.
---

# External Dependencies

## Runtime Dependencies

### <a id="D-DEP-GIT"></a>D-DEP-GIT: Git CLI

- **Used by:** Discovery (branch name, dirty status, unpushed commits), worktree discovery (`git worktree list --porcelain`), config (`git config core.excludesFile`)
- **Where it runs:** Runtime on every machine running `penpal-server`
- **Why external:** Git is a separate tool managing repository state; cannot be embedded

### <a id="D-DEP-CLAUDE"></a>D-DEP-CLAUDE: Claude Code CLI

- **Used by:** Agent management (`agents.Manager.Start()` spawns `claude` subprocess), install flow (`claude plugin marketplace add`, `claude plugin install`)
- **Where it runs:** Runtime on end-user machine (optional — agent features degrade gracefully if not found)
- **Why external:** Separately distributed Anthropic CLI that authenticates to Anthropic's APIs

### <a id="D-DEP-PS-LSOF"></a>D-DEP-PS-LSOF: ps and lsof

- **Used by:** Agent detection (`ps -eo pid,args` to find claude processes, `lsof -a -p {pid} -d cwd -Fn` to determine CWD)
- **Where it runs:** Runtime on macOS (primary target)
- **Why external:** Process inspection requires OS-level tools

### <a id="D-DEP-BLOCKCELL"></a>D-DEP-BLOCKCELL: Blockcell API

- **Used by:** Publish feature (`POST https://blockcell.sqprod.co/api/v1/sites/{name}/upload`)
- **Where it runs:** Runtime, triggered by the "Publish to Blockcell" action
- **Why external:** Remote hosted static-site hosting service; cannot be run locally

### <a id="D-DEP-ANTHROPIC-API"></a>D-DEP-ANTHROPIC-API: Anthropic API

- **Used by:** Agent collaboration (accessed indirectly via the `claude` CLI subprocess with `--model opus`)
- **Where it runs:** Runtime, when user starts an agent for a project
- **Why external:** Hosted AI inference service; credentials managed by the claude CLI

## Build-Time Dependencies

### <a id="D-DEP-GO"></a>D-DEP-GO: Go 1.24+

- **Used by:** Compiling `penpal-server` and `penpal-cli` binaries
- **Where it runs:** Dev and CI only (binaries are statically compiled and bundled)
- **Why external:** Go is a compiled language runtime

### <a id="D-DEP-RUST"></a>D-DEP-RUST: Rust (stable) + Cargo

- **Used by:** Compiling the Tauri shell (`frontend/src-tauri/`)
- **Where it runs:** Dev and CI only
- **Why external:** Tauri requires native Rust compilation

### <a id="D-DEP-NODE"></a>D-DEP-NODE: Node.js + pnpm

- **Used by:** Building the React frontend (Vite), running frontend tests (Vitest), running e2e tests (Playwright)
- **Where it runs:** Dev and CI only (compiled JS/HTML assets bundled into .app)
- **Why external:** Vite and TypeScript compiler require a Node.js runtime

## Optional Runtime Dependencies

### <a id="D-DEP-BREW"></a>D-DEP-BREW: Homebrew

- **Used by:** Install flow (`brew --prefix` to locate bin directory for CLI symlink)
- **Where it runs:** Runtime on macOS (optional — falls back to `/usr/local/bin`)
- **Why external:** macOS package manager; optional with fallback

### <a id="D-DEP-OSASCRIPT"></a>D-DEP-OSASCRIPT: osascript

- **Used by:** Copy-file-path feature (clipboard access via AppleScript)
- **Where it runs:** Runtime on macOS only
- **Why external:** macOS-specific clipboard scripting
