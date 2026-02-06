# Default recipe
default: run

# Build the binary
build:
    go build -o birdseye .

# Run the server and open browser
run: build
    #!/usr/bin/env bash
    ./birdseye &
    PID=$!

    # Wait for server to be ready
    echo "Waiting for server..."
    until curl -s http://localhost:8080/ > /dev/null 2>&1; do
        sleep 0.2
    done
    open "http://localhost:8080"

    wait $PID

# Development mode with hot reload
dev:
    #!/usr/bin/env bash
    # Install fswatch if needed
    if ! command -v fswatch &> /dev/null; then
        echo "Installing fswatch..."
        brew install fswatch
    fi

    PIDFILE=".birdseye.pid"

    # Kill previous birdseye dev server if running
    if [ -f "$PIDFILE" ]; then
        OLD_PID=$(cat "$PIDFILE")
        if kill -0 "$OLD_PID" 2>/dev/null; then
            echo "Stopping previous birdseye server (PID $OLD_PID)..."
            kill "$OLD_PID" 2>/dev/null
            sleep 0.5
        fi
        rm -f "$PIDFILE"
    fi

    # Find an available port starting from 8080
    PORT=8080
    while lsof -ti:$PORT >/dev/null 2>&1; do
        echo "Port $PORT is in use by another process, trying next..."
        PORT=$((PORT + 1))
    done

    start_server() {
        go build -o birdseye . && ./birdseye -dev -port $PORT &
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

    # Symlink skills for global availability
    mkdir -p ~/.claude/skills
    ln -sfn "$(pwd)/skills/monitor-reviews" ~/.claude/skills/monitor-reviews

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
    rm -f birdseye

# Format code
fmt:
    go fmt ./...

# Run tests
test:
    go test ./...

# Tidy dependencies
tidy:
    go mod tidy

# Install MCP server config globally for Claude
install-mcp:
    claude mcp add birdseye --scope user --transport http http://localhost:8080/mcp
