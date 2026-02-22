# Builderbot Monorepo
# Run `just setup` once after cloning.

# Default: list available recipes
default:
    @just --list

# ── Setup ──────────────────────────────────────────────────

# First-time setup
setup:
    lefthook install
    pnpm install

# ── Per-App (delegate) ─────────────────────────────────────

# Run a just recipe in a specific app (e.g., just app mark dev)
app name *ARGS:
    just -f apps/{{name}}/justfile {{ARGS}}

# Shortcuts for the most common app commands
dev app="mark" *ARGS:
    just -f apps/{{app}}/justfile dev {{ARGS}}

# ── Cross-Cutting ──────────────────────────────────────────

# Format all apps + crates
fmt:
    cargo fmt --all
    for dir in apps/*/; do \
        [ -f "$dir/justfile" ] && just -f "$dir/justfile" fmt || true; \
    done

# Lint everything
lint:
    cargo clippy --workspace -- -D warnings
    for dir in apps/*/; do \
        [ -f "$dir/justfile" ] && just -f "$dir/justfile" lint || true; \
    done

# Check everything (what CI runs)
ci:
    for dir in apps/*/; do \
        [ -f "$dir/justfile" ] && just -f "$dir/justfile" ci || true; \
    done
    cargo test --workspace

# ── Crates ─────────────────────────────────────────────────

# Build shared crates
build:
    cargo build

# Run all tests
test:
    cargo test --workspace

# Install the summarize binary
install-summarize:
    cargo install --path crates/summarize

# Run summarize directly (e.g. just summarize --prompt "What?" src/)
summarize *ARGS:
    cargo run --release -p summarize -- {{ARGS}}
