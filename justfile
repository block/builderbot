# Builderbot Monorepo
# Run `just setup` once after cloning.
#
# Common flows:
#   just dev            # Start Mark
#   just dev staged     # Start Staged
#   just app mark ci    # Run any app recipe
#   just check          # Full non-modifying checks

# Default: list available recipes
default:
    @just --list

# ============================================================================
# Setup
# ============================================================================

# First-time setup
setup:
    lefthook install
    pnpm install

# ============================================================================
# App Delegation
# ============================================================================

# List app workspaces that expose a justfile
apps:
    #!/usr/bin/env bash
    set -euo pipefail
    for dir in apps/*/; do
        [[ -f "$dir/justfile" ]] || continue
        basename "$dir"
    done

# Run a recipe in a specific app (e.g. `just app mark dev`)
app name recipe="dev" *args:
    #!/usr/bin/env bash
    set -euo pipefail

    app_justfile="apps/{{name}}/justfile"
    if [[ ! -f "$app_justfile" ]]; then
        echo "Unknown app '{{name}}'. Available apps:"
        for dir in apps/*/; do
            [[ -f "$dir/justfile" ]] || continue
            echo "  - $(basename "$dir")"
        done
        exit 1
    fi

    just -f "$app_justfile" {{recipe}} {{args}}

# Human-friendly shortcuts (supports `just dev staged` style)
dev app="mark" *args:
    just app {{app}} dev {{args}}

# Convenience aliases
mark recipe="dev" *args:
    just app mark {{recipe}} {{args}}

staged recipe="dev" *args:
    just app staged {{recipe}} {{args}}

# ============================================================================
# Cross-Cutting
# ============================================================================

# Format all apps + crates
fmt:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo fmt --all
    for dir in apps/*/; do
        [[ -f "$dir/justfile" ]] || continue
        recipes="$(just -f "$dir/justfile" --summary)"
        if echo "$recipes" | tr ' ' '\n' | grep -qx "fmt"; then
            just -f "$dir/justfile" fmt
        fi
    done

# Lint everything
lint:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy --workspace -- -D warnings
    for dir in apps/*/; do
        [[ -f "$dir/justfile" ]] || continue
        recipes="$(just -f "$dir/justfile" --summary)"
        if echo "$recipes" | tr ' ' '\n' | grep -qx "lint"; then
            just -f "$dir/justfile" lint
        fi
    done

# Verify everything without modifying files (CI-friendly)
check:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo fmt --all --check
    cargo clippy --workspace -- -D warnings
    for dir in apps/*/; do
        [[ -f "$dir/justfile" ]] || continue
        recipes="$(just -f "$dir/justfile" --summary)"
        if echo "$recipes" | tr ' ' '\n' | grep -qx "ci"; then
            just -f "$dir/justfile" ci
        elif echo "$recipes" | tr ' ' '\n' | grep -qx "check"; then
            just -f "$dir/justfile" check
        fi
    done
    cargo test --workspace

# Alias: many people expect `just ci`
ci: check

# ============================================================================
# Crates
# ============================================================================

# Build shared crates
build:
    cargo build

# Run all tests
test:
    cargo test --workspace

# Install the summarize binary
install-summarize:
    cargo install --path crates/summarize

# Run summarize directly (e.g. `just summarize --prompt "What?" src/`)
summarize *args:
    cargo run --release -p summarize -- {{args}}
