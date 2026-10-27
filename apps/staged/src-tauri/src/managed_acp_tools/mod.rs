//! Staged-managed ACP tool installs.
//!
//! Staged owns both sides of every npm-backed agent install: the managed Node
//! runtime (`managed_node`) supplies `node`/`npm`, and everything npm writes
//! lands in Staged-private directories under `~/.staged/packages` instead of
//! the host's global prefix. Two install families live here:
//!
//! - **Private npm prefix** (`npm-prefix/`): the doctor crate's runtime
//!   `npm install -g` fixes (copilot, amp-acp) are steered here by the env
//!   pairs in [`managed_npm_env`].
//! - **Managed bridges** (`tools/<id>/` + `bin/`): the claude/codex ACP
//!   bridges in [`MANAGED_TOOLS`]. [`install_managed_tool`] installs — or
//!   upgrades — each to the latest published version with a floating
//!   `npm install <pkg>@latest --prefix` on the managed runtime, validates
//!   the staged native CLI before promotion, writes an absolute-path shim
//!   into `bin/` (no host `node` on PATH required), and
//!   records the installed version in `state.json`. The startup reconciler
//!   (`acp_tools_reconciler`) runs this for every managed bridge on launch,
//!   so a new bridge release ships to users the next time Staged starts.
//!
//! Layout under `~/.staged/packages` (shared by every running Staged
//! instance — see the cross-process locking notes in `managed_node`; every
//! mutation of the bridge trees below holds that flock on top of this
//! module's in-process tool-install mutex):
//!
//! - `npm-prefix/` — the private npm global prefix.
//! - `node/<version>/<platform>/` — managed Node runtimes (`managed_node`).
//! - `tools/<id>/` — per-bridge npm `--prefix` trees for the managed ACP
//!   bridges (claude-acp, codex-acp).
//! - `bin/` — Staged-written shims for managed bridges.
//! - `state.json` — installed bridge versions + last reconcile outcome.
//!
//! `STAGED_ACP_TOOLS_DIR` stays honored as a dev/bridge-developer override:
//! when set, bridge management is disabled (no managed tools, no shim dir,
//! no installs) so the override dir is the one source of bridge binaries.
//! The `no-managed-acp-tools` build feature compiles the managed bridge set
//! to empty for restricted builds — nothing installs, and the shim dir is
//! hidden from PATH prepends so shims another build left in the shared tree
//! cannot resolve.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::managed_node;

mod install;
mod probe;
mod reconcile;
mod registry;
mod validation;

#[cfg(test)]
#[path = "tests/support.rs"]
pub(crate) mod test_support;
#[cfg(test)]
#[path = "tests/layout.rs"]
mod tests;

pub use install::{install_managed_tool, InstallLineFn, ManagedToolError};
pub(crate) use probe::configured_npm_registry;
pub use probe::{installed_tool_version, latest_published_version};
pub(crate) use reconcile::{finish_reconcile, now_ms};
pub use registry::{fallback_npm_registry, npm_registry_override, NPM_REGISTRY_ENV};
pub(crate) use registry::{
    gateway_fallback_registry, gateway_retry_failed_message, install_failure_hint,
};

/// Dev/bridge-developer override: a directory of bridge binaries that
/// replaces managed bridge resolution.
pub const ACP_TOOLS_DIR_ENV: &str = "STAGED_ACP_TOOLS_DIR";

/// The `STAGED_ACP_TOOLS_DIR` override dir, when set and non-empty.
pub fn dev_tools_override_dir() -> Option<PathBuf> {
    std::env::var_os(ACP_TOOLS_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn dev_tools_override_active() -> bool {
    dev_tools_override_dir().is_some()
}

/// Whether this build manages ACP bridge installs (and therefore exposes the
/// managed shim dir): the `STAGED_ACP_TOOLS_DIR` dev override supplies
/// bridges from its own dir instead, the `no-managed-acp-tools` feature
/// compiles management out for restricted builds, and an unsupported target
/// has no managed runtime to install onto.
pub fn managed_tools_enabled() -> bool {
    managed_tools_enabled_from_parts(
        dev_tools_override_active(),
        cfg!(feature = "no-managed-acp-tools"),
        managed_node::current_target_triple().is_some(),
    )
}

fn managed_tools_enabled_from_parts(
    override_active: bool,
    managed_tools_disabled: bool,
    supported_target: bool,
) -> bool {
    !override_active && !managed_tools_disabled && supported_target
}

/// Why this build does not manage bridge installs, for Doctor to say so on
/// the Claude/Codex rows — a copy resolved from Homebrew is then by design,
/// not a failed install. `None` when management is on.
pub(crate) fn managed_tools_disabled_reason() -> Option<String> {
    managed_tools_disabled_reason_from_parts(
        dev_tools_override_dir().as_deref(),
        cfg!(feature = "no-managed-acp-tools"),
        managed_node::current_target_triple().is_some(),
    )
}

fn managed_tools_disabled_reason_from_parts(
    override_dir: Option<&Path>,
    managed_tools_disabled: bool,
    supported_target: bool,
) -> Option<String> {
    if let Some(dir) = override_dir {
        return Some(format!("{ACP_TOOLS_DIR_ENV} is set to {}", dir.display()));
    }
    if managed_tools_disabled {
        return Some("this build was compiled without managed ACP tools".to_string());
    }
    if !supported_target {
        return Some("no managed Node.js runtime is available for this platform".to_string());
    }
    None
}

/// The Staged-private npm global prefix, `~/.staged/packages/npm-prefix`.
pub fn npm_prefix_dir() -> Option<PathBuf> {
    crate::paths::packages_dir().map(|dir| dir.join("npm-prefix"))
}

/// Where npm writes global bin shims for the private prefix.
pub fn npm_prefix_bin_dir() -> Option<PathBuf> {
    npm_prefix_dir().map(|dir| dir.join("bin"))
}

/// `~/.staged/packages/bin` — the Staged-written shims for managed bridges.
/// `None` when this build does not manage bridges (see
/// [`managed_tools_enabled`]), so stale managed shims cannot resolve while
/// the `STAGED_ACP_TOOLS_DIR` dev override is active.
pub fn managed_shim_bin_dir() -> Option<PathBuf> {
    if !managed_tools_enabled() {
        return None;
    }
    crate::paths::packages_dir().map(|root| shim_bin_dir(&root))
}

pub(super) fn shim_bin_dir(packages_root: &Path) -> PathBuf {
    packages_root.join("bin")
}

/// `<packages>/tools` — every managed bridge's npm `--prefix` tree lives
/// under here.
pub fn tools_root(packages_root: &Path) -> PathBuf {
    packages_root.join("tools")
}

/// `<packages>/tools/<id>` — the live npm `--prefix` a managed bridge resolves
/// from. Floating upgrades stage a fresh tree beside it and swap it in
/// atomically (see [`install_managed_tool`]), so this path is stable and
/// version-independent — a shim can point at it once and never be rewritten.
pub fn tool_install_dir(packages_root: &Path, id: &str) -> PathBuf {
    tools_root(packages_root).join(id)
}

/// `<packages>/tools/<id>.staging` — the scratch `--prefix` a floating install
/// lands in before it is verified and atomically swapped into the live
/// [`tool_install_dir`]. A sibling of the live prefix so the swap is a single
/// same-filesystem rename.
pub(super) fn staging_install_dir(packages_root: &Path, id: &str) -> PathBuf {
    tools_root(packages_root).join(format!("{id}.staging"))
}

/// `<packages>/state.json` — installed bridge versions + the last reconcile
/// outcome.
pub fn state_path(packages_root: &Path) -> PathBuf {
    packages_root.join("state.json")
}

/// Directories to prepend (in order) wherever agent binaries must resolve:
/// the `STAGED_ACP_TOOLS_DIR` dev override when active (it replaces the
/// managed shim dir), then the managed bridge shims, the private prefix's
/// bin shims, and the managed Node runtime's bin dir — the latter is what
/// makes npm's `#!/usr/bin/env node` shims run without host Node, and what
/// resolves `npm` itself for install fixes.
pub fn managed_prepend_dirs() -> Vec<PathBuf> {
    managed_prepend_dirs_from_parts(
        dev_tools_override_dir(),
        managed_shim_bin_dir(),
        npm_prefix_bin_dir(),
        managed_node::managed_node_bin_dir(),
    )
}

fn managed_prepend_dirs_from_parts(
    override_bin: Option<PathBuf>,
    shim_bin: Option<PathBuf>,
    npm_prefix_bin: Option<PathBuf>,
    node_bin: Option<PathBuf>,
) -> Vec<PathBuf> {
    override_bin
        .into_iter()
        .chain(shim_bin)
        .chain(npm_prefix_bin)
        .chain(node_bin)
        .collect()
}

/// Env pairs steering every npm invocation Staged spawns into the private
/// prefix. Both spellings are set: npm canonically reads the lowercase
/// `npm_config_*` form, but tooling conventionally exports the uppercase one.
/// `sanitize_shell_env` already strips user-shell values for these keys from
/// captured snapshots, so these pairs are authoritative, not a race.
pub fn managed_npm_env() -> Vec<(String, String)> {
    npm_prefix_dir()
        .map(|prefix| managed_npm_env_at(&prefix))
        .unwrap_or_default()
}

pub fn managed_npm_env_at(prefix: &Path) -> Vec<(String, String)> {
    let prefix_value = prefix.to_string_lossy().into_owned();
    let cache_value = prefix.join("cache").to_string_lossy().into_owned();
    let corepack_value = prefix.join("corepack").to_string_lossy().into_owned();
    vec![
        ("NPM_CONFIG_PREFIX".to_string(), prefix_value.clone()),
        ("npm_config_prefix".to_string(), prefix_value),
        ("NPM_CONFIG_CACHE".to_string(), cache_value.clone()),
        ("npm_config_cache".to_string(), cache_value),
        ("COREPACK_HOME".to_string(), corepack_value),
    ]
}

/// Overlay the managed npm env onto an environment snapshot, replacing any
/// same-named entries so a stray inherited value can never win.
pub fn apply_managed_npm_env(vars: &mut Vec<(String, String)>, overrides: &[(String, String)]) {
    for (key, value) in overrides {
        match vars.iter_mut().find(|(existing, _)| existing == key) {
            Some(entry) => entry.1 = value.clone(),
            None => vars.push((key.clone(), value.clone())),
        }
    }
}

/// Whether a doctor fix command runs through npm — and therefore needs the
/// managed Node runtime installed first. Mirrors the doctor crate's (private)
/// npm-command predicate so the two stay in agreement about which commands
/// get registry/env treatment.
pub fn is_npm_backed_command(command: &str) -> bool {
    command.starts_with("npm ") || command.contains("npm install") || command.contains("npm view")
}

// =============================================================================
// The managed bridge set — installed and upgraded from the npm registry
// =============================================================================

/// A Staged-managed ACP bridge: installed and upgraded from the npm registry
/// at runtime (see [`install_managed_tool`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManagedTool {
    /// The install id (`tools/<id>` dir name, `state.json` key).
    pub id: &'static str,
    /// The bin name the shim is written under — the command name bridge
    /// resolution (`find_command`, provider discovery) looks up.
    pub binary: &'static str,
    /// The npm package installed from the registry.
    pub package: &'static str,
}

/// The ACP bridges Staged installs and upgrades on every launch. Both vendor
/// their agent's full CLI (Claude Code, `codex`) inside the npm package, so
/// no separate main-CLI install is needed.
/// Each entry must have a runtime probe in [`validation::validate_staged_tool`].
pub const MANAGED_TOOLS: &[ManagedTool] = &[
    ManagedTool {
        id: "claude-acp",
        binary: "claude-agent-acp",
        package: "@agentclientprotocol/claude-agent-acp",
    },
    ManagedTool {
        id: "codex-acp",
        binary: "codex-acp",
        package: "@agentclientprotocol/codex-acp",
    },
];

/// The managed bridges this build installs at runtime, or an empty list when
/// nothing is managed (see [`managed_tools_enabled`]).
pub fn managed_tools() -> Vec<ManagedTool> {
    if !managed_tools_enabled() {
        return Vec::new();
    }
    MANAGED_TOOLS.to_vec()
}

/// The managed bridge with this install id, when this build manages it.
pub fn managed_tool(id: &str) -> Option<ManagedTool> {
    managed_tools().into_iter().find(|tool| tool.id == id)
}

// =============================================================================
// state.json — installed versions + last reconcile result
// =============================================================================

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManagedToolsState {
    pub tools: BTreeMap<String, InstalledToolPin>,
    pub last_reconcile: Option<ReconcileRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledToolPin {
    pub binary: String,
    /// The version npm resolved for `<pkg>@latest`, recorded for the
    /// reconcile log and future doctor readouts. Empty when the installed
    /// `package.json` could not be read.
    pub version: String,
    /// The managed Node.js runtime version the shim execs by absolute path.
    /// After a Node pin bump this trails the embedded pin until the bridge
    /// reinstalls and its shim is rewritten — which is why superseded
    /// runtimes are pruned only after a fully-successful reconcile.
    #[serde(default)]
    pub node_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileRecord {
    pub at_ms: u64,
    pub ok: bool,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Read `<packages>/state.json`; a missing or corrupt file is an empty state.
pub(crate) fn read_state(packages_root: &Path) -> ManagedToolsState {
    std::fs::read_to_string(state_path(packages_root))
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub(super) fn write_state(packages_root: &Path, state: &ManagedToolsState) -> std::io::Result<()> {
    std::fs::create_dir_all(packages_root)?;
    let path = state_path(packages_root);
    let temp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(&temp, format!("{json}\n"))?;
    std::fs::rename(&temp, &path)
}
