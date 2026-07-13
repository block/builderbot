#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app_root="$(cd "$script_dir/.." && pwd)"
lock_file="${ACP_TOOLS_LOCK_FILE:-$app_root/acp-tools.lock.json}"

# shellcheck source=scripts/lib/acp-node-wrapper.sh
source "$script_dir/lib/acp-node-wrapper.sh"

usage() {
  cat <<'USAGE'
Usage: scripts/ensure-acp-tools.sh [--target <target-triple>] [--print-bin-dir]

Installs the ACP bridge tools pinned in acp-tools.lock.json into the shared
Staged dev cache. The lockfile is target-specific; only entries matching the
requested target are prepared. Each tool is installed as a vendored npm
package tree with a small executable wrapper, validated against the locked
versions and integrity hashes.

Environment variables:
  ACP_TOOLS_LOCK_FILE    lockfile path (default: ./acp-tools.lock.json)
  ACP_TOOLS_CACHE_DIR    cache dir override
USAGE
}

default_cache_root() {
  if [[ -n "${XDG_CACHE_HOME:-}" ]]; then
    printf '%s/staged-dev/acp-tools\n' "$XDG_CACHE_HOME"
    return
  fi
  case "$(uname -s)" in
    Darwin) printf '%s/Library/Caches/staged-dev/acp-tools\n' "$HOME" ;;
    *) printf '%s/.cache/staged-dev/acp-tools\n' "$HOME" ;;
  esac
}

target=""
print_bin_dir=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      target="${2:-}"
      [[ -n "$target" ]] || { echo "--target requires a value" >&2; exit 1; }
      shift 2
      ;;
    --print-bin-dir)
      print_bin_dir=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$target" ]]; then
  target="$(rustc -vV | sed -n 's|host: ||p')"
fi
if [[ -z "$target" ]]; then
  echo "Could not determine rust host target. Pass --target explicitly." >&2
  exit 1
fi

cache_root="${ACP_TOOLS_CACHE_DIR:-$(default_cache_root)}"
bin_dir="$cache_root/bin/$target"

if [[ ! -f "$lock_file" ]]; then
  echo "ACP tools lockfile not found: $lock_file" >&2
  exit 1
fi

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Required tool missing: $1" >&2
    exit 1
  fi
}

require_tool node
require_tool npm

lock_entries="$(node - "$lock_file" "$target" <<'NODE'
const fs = require("node:fs");
const [lockFile, target] = process.argv.slice(2);
const data = JSON.parse(fs.readFileSync(lockFile, "utf8"));
const entries = (data.tools ?? []).filter((tool) => tool.target === target);
function requireString(entry, field) {
  if (typeof entry[field] !== "string" || entry[field].trim() === "") {
    throw new Error(`Invalid ACP tool lock entry for ${entry.id ?? "(unknown)"}: missing ${field}`);
  }
}
for (const entry of entries) {
  if (entry.source !== "npm") {
    throw new Error(`Invalid ACP tool lock entry for ${entry.id}: unsupported source ${entry.source}`);
  }
  for (const field of [
    "id",
    "binary",
    "target",
    "package",
    "version",
    "integrity",
    "tarball",
    "npmOs",
    "npmCpu",
    "dependencyPackage",
    "dependencyVersion",
    "dependencyIntegrity",
    "dependencyTarball",
    "nativePackage",
    "nativePackageName",
    "nativeVersion",
    "nativeIntegrity",
    "nativeTarball",
    "nativeExecutable",
  ]) {
    requireString(entry, field);
  }
}
process.stdout.write(JSON.stringify(entries));
NODE
)"

entry_count="$(node -e 'process.stdout.write(String(JSON.parse(process.argv[1]).length))' "$lock_entries")"
mkdir -p "$bin_dir"
if [[ "$entry_count" == "0" ]]; then
  find "$bin_dir" -type f -delete
  if [[ "$print_bin_dir" == "1" ]]; then
    printf '%s\n' "$bin_dir"
  else
    echo "No ACP tools locked for target $target."
  fi
  exit 0
fi

validate_npm_install() {
  local install_dir="$1"
  local package="$2"
  local version="$3"
  local integrity="$4"
  local dependency_package="$5"
  local dependency_version="$6"
  local dependency_integrity="$7"
  local native_package="$8"
  local native_package_name="$9"
  local native_version="${10}"
  local native_integrity="${11}"
  local native_executable="${12}"
  local claude_code_version="${13}"

  node - "$install_dir" "$package" "$version" "$integrity" "$dependency_package" "$dependency_version" "$dependency_integrity" "$native_package" "$native_package_name" "$native_version" "$native_integrity" "$native_executable" "$claude_code_version" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");

const [
  installDir,
  packageName,
  expectedVersion,
  expectedIntegrity,
  dependencyPackageName,
  expectedDependencyVersion,
  expectedDependencyIntegrity,
  nativePackageName,
  expectedNativePackageName,
  expectedNativeVersion,
  expectedNativeIntegrity,
  nativeExecutable,
  expectedClaudeCodeVersion,
] = process.argv.slice(2);

function packagePath(name, ...segments) {
  return path.join(installDir, "node_modules", ...name.split("/"), ...segments);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function assertEqual(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${expected}, got ${actual}`);
  }
}

function packageLockEntry(lock, packageName) {
  const suffix = `node_modules/${packageName}`;
  const match = Object.entries(lock.packages ?? {}).find(([key]) => key === suffix || key.endsWith(`/${suffix}`));
  if (!match) {
    throw new Error(`package-lock entry not found for ${packageName}`);
  }
  return match[1];
}

const packageJson = readJson(packagePath(packageName, "package.json"));
assertEqual(packageJson.name, packageName, `${packageName} name`);
assertEqual(packageJson.version, expectedVersion, `${packageName} version`);

const lock = readJson(path.join(installDir, "package-lock.json"));
assertEqual(packageLockEntry(lock, packageName).integrity, expectedIntegrity, `${packageName} integrity`);

const dependencyPackageJson = readJson(packagePath(dependencyPackageName, "package.json"));
assertEqual(
  dependencyPackageJson.name,
  dependencyPackageName,
  `${dependencyPackageName} name`,
);
assertEqual(
  dependencyPackageJson.version,
  expectedDependencyVersion,
  `${dependencyPackageName} version`,
);
if (expectedClaudeCodeVersion && expectedClaudeCodeVersion !== "null") {
  assertEqual(
    dependencyPackageJson.claudeCodeVersion,
    expectedClaudeCodeVersion,
    `${dependencyPackageName} claudeCodeVersion`,
  );
}
assertEqual(
  packageLockEntry(lock, dependencyPackageName).integrity,
  expectedDependencyIntegrity,
  `${dependencyPackageName} integrity`,
);

const nativePackageJson = readJson(packagePath(nativePackageName, "package.json"));
assertEqual(
  nativePackageJson.name,
  expectedNativePackageName,
  `${nativePackageName} package name`,
);
assertEqual(
  nativePackageJson.version,
  expectedNativeVersion,
  `${nativePackageName} version`,
);
fs.accessSync(packagePath(nativePackageName, nativeExecutable), fs.constants.X_OK);
assertEqual(
  packageLockEntry(lock, nativePackageName).integrity,
  expectedNativeIntegrity,
  `${nativePackageName} integrity`,
);
NODE
}

node -e '
const entries = JSON.parse(process.argv[1]);
for (const entry of entries) {
  console.log([
    entry.id,
    entry.binary,
    entry.package,
    entry.version,
    entry.integrity,
    entry.tarball,
    entry.npmOs,
    entry.npmCpu,
    entry.npmLibc ?? "",
    entry.nodeEngine ?? ">=22",
    entry.dependencyPackage,
    entry.dependencyVersion,
    entry.dependencyIntegrity,
    entry.dependencyTarball,
    entry.nativePackage,
    entry.nativePackageName,
    entry.nativeVersion,
    entry.nativeIntegrity,
    entry.nativeTarball,
    entry.nativeExecutable,
    entry.claudeCodeVersion ?? "",
  ].join("\x1f"));
}
' "$lock_entries" | while IFS=$'\x1f' read -r id binary package version integrity tarball npm_os npm_cpu npm_libc node_engine dependency_package dependency_version dependency_integrity dependency_tarball native_package native_package_name native_version native_integrity native_tarball native_executable claude_code_version; do
  [[ -n "$id" ]] || continue

  tool_dir="$cache_root/$target/$id/$version"
  install_dir="$tool_dir/npm"
  package_dir="$install_dir/node_modules/$package"
  entrypoint="$package_dir/dist/index.js"
  native_binary="$install_dir/node_modules/$native_package/$native_executable"
  staged_bin="$bin_dir/$binary"
  # The staged output is shared across lock versions, so its freshness stamp
  # must live next to it, not in the per-version tool_dir: a per-version stamp
  # stays self-consistent after a lock revert and would skip re-staging.
  stamp="$staged_bin.stamp"
  if [[ -x "$staged_bin" && -f "$stamp" && -f "$entrypoint" && -x "$native_binary" ]]; then
    # shellcheck disable=SC1090
    source "$stamp"
    if [[ "${STAMP_PACKAGE:-}" == "$package" && "${STAMP_VERSION:-}" == "$version" && "${STAMP_INTEGRITY:-}" == "$integrity" && "${STAMP_DEPENDENCY_PACKAGE:-}" == "$dependency_package" && "${STAMP_DEPENDENCY_VERSION:-}" == "$dependency_version" && "${STAMP_DEPENDENCY_INTEGRITY:-}" == "$dependency_integrity" && "${STAMP_NATIVE_PACKAGE:-}" == "$native_package" && "${STAMP_NATIVE_PACKAGE_NAME:-}" == "$native_package_name" && "${STAMP_NATIVE_VERSION:-}" == "$native_version" && "${STAMP_NATIVE_INTEGRITY:-}" == "$native_integrity" && "${STAMP_NATIVE_EXECUTABLE:-}" == "$native_executable" ]]; then
      continue
    fi
  fi

  echo "Installing ACP tool $id $version from npm for $target..." >&2
  rm -rf "$install_dir"
  mkdir -p "$install_dir" "$bin_dir"
  npm_args=(
    install
    --prefix "$install_dir"
    --omit=dev
    --include=optional
    --ignore-scripts
    --no-audit
    --no-fund
    --os "$npm_os"
    --cpu "$npm_cpu"
  )
  if [[ -n "$npm_libc" ]]; then
    npm_args+=(--libc "$npm_libc")
  fi
  npm_args+=("$package@$version")
  npm "${npm_args[@]}" >&2

  validate_npm_install "$install_dir" "$package" "$version" "$integrity" "$dependency_package" "$dependency_version" "$dependency_integrity" "$native_package" "$native_package_name" "$native_version" "$native_integrity" "$native_executable" "$claude_code_version"
  write_node_wrapper "$staged_bin" "$entrypoint" "$node_engine"
  {
    printf 'STAMP_TARGET=%q\n' "$target"
    printf 'STAMP_PACKAGE=%q\n' "$package"
    printf 'STAMP_VERSION=%q\n' "$version"
    printf 'STAMP_INTEGRITY=%q\n' "$integrity"
    printf 'STAMP_TARBALL=%q\n' "$tarball"
    printf 'STAMP_NPM_OS=%q\n' "$npm_os"
    printf 'STAMP_NPM_CPU=%q\n' "$npm_cpu"
    printf 'STAMP_NPM_LIBC=%q\n' "$npm_libc"
    printf 'STAMP_NODE_ENGINE=%q\n' "$node_engine"
    printf 'STAMP_DEPENDENCY_PACKAGE=%q\n' "$dependency_package"
    printf 'STAMP_DEPENDENCY_VERSION=%q\n' "$dependency_version"
    printf 'STAMP_DEPENDENCY_INTEGRITY=%q\n' "$dependency_integrity"
    printf 'STAMP_DEPENDENCY_TARBALL=%q\n' "$dependency_tarball"
    printf 'STAMP_CLAUDE_CODE_VERSION=%q\n' "$claude_code_version"
    printf 'STAMP_NATIVE_PACKAGE=%q\n' "$native_package"
    printf 'STAMP_NATIVE_PACKAGE_NAME=%q\n' "$native_package_name"
    printf 'STAMP_NATIVE_VERSION=%q\n' "$native_version"
    printf 'STAMP_NATIVE_INTEGRITY=%q\n' "$native_integrity"
    printf 'STAMP_NATIVE_TARBALL=%q\n' "$native_tarball"
    printf 'STAMP_NATIVE_EXECUTABLE=%q\n' "$native_executable"
    printf 'STAMP_BINARY=%q\n' "$binary"
  } > "$stamp"
done

# bin_dir is on the Goose search path, so binaries (and stamps) for tools no
# longer in the lock must be pruned, not just left behind.
locked_binaries="$(node -e '
const entries = JSON.parse(process.argv[1]);
for (const entry of entries) console.log(entry.binary);
' "$lock_entries" | sort -u)"
find "$bin_dir" -type f -print0 | while IFS= read -r -d '' staged_file; do
  name="$(basename "$staged_file")"
  if ! printf '%s\n' "$locked_binaries" | grep -Fxq -- "${name%.stamp}"; then
    rm -f -- "$staged_file"
  fi
done

if [[ "$print_bin_dir" == "1" ]]; then
  printf '%s\n' "$bin_dir"
fi
