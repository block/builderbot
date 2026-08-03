//! Staged-managed ACP tool install locations and npm environment.
//!
//! Staged owns both sides of every npm-backed agent install: the managed Node
//! runtime (`managed_node`) supplies `node`/`npm`, and everything npm writes
//! lands in Staged-private directories under `~/.staged/packages` instead of
//! the host's global prefix. This module holds the path layout, the npm env
//! pairs that steer installs into the private prefix, and the PATH prepends
//! that make the results resolvable; the floating bridge installer and the
//! startup reconciler that build on these land next.
//!
//! Layout under `~/.staged/packages` (shared by every running Staged
//! instance — see the cross-process locking notes in `managed_node`):
//!
//! - `npm-prefix/` — the private npm global prefix the doctor crate's
//!   runtime `npm install -g` fixes (copilot, amp-acp) are steered into by
//!   the env pairs in [`managed_npm_env`].
//! - `node/<version>/<platform>/` — managed Node runtimes (`managed_node`).
//! - `tools/<id>/` — per-bridge npm `--prefix` trees for the managed ACP
//!   bridges (claude-acp, codex-acp).
//! - `bin/` — Staged-written shims for managed bridges.
//! - `state.json` — installed bridge versions + last reconcile outcome.
//!
//! `STAGED_ACP_TOOLS_DIR` stays honored as a dev/bridge-developer override:
//! when set, bridge management is disabled (no managed shim dir) so the
//! override dir is the one source of bridge binaries.

use std::path::{Path, PathBuf};

use crate::managed_node;

/// Dev/bridge-developer override (exported by `just dev`): a directory of
/// bridge binaries that replaces managed bridge resolution.
pub const ACP_TOOLS_DIR_ENV: &str = "STAGED_ACP_TOOLS_DIR";

/// Block's internal Artifactory npm registry. Direct access to
/// `registry.npmjs.org` is blocked by Cloudflare WARP on managed devices, so
/// npm-backed agent installs must route through this proxy. The doctor crate
/// exposes an optional `npm_registry` param but bakes in no registry of its
/// own, so Staged supplies this URL at every fix/run call site.
pub const BLOCK_NPM_REGISTRY_URL: &str =
    "https://global.block-artifacts.com/artifactory/api/npm/square-npm/";

/// The npm registry every Staged-run npm command routes through, or `None`
/// (npm's default public registry) for `no-block-npm-registry` builds.
pub fn npm_registry() -> Option<&'static str> {
    if cfg!(feature = "no-block-npm-registry") {
        None
    } else {
        Some(BLOCK_NPM_REGISTRY_URL)
    }
}

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
/// bridges from its own dir instead, and an unsupported target has no
/// managed runtime to install onto.
pub fn managed_tools_enabled() -> bool {
    managed_tools_enabled_from_parts(
        dev_tools_override_active(),
        managed_node::current_target_triple().is_some(),
    )
}

fn managed_tools_enabled_from_parts(override_active: bool, supported_target: bool) -> bool {
    !override_active && supported_target
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

fn shim_bin_dir(packages_root: &Path) -> PathBuf {
    packages_root.join("bin")
}

/// `<packages>/tools` — every managed bridge's npm `--prefix` tree lives
/// under here.
pub fn tools_root(packages_root: &Path) -> PathBuf {
    packages_root.join("tools")
}

/// `<packages>/tools/<id>` — the npm `--prefix` a managed bridge installs
/// into. Floating upgrades reuse the same prefix, so the entrypoint path a
/// shim points at is version-independent.
pub fn tool_install_dir(packages_root: &Path, id: &str) -> PathBuf {
    tools_root(packages_root).join(id)
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
/// makes npm's `#!/usr/bin/env node` shims (and, until the bundle flip, the
/// bundled bridge wrappers) run without host Node, and what resolves `npm`
/// itself for install fixes.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_registry_follows_registry_feature() {
        if cfg!(feature = "no-block-npm-registry") {
            assert_eq!(npm_registry(), None);
        } else {
            assert_eq!(npm_registry(), Some(BLOCK_NPM_REGISTRY_URL));
        }
    }

    #[test]
    fn path_helpers_lay_out_the_packages_tree() {
        let root = Path::new("/home/.staged/packages");
        assert_eq!(shim_bin_dir(root), root.join("bin"));
        assert_eq!(tools_root(root), root.join("tools"));
        assert_eq!(
            tool_install_dir(root, "claude-acp"),
            root.join("tools").join("claude-acp")
        );
        assert_eq!(state_path(root), root.join("state.json"));
    }

    #[test]
    fn managed_tools_require_no_override_and_a_supported_target() {
        assert!(managed_tools_enabled_from_parts(false, true));
        assert!(!managed_tools_enabled_from_parts(true, true));
        assert!(!managed_tools_enabled_from_parts(false, false));
    }

    #[test]
    fn managed_npm_env_points_every_pair_into_the_prefix() {
        let env = managed_npm_env_at(Path::new("/data/packages/npm-prefix"));
        let expect = |key: &str, value: &str| {
            assert_eq!(
                env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()),
                Some(value),
                "{key}"
            );
        };
        expect("NPM_CONFIG_PREFIX", "/data/packages/npm-prefix");
        expect("npm_config_prefix", "/data/packages/npm-prefix");
        expect("NPM_CONFIG_CACHE", "/data/packages/npm-prefix/cache");
        expect("npm_config_cache", "/data/packages/npm-prefix/cache");
        expect("COREPACK_HOME", "/data/packages/npm-prefix/corepack");
        assert_eq!(env.len(), 5);
    }

    #[test]
    fn apply_managed_npm_env_replaces_and_inserts() {
        let mut vars = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("NPM_CONFIG_PREFIX".to_string(), "/stray/prefix".to_string()),
        ];

        apply_managed_npm_env(
            &mut vars,
            &managed_npm_env_at(Path::new("/data/npm-prefix")),
        );

        assert_eq!(vars.len(), 6);
        assert_eq!(vars[0], ("PATH".to_string(), "/usr/bin".to_string()));
        assert_eq!(
            vars[1],
            (
                "NPM_CONFIG_PREFIX".to_string(),
                "/data/npm-prefix".to_string()
            )
        );
        assert!(vars
            .iter()
            .any(|(k, v)| k == "COREPACK_HOME" && v == "/data/npm-prefix/corepack"));
    }

    #[test]
    fn managed_prepend_dirs_orders_shims_then_prefix_then_node() {
        assert_eq!(
            managed_prepend_dirs_from_parts(
                None,
                Some(PathBuf::from("/data/packages/bin")),
                Some(PathBuf::from("/data/packages/npm-prefix/bin")),
                Some(PathBuf::from("/data/packages/node/v1/plat/bin")),
            ),
            vec![
                PathBuf::from("/data/packages/bin"),
                PathBuf::from("/data/packages/npm-prefix/bin"),
                PathBuf::from("/data/packages/node/v1/plat/bin"),
            ]
        );
        // The dev override replaces the managed shim dir and resolves first;
        // the prefix bin still resolves already-installed shims (host node
        // may run them).
        assert_eq!(
            managed_prepend_dirs_from_parts(
                Some(PathBuf::from("/dev/acp/bin")),
                None,
                Some(PathBuf::from("/data/packages/npm-prefix/bin")),
                None
            ),
            vec![
                PathBuf::from("/dev/acp/bin"),
                PathBuf::from("/data/packages/npm-prefix/bin"),
            ]
        );
    }

    #[test]
    fn npm_backed_commands_are_detected() {
        for command in [
            "npm install -g @github/copilot",
            "npm install -g amp-acp@latest --registry=https://example.test/npm/",
            "sh -c 'npm install -g @agentclientprotocol/claude-agent-acp'",
        ] {
            assert!(is_npm_backed_command(command), "{command}");
        }
        for command in [
            "curl -fsSL https://cursor.com/install | bash",
            "brew install --cask codex",
            "claude /login",
        ] {
            assert!(!is_npm_backed_command(command), "{command}");
        }
    }
}
