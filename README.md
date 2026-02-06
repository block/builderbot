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

## Usage

```bash
just run    # Build, start server, open browser
just dev    # Same, but with hot reload on file changes
```

Requires Go 1.21+ and [just](https://github.com/casey/just). Hot reload requires fswatch (auto-installed via brew).

## Options

```bash
./birdseye -port 3000              # Custom port (default: 8080)
./birdseye -root /path/to/projects # Custom root directory
```

## MCP Integration

Birdseye exposes an MCP server so AI agents can interact with document reviews. Install it for Claude Code:

```bash
just install-mcp
```

Available tools: `birdseye_list_threads`, `birdseye_read_thread`, `birdseye_reply`, `birdseye_create_thread`, `birdseye_resolve`, `birdseye_request_review`, `birdseye_files_in_review`.
