#!/usr/bin/env bash

set -euo pipefail

log() {
    printf '[mark-install] %s\n' "$1"
}

require_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        printf 'Error: required command not found: %s\n' "$1" >&2
        exit 1
    fi
}

copy_app() {
    local source_app="$1"
    local destination_app="$2"

    rm_with_optional_sudo "$destination_app"
    mkdir_with_optional_sudo "$(dirname "$destination_app")"
    copy_with_optional_sudo "$source_app" "$destination_app"
}

install_cli() {
    local source_cli="$1"
    local destination_cli="$2"

    mkdir_with_optional_sudo "$(dirname "$destination_cli")"
    install_with_optional_sudo "$source_cli" "$destination_cli"
}

mkdir_with_optional_sudo() {
    local dir="$1"
    mkdir -p "$dir" 2>/dev/null || sudo mkdir -p "$dir"
}

rm_with_optional_sudo() {
    local target="$1"
    if [ -e "$target" ]; then
        rm -rf "$target" 2>/dev/null || sudo rm -rf "$target"
    fi
}

copy_with_optional_sudo() {
    local source="$1"
    local destination="$2"
    cp -R "$source" "$destination" 2>/dev/null || sudo cp -R "$source" "$destination"
}

install_with_optional_sudo() {
    local source="$1"
    local destination="$2"
    install -m 0755 "$source" "$destination" 2>/dev/null || sudo install -m 0755 "$source" "$destination"
}

if [ "$(uname -s)" != "Darwin" ]; then
    echo "Error: mark installer currently supports macOS only." >&2
    exit 1
fi

require_cmd git
require_cmd bash

REPO_URL="${MARK_REPO_URL:-https://github.com/block/builderbot.git}"
REPO_REF="${MARK_REPO_REF:-main}"
SOURCE_DIR="${MARK_SOURCE_DIR:-}"
INSTALL_DIR="${MARK_INSTALL_DIR:-/Applications}"
CLI_DIR="${MARK_CLI_DIR:-/usr/local/bin}"
WORK_DIR="${MARK_WORK_DIR:-}"
KEEP_WORK_DIR="${MARK_KEEP_WORK_DIR:-0}"

TEMP_DIR=""
if [ -z "$SOURCE_DIR" ]; then
    if [ -n "$WORK_DIR" ]; then
        mkdir -p "$WORK_DIR"
        TEMP_DIR="$(cd "$WORK_DIR" && pwd)/builderbot-mark-install"
        rm -rf "$TEMP_DIR"
    else
        TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/builderbot-mark-install.XXXXXX")"
    fi

    cleanup() {
        if [ "$KEEP_WORK_DIR" = "1" ] || [ -z "$TEMP_DIR" ]; then
            return
        fi
        rm -rf "$TEMP_DIR" 2>/dev/null || sudo rm -rf "$TEMP_DIR" || true
    }
    trap cleanup EXIT

    log "cloning builderbot (${REPO_REF})"
    git clone --depth 1 --branch "$REPO_REF" "$REPO_URL" "$TEMP_DIR"
    SOURCE_DIR="$TEMP_DIR"
else
    SOURCE_DIR="$(cd "$SOURCE_DIR" && pwd)"
    log "using local source: $SOURCE_DIR"
fi

if [ ! -f "$SOURCE_DIR/bin/activate-hermit" ]; then
    echo "Error: could not find hermit activation script at $SOURCE_DIR/bin/activate-hermit" >&2
    exit 1
fi

if [ ! -f "$SOURCE_DIR/scripts/mark" ]; then
    echo "Error: could not find mark launcher script at $SOURCE_DIR/scripts/mark" >&2
    exit 1
fi

if [ ! -f "$SOURCE_DIR/apps/mark/justfile" ]; then
    echo "Error: could not find Mark app at $SOURCE_DIR/apps/mark" >&2
    exit 1
fi

cd "$SOURCE_DIR"

log "activating hermit"
# shellcheck disable=SC1091
source ./bin/activate-hermit

require_cmd just
require_cmd pnpm
require_cmd lefthook

log "installing git hooks"
lefthook install

log "installing dependencies"
pnpm install

log "building Mark"
just app mark build

APP_SOURCE="$SOURCE_DIR/apps/mark/src-tauri/target/release/bundle/macos/Mark.app"
CLI_SOURCE="$SOURCE_DIR/scripts/mark"
APP_DEST="$INSTALL_DIR/Mark.app"
CLI_DEST="$CLI_DIR/mark"

if [ ! -d "$APP_SOURCE" ]; then
    echo "Error: Mark.app was not produced at $APP_SOURCE" >&2
    exit 1
fi

log "installing app to $APP_DEST"
copy_app "$APP_SOURCE" "$APP_DEST"

log "installing CLI to $CLI_DEST"
install_cli "$CLI_SOURCE" "$CLI_DEST"

log "installation complete"
printf 'Run `%s` to launch Mark from your current directory.\n' "$(basename "$CLI_DEST")"
