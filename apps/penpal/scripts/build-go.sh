#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
OUT_DIR="$ROOT_DIR/frontend/src-tauri/binaries"

mkdir -p "$OUT_DIR"

echo "Building penpal-server for darwin/arm64..."
GOOS=darwin GOARCH=arm64 go build -o "$OUT_DIR/penpal-server-aarch64-apple-darwin" "$ROOT_DIR/cmd/penpal-server"

echo "Building penpal-server for darwin/amd64..."
GOOS=darwin GOARCH=amd64 go build -o "$OUT_DIR/penpal-server-x86_64-apple-darwin" "$ROOT_DIR/cmd/penpal-server"

echo "Building penpal-cli for darwin/arm64..."
GOOS=darwin GOARCH=arm64 go build -o "$OUT_DIR/penpal-cli-aarch64-apple-darwin" "$ROOT_DIR/cmd/penpal-cli"

echo "Building penpal-cli for darwin/amd64..."
GOOS=darwin GOARCH=amd64 go build -o "$OUT_DIR/penpal-cli-x86_64-apple-darwin" "$ROOT_DIR/cmd/penpal-cli"

echo "Done. Binaries:"
ls -la "$OUT_DIR"/penpal-*
