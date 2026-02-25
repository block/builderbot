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

Building requires Go, Node.js, and Rust.

### Develop

```bash
just dev           # Desktop app with Vite HMR + Go sidecar
```

### Install

```bash
just install       # Build .app, copy to /Applications, install Claude Code plugin
just uninstall     # Remove .app and Claude Code plugin
```

### CLI

```bash
penpal open thoughts/shared/plans/my-doc.md   # Open file in Penpal desktop app
```

## Development

| Command | Description |
|---|---|
| `just dev` | Desktop app with Vite HMR + Go sidecar |
| `just build` | Build sidecar + frontend + desktop `.app` bundle |
| `just build-sidecar` | Build Go sidecar binaries for desktop app |
| `just test` | Run all tests (Go + React) |
| `just test-e2e` | Playwright end-to-end tests |
| `just check` | Format Go code + tidy dependencies |
| `just clean` | Remove build artifacts |

### Server options

```bash
./penpal -port 3000              # Custom API port (default: 8080)
./penpal -go-port 9000           # Custom Go template UI port (default: 8081)
./penpal -root /path/to/projects # Custom root directory
```

## [Changelog](CHANGELOG.md) | [Roadmap](ROADMAP.md)
