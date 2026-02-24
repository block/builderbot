# Default recipe
default: dev

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

# Development mode: Go server + Vite dev server, opens browser
dev:
    #!/usr/bin/env bash
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

    GO_PID=""
    VITE_PID=""
    cleanup() {
        echo "Stopping..."
        [ -n "$GO_PID" ] && kill $GO_PID 2>/dev/null
        [ -n "$VITE_PID" ] && kill $VITE_PID 2>/dev/null
        rm -f "$PIDFILE"
        exit 0
    }
    trap cleanup INT TERM

    # Build and start Go server
    go build -o penpal . && ./penpal -dev -port $PORT &
    GO_PID=$!
    echo $GO_PID > "$PIDFILE"

    # Wait for Go server to be ready
    echo "Waiting for Go server..."
    until curl -s http://localhost:$PORT/ > /dev/null 2>&1; do
        sleep 0.2
    done

    # Start Vite dev server
    cd frontend && npm run dev &
    VITE_PID=$!
    cd ..

    # Wait briefly for Vite to start, then open browser
    sleep 2
    open "http://localhost:5173"

    echo ""
    echo "Go server:    http://localhost:$PORT"
    echo "Vite (React): http://localhost:5173"
    echo ""

    wait $GO_PID

# Development mode: full Tauri desktop app with Vite HMR
dev-tauri: build-sidecar
    cd frontend && npm run tauri:dev

# Build Go sidecar binaries for Tauri
build-sidecar:
    ./scripts/build-sidecar.sh

# Build production Tauri app
build: ensure-deps build-sidecar
    cd frontend && npm install && npm run build && npm run tauri:build

# Install Penpal: build desktop app + install Claude Code plugin
install: build
    #!/usr/bin/env bash
    set -euo pipefail

    # Copy .app to /Applications
    APP_SRC="frontend/src-tauri/target/release/bundle/macos/Penpal.app"
    if [ -d "$APP_SRC" ]; then
        echo "Installing Penpal.app to /Applications..."
        rm -rf /Applications/Penpal.app
        cp -R "$APP_SRC" /Applications/Penpal.app
        echo "Penpal.app installed."
    else
        echo "Warning: $APP_SRC not found, skipping app install."
    fi

    # Install Claude Code plugin
    claude plugin uninstall birdseye 2>/dev/null || true
    claude plugin marketplace remove birdseye 2>/dev/null || true
    rm -f ~/.claude/skills/monitor-reviews
    claude plugin marketplace add "$(pwd)" 2>/dev/null || true
    claude plugin install penpal
    echo "Penpal Claude Code plugin installed."

# Uninstall Penpal
uninstall:
    #!/usr/bin/env bash
    rm -rf /Applications/Penpal.app 2>/dev/null || true
    echo "Penpal.app removed from /Applications."
    claude plugin uninstall penpal 2>/dev/null || true
    claude plugin marketplace remove penpal 2>/dev/null || true
    echo "Penpal Claude Code plugin uninstalled."

# Run all tests
test: test-go test-frontend test-js

# Run Go tests
test-go:
    go test ./...

# Run React frontend tests
test-frontend:
    cd frontend && npm run test:run

# Run legacy JavaScript tests
test-js:
    node --test js/*_test.js

# Run Playwright e2e tests
test-e2e:
    cd e2e && npx playwright test

# Clean build artifacts
clean:
    rm -f penpal
    rm -rf frontend/dist
    rm -rf frontend/src-tauri/target
    rm -rf frontend/src-tauri/binaries

# Format code
fmt:
    go fmt ./...

# Tidy dependencies
tidy:
    go mod tidy
