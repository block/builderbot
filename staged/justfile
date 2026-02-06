# Staged - Git Diff Viewer

# Run the app in development mode (optionally point to another repo)
dev repo="": _ensure-deps
    #!/usr/bin/env bash
    set -euo pipefail
    # Pick a free port so multiple worktrees can run simultaneously
    VITE_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()')
    export VITE_PORT
    
    # Build Tauri config with dynamic port
    TAURI_CONFIG="{\"build\":{\"devUrl\":\"http://localhost:${VITE_PORT}\"}}"
    
    # Check if we're in a worktree and generate custom icon
    if git rev-parse --is-inside-work-tree &>/dev/null; then
        GIT_DIR=$(git rev-parse --git-dir)
        if [[ "$GIT_DIR" == *".git/worktrees/"* ]]; then
            # Get branch name and take only the last component (after final /)
            BRANCH_NAME=$(git rev-parse --abbrev-ref HEAD)
            WORKTREE_NAME="${BRANCH_NAME##*/}"
            echo "Worktree detected, branch: ${BRANCH_NAME}, label: ${WORKTREE_NAME}"
            
            # Generate dev icon with worktree label
            ICON_DIR="$(pwd)/src-tauri/target/dev-icons"
            mkdir -p "$ICON_DIR"
            DEV_ICON="$ICON_DIR/icon.icns"
            
            if swift scripts/generate-dev-icon.swift src-tauri/icons/icon.icns "$DEV_ICON" "$WORKTREE_NAME"; then
                echo "Generated dev icon with label: ${WORKTREE_NAME}"
                # Add icon override to Tauri config
                TAURI_CONFIG="{\"build\":{\"devUrl\":\"http://localhost:${VITE_PORT}\"},\"bundle\":{\"icon\":[\"$DEV_ICON\"]}}"
            else
                echo "Warning: Failed to generate dev icon, using default"
            fi
        fi
    fi
    
    echo "Starting dev server on port ${VITE_PORT}"
    {{ if repo != "" { "export STAGED_REPO=" + repo } else { "" } }}
    npx tauri dev --config "$TAURI_CONFIG"

# Build the app for production
build:
    npm run tauri:build

# Run just the frontend (for quick UI iteration)
frontend:
    npm run dev

# ============================================================================
# Code Quality
# ============================================================================

# Format all code (Rust + TypeScript/Svelte)
fmt:
    cd src-tauri && cargo fmt
    npx prettier --write "src/**/*.{ts,svelte,css,html}"

# Check formatting without modifying files
fmt-check:
    cd src-tauri && cargo fmt --check
    npx prettier --check "src/**/*.{ts,svelte,css,html}"

# Lint Rust code
lint:
    cd src-tauri && cargo clippy -- -D warnings

# Type check everything
typecheck:
    npm run check
    cd src-tauri && cargo check

# Run all checks (format, lint, typecheck) - use before submitting work
check-all: fmt-check lint typecheck

# ============================================================================
# Setup & Maintenance
# ============================================================================

# Install deps if needed (runs silently if already installed)
_ensure-deps:
    @[ -d node_modules/.package-lock.json ] || npm install
    @[ -d src-tauri/target/debug ] || (cd src-tauri && cargo fetch)

# Install dependencies
install:
    rustup default stable
    npm install
    lefthook install
    cd src-tauri && cargo fetch

# Clean build artifacts
clean:
    rm -rf dist
    rm -rf src-tauri/target
    rm -rf node_modules
