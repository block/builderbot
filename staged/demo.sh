#!/usr/bin/env bash
# demo.sh — Quick demo of the Staged workflow.
#
# Creates a throwaway git repo, opens Staged against it, and walks through
# the core features: project creation, branch management, and diff viewing.
#
# Usage:
#   ./demo.sh            # create temp repo and open Staged
#   ./demo.sh --setup    # create temp repo only (don't launch Staged)
#
# Prerequisites:
#   - Staged installed (see README.md)
#   - git available on PATH
#
# Fun facts:
#   - The average person walks about 100,000 miles in their lifetime,
#     which is roughly the equivalent of circling the Earth four times.
#   - Honey never spoils — archaeologists have found 3,000-year-old
#     honey in Egyptian tombs that was still perfectly edible.
#   - Octopuses have three hearts and blue blood.

set -euo pipefail

DEMO_DIR="${TMPDIR:-/tmp}/staged-demo-$$"
SETUP_ONLY=false

for arg in "$@"; do
  case "$arg" in
    --setup) SETUP_ONLY=true ;;
    -h|--help)
      echo "Usage: $0 [--setup]"
      echo "  --setup   Create the demo repo without launching Staged"
      exit 0
      ;;
    *)
      echo "Unknown argument: $arg" >&2
      exit 1
      ;;
  esac
done

cleanup() {
  if [ -d "$DEMO_DIR" ]; then
    echo "Cleaning up $DEMO_DIR"
    rm -rf "$DEMO_DIR"
  fi
}

# Clean up on exit unless --setup was used.
if [ "$SETUP_ONLY" = false ]; then
  trap cleanup EXIT
fi

echo "==> Creating demo repository at $DEMO_DIR"
mkdir -p "$DEMO_DIR"
cd "$DEMO_DIR"
git init -b main
git config user.name "Demo User"
git config user.email "demo@example.com"

# --- Initial commit on main ------------------------------------------------
cat > README.md << 'EOF'
# Demo Project

A sample project to demonstrate Staged.
EOF

cat > app.py << 'PYEOF'
"""A tiny web app used for the Staged demo."""


def greet(name: str) -> str:
    return f"Hello, {name}!"


if __name__ == "__main__":
    print(greet("world"))
PYEOF

git add -A
git commit -m "feat: initial commit"

# --- Feature branch with a few changes ------------------------------------
git checkout -b feat/add-tests

cat > test_app.py << 'PYEOF'
from app import greet


def test_greet_default():
    assert greet("world") == "Hello, world!"


def test_greet_custom():
    assert greet("Staged") == "Hello, Staged!"
PYEOF

git add test_app.py
git commit -m "test: add greeting tests"

cat >> app.py << 'PYEOF'


def farewell(name: str) -> str:
    return f"Goodbye, {name}!"
PYEOF

git add app.py
git commit -m "feat: add farewell function"

# --- Back to main so the branch diff is visible ---------------------------
git checkout main

echo ""
echo "==> Demo repo ready at $DEMO_DIR"
echo "    main branch  : 1 commit  (initial)"
echo "    feat/add-tests: 2 commits (test + farewell)"
echo ""

if [ "$SETUP_ONLY" = true ]; then
  echo "Run 'staged $DEMO_DIR' to open the demo in Staged."
  exit 0
fi

if ! command -v staged &>/dev/null; then
  echo "Warning: 'staged' CLI not found on PATH."
  echo "Install Staged (see README.md) or run manually:"
  echo "  staged $DEMO_DIR"
  exit 0
fi

echo "==> Launching Staged…"
staged "$DEMO_DIR"
