#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app_root="$(cd "$script_dir/.." && pwd)"
lock_file="${ACP_TOOLS_LOCK_FILE:-$app_root/acp-tools.lock.json}"

# shellcheck source=scripts/lib/acp-node-wrapper.sh
source "$script_dir/lib/acp-node-wrapper.sh"

usage() {
  cat <<'USAGE'
Usage: scripts/prepare-acp-tools-resource.sh [target-triple]

Stages the locked ACP bridge tools into src-tauri/resources/acp so Tauri can
bundle them as application resources: vendored npm package trees under
resources/acp/node and executable wrappers under resources/acp/bin. The
optional target triple defaults to the Rust host target.

Note: resources/acp/bin holds a single target at a time, so staging must stay
tied to the build target.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

target="${1:-}"
ensure_args=()
if [[ -n "$target" ]]; then
  ensure_args+=(--target "$target")
else
  target="$(rustc -vV | sed -n 's|host: ||p')"
fi
if [[ -z "$target" ]]; then
  echo "Could not determine rust host target." >&2
  exit 1
fi

cache_bin_dir="$("$script_dir/ensure-acp-tools.sh" ${ensure_args[@]+"${ensure_args[@]}"} --print-bin-dir)"
cache_root="$(dirname "$(dirname "$cache_bin_dir")")"
resource_root="$app_root/src-tauri/resources/acp"
resource_bin_dir="$resource_root/bin"
resource_node_dir="$resource_root/node"
mkdir -p "$resource_bin_dir"

# Keep .gitkeep but refresh any staged tools from the lock.
find "$resource_bin_dir" -type f ! -name ".gitkeep" -delete
rm -rf "$resource_node_dir"
mkdir -p "$resource_node_dir"

codesign_if_darwin() {
  local file="$1"
  if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
    codesign --force --sign - "$file" >/dev/null 2>&1 || true
  fi
}

while IFS=$'\t' read -r id binary package version node_engine; do
  [[ -n "$id" ]] || continue
  install_dir="$cache_root/$target/$id/$version/npm"
  entrypoint="$install_dir/node_modules/$package/dist/index.js"
  if [[ ! -f "$entrypoint" ]]; then
    echo "Locked npm ACP tool missing from cache: $package@$version" >&2
    exit 1
  fi
  resource_package_dir="$resource_node_dir/$id"
  mkdir -p "$resource_package_dir"
  cp -R "$install_dir/." "$resource_package_dir/"
  resource_entrypoint="$resource_package_dir/node_modules/$package/dist/index.js"
  if [[ ! -f "$resource_entrypoint" ]]; then
    echo "Failed to stage npm ACP tool: $package@$version" >&2
    exit 1
  fi
  write_node_wrapper "$resource_bin_dir/$binary" "../node/$id/node_modules/$package/dist/index.js" "$node_engine"
  while IFS= read -r native_binary; do
    [[ -n "$native_binary" ]] || continue
    codesign_if_darwin "$native_binary"
  done < <(find "$resource_package_dir" -type f \( -name claude -o -name codex \))
done < <(node - "$lock_file" "$target" <<'NODE'
const fs = require("node:fs");
const [lockFile, target] = process.argv.slice(2);
const data = JSON.parse(fs.readFileSync(lockFile, "utf8"));
for (const entry of data.tools ?? []) {
  if (entry.target !== target || typeof entry.binary !== "string") continue;
  if (entry.source !== "npm") {
    throw new Error(`Unsupported ACP tool source: ${entry.source}`);
  }
  console.log([entry.id, entry.binary, entry.package, entry.version, entry.nodeEngine ?? ">=22"].join("\t"));
}
NODE
)

echo "Staged ACP tools resource: $resource_bin_dir"
