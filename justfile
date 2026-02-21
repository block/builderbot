# Default recipe
default: run

# Build the binary
build: ensure-deps
    go build -o penpal .

# Ensure required tools are installed
ensure-deps:
    #!/usr/bin/env bash
    if ! command -v go &> /dev/null; then
        echo "Installing Go..."
        brew install go
    fi
    if ! command -v claude &> /dev/null; then
        echo "Installing Claude Code..."
        brew install claude-code
    fi

# Run the server and open browser
run: build
    #!/usr/bin/env bash
    PORT=8080

    # If port is in use, check if it's a penpal instance we can take over
    if lsof -ti:$PORT >/dev/null 2>&1; then
        if curl -s "http://localhost:$PORT/api/projects" >/dev/null 2>&1; then
            echo "Port $PORT held by another penpal instance, stopping it..."
            BLOCKING_PID=$(lsof -ti:$PORT)
            kill $BLOCKING_PID 2>/dev/null
            sleep 0.5
        else
            echo "Error: port $PORT is in use by a non-penpal process." >&2
            echo "Stop that process or use: ./penpal -port <other-port>" >&2
            exit 1
        fi
    fi

    ./penpal &
    PID=$!

    # Wait for server to be ready
    echo "Waiting for server..."
    until curl -s http://localhost:$PORT/ > /dev/null 2>&1; do
        sleep 0.2
    done
    open "http://localhost:$PORT"

    wait $PID

# Development mode with hot reload
dev:
    #!/usr/bin/env bash
    # Install fswatch if needed
    if ! command -v fswatch &> /dev/null; then
        echo "Installing fswatch..."
        brew install fswatch
    fi

    PIDFILE=".penpal.pid"

    PORT=8080

    # Kill previous penpal dev server if running (via PID file or port probe)
    if [ -f "$PIDFILE" ]; then
        OLD_PID=$(cat "$PIDFILE")
        if kill -0 "$OLD_PID" 2>/dev/null; then
            echo "Stopping previous penpal server (PID $OLD_PID)..."
            kill "$OLD_PID" 2>/dev/null
            sleep 0.5
        fi
        rm -f "$PIDFILE"
    fi

    # If port is still in use, check if it's a penpal instance we can take over
    if lsof -ti:$PORT >/dev/null 2>&1; then
        if curl -s "http://localhost:$PORT/api/projects" >/dev/null 2>&1; then
            echo "Port $PORT held by another penpal instance, stopping it..."
            BLOCKING_PID=$(lsof -ti:$PORT)
            kill $BLOCKING_PID 2>/dev/null
            sleep 0.5
        else
            echo "Error: port $PORT is in use by a non-penpal process." >&2
            echo "Stop that process or use: ./penpal -dev -port <other-port>" >&2
            exit 1
        fi
    fi

    start_server() {
        go build -o penpal . && ./penpal -dev -port $PORT &
        PID=$!
        echo $PID > "$PIDFILE"
    }

    cleanup() {
        echo "Stopping server..."
        kill $PID 2>/dev/null
        rm -f "$PIDFILE"
        exit 0
    }
    trap cleanup INT TERM

    start_server

    # Wait for server to be ready before opening browser
    echo "Waiting for server..."
    until curl -s http://localhost:$PORT/ > /dev/null 2>&1; do
        sleep 0.2
    done
    open "http://localhost:$PORT"

    # Only watch .go files — template changes are picked up live via -dev flag
    echo "Watching for Go changes... (Ctrl+C to stop)"
    echo "Template changes are live — just reload the browser."
    fswatch -o -r --include='\.go$' --exclude='.*' . | while read; do
        echo "Go change detected, rebuilding..."
        kill $PID 2>/dev/null
        # Wait for port to be released
        while lsof -ti:$PORT >/dev/null 2>&1; do
            sleep 0.1
        done
        start_server
    done

# Clean build artifacts
clean:
    rm -f penpal

# Format code
fmt:
    go fmt ./...

# Run tests
test: test-go test-js

# Run Go tests
test-go:
    go test ./...

# Run JavaScript tests
test-js:
    node --test js/*_test.js

# Tidy dependencies
tidy:
    go mod tidy

# Install penpal as a Claude Code plugin (MCP server + skills)
install-claude:
    #!/usr/bin/env bash
    set -euo pipefail
    # Clean up legacy skill symlink if present
    rm -f ~/.claude/skills/monitor-reviews
    # Add the penpal directory as a local marketplace, then install the plugin
    claude plugin marketplace add "$(pwd)" 2>/dev/null || true
    claude plugin install penpal
    echo "Penpal plugin installed for Claude Code."

# Uninstall penpal Claude Code plugin
uninstall-claude:
    #!/usr/bin/env bash
    claude plugin uninstall penpal 2>/dev/null || true
    claude plugin marketplace remove penpal 2>/dev/null || true
    echo "Penpal plugin uninstalled."
