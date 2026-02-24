# Penpal

A desktop app and local web server that **only** operates on markdown files inside `thoughts/` directories. It auto-discovers projects containing a `thoughts/` directory and provides a UI for browsing, searching, and collaboratively reviewing the documents within.

**This is NOT a code review tool.** Penpal is for reviewing _documentation_ -- research, plans, guides, and other markdown artifacts that AI agents generate in `thoughts/` directories.

## Features

- Auto-discovers projects with `thoughts/` directories
- Flat file view with research/plan type badges
- Full-text search across all files
- Rendered markdown with syntax highlighting and mermaid diagrams
- Git branch and status display
- **Comment threads** anchored to specific text in documents (like Google Docs)
- **Review workflow** -- agents can request review, humans leave comments, agents respond
- **MCP server** at `/mcp` for AI agents to participate in document review programmatically
- **Agent presence** -- shows when an agent is actively monitoring a file

## Quick Start

### Prerequisites

**just** - Command runner ([install guide](https://github.com/casey/just))
```bash
brew install just
```

### Run the web UI

```bash
just run
```

Builds the frontend and Go server, then opens the React UI at `localhost:8080`. The Go template UI is also available at `localhost:8081`.

### Claude Code plugin

```bash
just install-claude      # Install the Penpal plugin for Claude Code
just uninstall-claude    # Remove it
```

## Desktop App

Penpal also ships as a native macOS desktop app. Building requires Go, Node.js, and Rust.

### Develop

```bash
just dev-desktop    # Desktop app with Vite HMR + Go sidecar
```

### Install

```bash
just install      # Build .app, copy to /Applications, install Claude Code plugin
just uninstall    # Remove .app and Claude Code plugin
```

## Development

```bash
just dev           # Go server + Vite dev server, opens localhost:5173
just test          # Run all tests (Go + React + JS)
```

| Command | Description |
|---|---|
| `just dev` | Go server (`:8080`/`:8081`) + Vite dev (`:5173`), opens browser |
| `just build` | Build Go binary |
| `just build-sidecar` | Build Go sidecar binaries for desktop app |
| `just build-desktop` | Build sidecar + frontend + desktop `.app` bundle |
| `just test` | Run all tests (`test-go` + `test-frontend` + `test-js`) |
| `just test-go` | Go tests |
| `just test-frontend` | React tests |
| `just test-js` | Legacy JS tests |
| `just test-e2e` | Playwright end-to-end tests |
| `just test-all` | All tests including e2e |
| `just fmt` | Format Go code |
| `just tidy` | Tidy Go dependencies |
| `just clean` | Remove build artifacts |

### Server options

```bash
./penpal -port 3000              # Custom API port (default: 8080)
./penpal -go-port 9000           # Custom Go template UI port (default: 8081)
./penpal -root /path/to/projects # Custom root directory
```

## [Changelog](CHANGELOG.md) | [Roadmap](ROADMAP.md)
