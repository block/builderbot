# Builderbot Monorepo
# Run `just setup` once after cloning to install git hooks.

# ============================================================================
# Setup
# ============================================================================

# First-time setup: install git hooks (run once at repo root)
setup:
    lefthook install

# ============================================================================
# Build
# ============================================================================

# Build all crates (debug)
build:
    cargo build

# Build all crates (release)
release:
    cargo build --release

# Run all tests
test:
    cargo test

# Format all workspace crates
fmt:
    cargo fmt --all

# Install the summarize binary to ~/.cargo/bin
install:
    cargo install --path crates/summarize

# ============================================================================
# Summarize
# ============================================================================

# Run summarize directly (e.g. just summarize --prompt "What?" src/)
summarize *ARGS:
    cargo run --release -p summarize -- {{ARGS}}
