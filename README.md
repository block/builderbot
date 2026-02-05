# Birdseye

A local web app for browsing `thoughts/` directories across your projects. Designed for developers who use AI agents that generate research and planning documents.

## Features

- Auto-discovers projects with `thoughts/` directories
- Flat file view with research/plan type badges
- Full-text search across all files
- Rendered markdown with syntax highlighting
- Git branch and status display

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
