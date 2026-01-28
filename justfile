# Staged - Git Diff Viewer

# Run the app in development mode (optionally point to another repo)
dev repo="":
    {{ if repo != "" { "STAGED_REPO=" + repo + " " } else { "" } }}npm run tauri:dev

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

# Install dependencies
install:
    rustup default stable
    npm install
    cd src-tauri && cargo fetch

# Clean build artifacts
clean:
    rm -rf dist
    rm -rf src-tauri/target
    rm -rf node_modules
