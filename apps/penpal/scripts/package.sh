#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
DIST_DIR="$ROOT_DIR/dist"

# Read version from Cargo.toml
VERSION=$(grep '^version' "$ROOT_DIR/frontend/src-tauri/Cargo.toml" | head -1 | sed 's/version = "//;s/"//')

# Architecture: use argument if provided, otherwise detect host
ARCH="${1:-$(uname -m)}"
case "$ARCH" in
    aarch64|arm64)  ARCH="arm64" ;;
    x86_64|amd64)   ARCH="x86_64" ;;
    *)              echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

APP_SRC="$ROOT_DIR/frontend/src-tauri/target/release/bundle/macos/Penpal.app"
if [ ! -d "$APP_SRC" ]; then
    echo "Error: $APP_SRC not found. Run 'just build' first." >&2
    exit 1
fi

# Verify the binary architecture matches the requested arch
ACTUAL_ARCH=$(lipo -archs "$APP_SRC/Contents/MacOS/Penpal")
if [[ "$ACTUAL_ARCH" != *"$ARCH"* ]]; then
    echo "Error: Binary architecture mismatch. Requested '$ARCH' but binary contains '$ACTUAL_ARCH'." >&2
    exit 1
fi

mkdir -p "$DIST_DIR"
OUTPUT="$DIST_DIR/Penpal-${VERSION}-${ARCH}.zip"

echo "Packaging Penpal.app → $OUTPUT"
# ditto preserves resource forks and code signatures; --keepParent puts Penpal.app at the zip root
ditto -c -k --sequesterRsrc --keepParent "$APP_SRC" "$OUTPUT"

echo "Done: $OUTPUT ($(du -h "$OUTPUT" | cut -f1))"
