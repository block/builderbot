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

## Installation

### Prerequisites

**just** - Command runner ([install guide](https://github.com/casey/just))
```bash
brew install just
```

The remaining dependencies (Go, Node.js, Rust) are needed to build from source.

### Install

```bash
just install
```

This builds the Penpal desktop app, copies it to `/Applications`, and installs the Claude Code plugin.

### Uninstall

```bash
just uninstall
```

## Development

```bash
just dev         # Go server (:8080) + Vite dev server (:5173), opens browser
just dev-tauri   # Full Tauri desktop app with Vite HMR
just test        # Run all tests (Go + React + JS)
```

| Command | What's built | Ports | UI |
|---|---|---|---|
| `just dev` | Go binary | Go `:8080`, Vite `:5173` | Opens `localhost:5173` in browser |
| `just dev-tauri` | Go sidecar binaries | Sidecar `:8080`, Vite `:5173` | Tauri native window |
| `just build` | Sidecar + frontend + `.app` bundle | — | — |
| `just install` | Same as build | — | Copies `.app` to `/Applications` |
| `just test` | — | — | — |

### Options

```bash
./penpal -port 3000              # Custom port (default: 8080)
./penpal -root /path/to/projects # Custom root directory
```

## [Changelog](CHANGELOG.md) | [Roadmap](ROADMAP.md)
