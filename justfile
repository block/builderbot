# Default recipe
default: dev

export PATH := env_var_or_default("CARGO_HOME", env_var("HOME") + "/.cargo") + "/bin:" + env_var("PATH")

# Ensure required tools are installed
ensure-deps:
    #!/usr/bin/env bash
    if ! command -v go &> /dev/null; then
        echo "Installing Go..."
        brew install go
    fi
    if ! command -v cargo &> /dev/null; then
        echo "Installing Rust..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    fi
    if ! command -v claude &> /dev/null; then
        echo "Installing Claude Code..."
        brew install claude-code
    fi

# Build production desktop app
build: ensure-deps build-sidecar
    cd frontend && npm install && VITE_BASE=/ npm run build && npm run tauri:build

# Development mode: full desktop app with Vite HMR
dev: build-sidecar
    cd frontend && VITE_BASE=/ PENPAL_PORT=8082 npm run tauri:dev

# Build Go sidecar binaries for desktop app
build-sidecar:
    ./scripts/build-sidecar.sh

# Install Penpal: build desktop app + install Claude Code plugin
install: build
    #!/usr/bin/env bash
    set -euo pipefail

    # Quit running Penpal if present
    if pgrep -x Penpal >/dev/null 2>&1; then
        echo "Quitting Penpal..."
        osascript -e 'quit app "Penpal"' 2>/dev/null || true
        sleep 1
        # Force kill if it didn't quit gracefully
        pkill -x Penpal 2>/dev/null || true
    fi

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

    # Launch the app
    open /Applications/Penpal.app

# Uninstall Penpal
uninstall:
    #!/usr/bin/env bash
    rm -rf /Applications/Penpal.app 2>/dev/null || true
    echo "Penpal.app removed from /Applications."
    claude plugin uninstall penpal 2>/dev/null || true
    claude plugin marketplace remove penpal 2>/dev/null || true
    echo "Penpal Claude Code plugin uninstalled."

# Run all tests
test:
    go test ./... && cd frontend && npm run test:run

# Run Playwright e2e tests
test-e2e:
    cd e2e && npx playwright test

# Clean build artifacts
clean:
    rm -f penpal
    rm -rf frontend/dist
    rm -rf frontend/src-tauri/target
    rm -rf frontend/src-tauri/binaries

# Format and tidy
check:
    go fmt ./... && go mod tidy
