#!/usr/bin/env bash
# Build the stagedctl CLI and stage it where Tauri's `bundle.externalBin`
# expects sidecars: src-tauri/binaries/stagedctl-<target-triple>.
#
# tauri-build copies the staged file next to the app binary at cargo-build
# time (target/{debug,release} in dev, Contents/MacOS in the bundle) and
# fails the app build when it is missing — so this script must run before
# any cargo build of the app crate. It is wired into the justfile
# (`stage-sidecar`, a dependency of dev/lint/test) and into the tauri
# before-dev/before-build hooks, which pass the bundling target through
# TAURI_ENV_TARGET_TRIPLE. Pass an explicit triple as $1 to cross-stage.

set -euo pipefail

cd "$(dirname "$0")/.."

TARGET="${1:-${TAURI_ENV_TARGET_TRIPLE:-}}"
if [ -z "$TARGET" ]; then
    TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi
if [ -z "$TARGET" ]; then
    echo "Error: could not determine a target triple for the stagedctl sidecar." >&2
    exit 1
fi

(cd src-tauri && cargo build --release --package stagedctl --target "$TARGET")

# The target dir may be redirected (CARGO_TARGET_DIR); ask cargo where it is.
TARGET_DIR="$(cd src-tauri && cargo metadata --format-version 1 --no-deps |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"

BUILT="$TARGET_DIR/$TARGET/release/stagedctl"
if [ ! -f "$BUILT" ]; then
    echo "Error: stagedctl was not produced at $BUILT" >&2
    exit 1
fi

mkdir -p src-tauri/binaries
cp -f "$BUILT" "src-tauri/binaries/stagedctl-$TARGET"
echo "[stagedctl-sidecar] staged src-tauri/binaries/stagedctl-$TARGET"
