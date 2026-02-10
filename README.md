# Birdseye

A local web app that **only** operates on markdown files inside `thoughts/` directories. It auto-discovers projects containing a `thoughts/` directory and provides a web UI for browsing, searching, and collaboratively reviewing the documents within.

**This is NOT a code review tool.** Birdseye is for reviewing _documentation_ -- research, plans, guides, and other markdown artifacts that AI agents generate in `thoughts/` directories.

## Features

- Auto-discovers projects with `thoughts/` directories
- Flat file view with research/plan type badges
- Full-text search across all files
- Rendered markdown with syntax highlighting
- Git branch and status display
- **Comment threads** anchored to specific text in documents (like Google Docs)
- **Review workflow** -- agents can request review, humans leave comments, agents respond
- **MCP server** at `/mcp` for AI agents to participate in document review programmatically
- **Agent presence** -- shows when an agent is actively monitoring a file

## Installation

### Prerequisites

1. **just** - Command runner for development tasks (the only manual install)
   ```bash
   # On macOS with Homebrew
   brew install just

   # Or see https://github.com/casey/just for other platforms
   ```

The remaining dependencies are auto-installed when you run `just run` or `just dev`:

2. **Claude Code** - Birdseye is designed to work with Claude Code agents for collaborative document review (see [go/claude-code](https://go/claude-code) for setup details)
3. **Go 1.21+** - Required to build and run the server
4. **fswatch** (optional) - For hot reload in dev mode, installed automatically by `just dev`

## Usage

```bash
just run    # Build, start server, open browser
just dev    # Same, but with hot reload on file changes
```

## Options

```bash
./birdseye -port 3000              # Custom port (default: 8080)
./birdseye -root /path/to/projects # Custom root directory
```

## Claude Code Plugin

Birdseye ships as a Claude Code plugin that bundles an MCP server (for programmatic access to comments and reviews) and the `monitor-reviews` skill. Install it with:

```bash
just install-claude
```

This registers birdseye as a local plugin marketplace and installs the plugin. The birdseye server must be running for the MCP tools to work.
