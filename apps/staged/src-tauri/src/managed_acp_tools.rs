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

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::AsyncBufReadExt;

use crate::managed_node;

mod validation;

/// Dev/bridge-developer override: a directory of bridge binaries that
/// replaces managed bridge resolution.
pub const ACP_TOOLS_DIR_ENV: &str = "STAGED_ACP_TOOLS_DIR";

// -----------------------------------------------------------------------------
// npm registry policy
//
// Staged passes no `--registry` of its own: npm resolves the registry from
// its normal config chain (`NPM_CONFIG_REGISTRY`, `~/.npmrc` including
// `@scope:registry` entries, then `registry.npmjs.org`). That keeps a
// corporate laptop whose `~/.npmrc` already points at an internal mirror —
// with whatever auth token it needs — on that mirror, and gives everyone
// else their own registry; a hardcoded URL served exactly one network and
// could not even override a scoped `~/.npmrc` entry.
//
// Two escape hatches sit around that default. `STAGED_NPM_REGISTRY` is an
// explicit override passed as `--registry` to every Staged-run npm command.
// `STAGED_FALLBACK_NPM_REGISTRY` is a *build-time* registry (compiled in via
// `option_env!`, never written into source) for the one population npm's own
// config fails: Block laptops with no `~/.npmrc` behind the Cloudflare
// Gateway, where `registry.npmjs.org` answers with a redirect to an HTML
// block page and npm reports `FETCH_ERROR`/invalid JSON. Only a failure that
// reads as that block page is retried through the fallback — a 404 from a
// mirror, a dead host, or an unresolvable version is not a registry problem a
// different registry can fix, and an explicit override is never second-
// guessed.
// -----------------------------------------------------------------------------

/// Explicit npm registry override, passed as `--registry` when set.
pub const NPM_REGISTRY_ENV: &str = "STAGED_NPM_REGISTRY";

/// The `STAGED_NPM_REGISTRY` override, trimmed, when set and non-empty.
/// `None` means npm decides from its own config.
pub fn npm_registry_override() -> Option<String> {
    registry_from_env_value(std::env::var(NPM_REGISTRY_ENV).ok().as_deref())
}

fn registry_from_env_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// The registry compiled in through `STAGED_FALLBACK_NPM_REGISTRY` at build
/// time, retried once when npm's configured registry turns out to be
/// gateway-blocked (see the policy note above). `None` for builds that set
/// no such variable.
pub fn fallback_npm_registry() -> Option<&'static str> {
    option_env!("STAGED_FALLBACK_NPM_REGISTRY")
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// How an npm command names its registry in errors and progress lines.
fn registry_label(registry: Option<&str>) -> String {
    registry
        .map(str::to_string)
        .unwrap_or_else(|| "npm default (per npm config)".to_string())
}

/// What a failed npm command's stderr says about why it failed, as far as
/// the registry retry needs to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NpmFailure {
    /// A network gateway answered for `registry.npmjs.org` with a block page
    /// instead of registry JSON — Cloudflare Gateway on managed Block laptops
    /// redirects to `blocked.teams.cloudflare.com`, and npm surfaces the HTML
    /// as `FETCH_ERROR` or a JSON parse error for a request it made to the
    /// public registry.
    GatewayBlocked,
    Other,
}

/// How npm names `registry.npmjs.org` as the host it *requested* — minipass-
/// fetch's `request to <url>` / `invalid json response body at <url>` and
/// npm's `registry: <url>` detail line. A bare `registry.npmjs.org` anywhere
/// in stderr is not enough: a mirror's own error body mentions its upstream
/// ("failed to fetch from registry.npmjs.org") when *it* could not reach it,
/// and that is the mirror failing, not a gateway blocking the public registry.
const NPMJS_REQUEST_MARKERS: [&str; 3] = [
    "request to https://registry.npmjs.org/",
    "response body at https://registry.npmjs.org/",
    "registry: https://registry.npmjs.org",
];

/// Gateway-blocked iff stderr names the block page itself, or a non-JSON /
/// fetch-level failure (`FETCH_ERROR`, `EJSONPARSE`, "invalid json",
/// "Unexpected token") happened on a request npm made to
/// `registry.npmjs.org`. `FETCH_ERROR` is minipass-fetch's catch-all for a
/// fetch that died without a system errno, so a truncated body from the
/// public registry classifies as blocked too — that costs one cheap extra
/// attempt through the fallback, which then reports its own error.
pub(crate) fn classify_npm_failure(stderr: &str) -> NpmFailure {
    if stderr.contains("blocked.teams.cloudflare.com") {
        return NpmFailure::GatewayBlocked;
    }
    let requested_npmjs = NPMJS_REQUEST_MARKERS
        .iter()
        .any(|marker| stderr.contains(marker));
    if requested_npmjs {
        let lower = stderr.to_ascii_lowercase();
        if stderr.contains("FETCH_ERROR")
            || stderr.contains("EJSONPARSE")
            || lower.contains("unexpected token")
            || lower.contains("invalid json")
        {
            return NpmFailure::GatewayBlocked;
        }
    }
    NpmFailure::Other
}

/// The registry to retry a failed npm command through, if any: only when no
/// explicit override was given, a build-time fallback exists, and `stderr`
/// reads as a gateway block of `registry.npmjs.org`.
pub(crate) fn gateway_fallback_registry<'a>(
    override_registry: Option<&str>,
    fallback: Option<&'a str>,
    stderr: &str,
) -> Option<&'a str> {
    if override_registry.is_some() {
        return None;
    }
    let fallback = fallback?;
    (classify_npm_failure(stderr) == NpmFailure::GatewayBlocked).then_some(fallback)
}

/// The error to surface when the retry through `fallback` failed as well:
/// keeps the original diagnosis (npm's default registry was gateway-blocked)
/// in front of the retry's own error, so the surfaced message explains why a
/// registry the user never configured was contacted at all.
pub(crate) fn gateway_retry_failed_message(fallback: &str, retry_error: &str) -> String {
    format!(
        "npm default registry (registry.npmjs.org) was blocked by a network gateway; \
         fallback {fallback} also failed: {retry_error}"
    )
}

/// What a recorded managed-install error says the user can change, as far
/// as the error text tells. Coarser than [`NpmFailure`] on purpose: this
/// classifies the *recorded* error (`state.json`'s `lastReconcile.errors`,
/// the Node download error), which carries npm's `error code` block via
/// [`npm_error_summary`] rather than the full stderr, and it also covers the
/// Node.js runtime download.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallFailureHint {
    /// `registry.npmjs.org` answered with a network gateway's block page and
    /// npm had no other registry configured.
    GatewayBlocked,
    /// The registry npm used answered 404 for the package.
    PackageMissing,
    /// The registry or download host could not be reached at all: DNS,
    /// refused/reset connections, or a timeout on the request.
    HostUnreachable,
}

/// Classify a recorded install error (see [`InstallFailureHint`]). `None`
/// when the text names no network or registry cause Doctor has advice for —
/// a validation failure, a missing entrypoint, a disk error.
pub(crate) fn classify_install_failure(error: &str) -> Option<InstallFailureHint> {
    if error.contains("blocked by a network gateway")
        || classify_npm_failure(error) == NpmFailure::GatewayBlocked
    {
        return Some(InstallFailureHint::GatewayBlocked);
    }
    if error.contains("E404") || error.contains("404 Not Found") {
        return Some(InstallFailureHint::PackageMissing);
    }
    const UNREACHABLE_MARKERS: [&str; 11] = [
        "ENOTFOUND",
        "ECONNREFUSED",
        "ECONNRESET",
        "ETIMEDOUT",
        "EAI_AGAIN",
        "EHOSTUNREACH",
        "ENETUNREACH",
        // reqwest's wording for the Node.js runtime download.
        "error sending request",
        "dns error",
        "connection refused",
        "operation timed out",
    ];
    if UNREACHABLE_MARKERS
        .iter()
        .any(|marker| error.contains(marker))
    {
        return Some(InstallFailureHint::HostUnreachable);
    }
    // The managed `npm install` itself timing out names its registry; a
    // validation probe timing out does not, and is not a network problem.
    if error.contains("timed out after") && error.contains("(registry:") {
        return Some(InstallFailureHint::HostUnreachable);
    }
    None
}

/// The one-line hint Doctor shows under a classified install failure, naming
/// what to change: `package` is the npm package the install was for.
pub(crate) fn install_failure_hint(error: &str, package: &str) -> Option<String> {
    Some(match classify_install_failure(error)? {
        InstallFailureHint::GatewayBlocked => {
            "this network blocks registry.npmjs.org and npm has no registry configured — \
             add `registry=<mirror>` to ~/.npmrc or set STAGED_NPM_REGISTRY"
                .to_string()
        }
        InstallFailureHint::PackageMissing => {
            format!("the configured registry does not carry {package}")
        }
        InstallFailureHint::HostUnreachable => {
            "the configured registry/host did not respond — check `registry=` in ~/.npmrc \
             or STAGED_NODE_DIST_URL"
                .to_string()
        }
    })
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

fn shim_bin_dir(packages_root: &Path) -> PathBuf {
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
fn staging_install_dir(packages_root: &Path, id: &str) -> PathBuf {
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

fn write_state(packages_root: &Path, state: &ManagedToolsState) -> std::io::Result<()> {
    std::fs::create_dir_all(packages_root)?;
    let path = state_path(packages_root);
    let temp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(&temp, format!("{json}\n"))?;
    std::fs::rename(&temp, &path)
}

// =============================================================================
// install_managed_tool — floating npm install + shim + state
// =============================================================================

/// Floating bridge installs download ~70-95 MB of packages through the
/// registry; a hung npm must not wedge the install mutex forever.
const NPM_INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug)]
pub enum ManagedToolError {
    DataDir(String),
    /// This id is not managed on this build/target; callers route before
    /// installing, so surfacing one means the managed set changed under a
    /// running operation.
    NotManaged(String),
    Node(managed_node::ManagedNodeError),
    NpmInstall(String),
    /// npm exited cleanly but the staged bridge failed install validation.
    Incomplete(String),
    Io(String),
}

impl std::fmt::Display for ManagedToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DataDir(message) => {
                write!(
                    f,
                    "failed to resolve the managed ACP tools directory: {message}"
                )
            }
            Self::NotManaged(message) => {
                write!(f, "not a Staged-managed ACP bridge: {message}")
            }
            Self::Node(error) => error.fmt(f),
            Self::NpmInstall(message) => write!(f, "npm install failed: {message}"),
            Self::Incomplete(message) => {
                write!(f, "installed ACP bridge is incomplete: {message}")
            }
            Self::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ManagedToolError {}

/// Receives every progress/output line of an install. Doctor fixes and the
/// startup reconciler feed these to the log with their own prefixes; there is
/// no streamed-output UI channel for installs.
pub type InstallLineFn<'a> = dyn Fn(&str) + Send + Sync + 'a;

fn tool_install_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Install (or upgrade) one managed bridge to the latest published version:
/// ensure the managed Node runtime, run the floating `npm install
/// <pkg>@latest` into a scratch prefix, swap the verified tree into the live
/// prefix, write the absolute-path shim, and record the installed version in
/// `state.json`. Safe to call concurrently — doctor fixes and the startup
/// reconciler serialize on one process-wide install mutex, and mutations of
/// the shared packages tree additionally hold the cross-process flock (other
/// Staged instances reconcile the same `~/.staged/packages`). A failed
/// install — including one npm aborts mid-reify, or an upstream `@latest`
/// that drops the entrypoint — leaves any previously installed version fully
/// in place, since the live tree is only ever replaced by an atomic swap of a
/// verified staging tree; the Node runtime its shim execs is likewise kept,
/// as superseded runtimes are pruned only after a fully-successful reconcile.
/// So an offline or partial launch never removes a working bridge.
///
/// `env_vars` is the sanitized login-shell snapshot doctor's checks and the
/// freshness probe run under ([`crate::doctor::doctor_env_vars`]); npm runs
/// under exactly that, so the registry config it resolves (`~/.npmrc`,
/// `NPM_CONFIG_REGISTRY` from the shell rc) is the one the probe saw.
pub async fn install_managed_tool(
    id: &str,
    env_vars: &[(String, String)],
    on_line: &InstallLineFn<'_>,
) -> Result<(), ManagedToolError> {
    let tool = managed_tool(id).ok_or_else(|| {
        ManagedToolError::NotManaged(format!("'{id}' is not a Staged-managed ACP bridge"))
    })?;
    let packages_root = crate::paths::packages_dir()
        .ok_or_else(|| ManagedToolError::DataDir("home directory is unavailable".to_string()))?;
    let node_root = managed_node::managed_node_root()
        .ok_or_else(|| ManagedToolError::DataDir("home directory is unavailable".to_string()))?;
    let node_install_dir = managed_node::pinned_install_dir(&node_root).ok_or_else(|| {
        ManagedToolError::NotManaged("no managed Node.js runtime pin for this target".to_string())
    })?;

    let _guard = tool_install_lock().lock().await;
    managed_node::ensure_managed_node_runtime()
        .await
        .map_err(ManagedToolError::Node)?;
    // The cross-process lock is taken only after the runtime ensure has
    // released the same flock — nesting the two would deadlock (see
    // `lock_packages_dir`). In the unlocked window another process can at
    // most install or prune, and both leave the pinned runtime in place.
    let _packages_lock = managed_node::lock_packages_dir(&packages_root)
        .await
        .map_err(ManagedToolError::Node)?;
    install_npm_tool(
        &packages_root,
        &node_install_dir,
        &managed_node::node_runtime_lock().version,
        &tool,
        env_vars,
        npm_registry_override().as_deref(),
        fallback_npm_registry(),
        validation::VALIDATION_TIMEOUT,
        on_line,
    )
    .await
}

/// The install body, path-parameterized so tests drive it with a fixture
/// `npm`. Caller holds the install mutex + packages flock and has ensured
/// the runtime. npm runs under the `env_vars` snapshot; `registry` is the
/// explicit override (or `None` for npm's own config) and `fallback_registry`
/// the build-time fallback tried once if the first attempt fails as
/// gateway-blocked (see the registry policy note).
#[allow(clippy::too_many_arguments)]
async fn install_npm_tool(
    packages_root: &Path,
    node_install_dir: &Path,
    node_version: &str,
    tool: &ManagedTool,
    env_vars: &[(String, String)],
    registry: Option<&str>,
    fallback_registry: Option<&str>,
    validation_timeout: Duration,
    on_line: &InstallLineFn<'_>,
) -> Result<(), ManagedToolError> {
    let install_dir = tool_install_dir(packages_root, tool.id);
    // Stage the floating install into a scratch prefix and swap it into the
    // live tree only after entrypoint and runtime validation pass. npm reifies in
    // place, so installing straight into `install_dir` would let a failure
    // mid-reify — or an upstream `@latest` that drops the entrypoint — replace
    // the live tree the (version-independent) shim already points at, breaking
    // the previously working bridge. Staging keeps the old tree untouched
    // until a verified new one is ready to swap in atomically.
    let staging_dir = staging_install_dir(packages_root, tool.id);
    reset_dir(&staging_dir)
        .map_err(|error| ManagedToolError::Io(format!("prepare staging dir: {error}")))?;

    on_line(&format!(
        "Installing {}@latest into ~/.staged/packages",
        tool.package
    ));
    if let Err(failure) = run_floating_npm_install(
        packages_root,
        node_install_dir,
        &staging_dir,
        tool,
        env_vars,
        registry,
        on_line,
    )
    .await
    {
        let Some(fallback) =
            gateway_fallback_registry(registry, fallback_registry, &failure.stderr)
        else {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(failure.error);
        };
        on_line(&format!(
            "npm could not reach registry.npmjs.org (network gateway block page); retrying through {fallback}"
        ));
        // A fresh scratch prefix: whatever the blocked attempt reified must
        // not seed the retry.
        if let Err(error) = reset_dir(&staging_dir) {
            let _ = std::fs::remove_dir_all(&staging_dir);
            return Err(ManagedToolError::Io(format!(
                "prepare staging dir for registry retry: {error}"
            )));
        }
        if let Err(failure) = run_floating_npm_install(
            packages_root,
            node_install_dir,
            &staging_dir,
            tool,
            env_vars,
            Some(fallback),
            on_line,
        )
        .await
        {
            let _ = std::fs::remove_dir_all(&staging_dir);
            // The retry's own error is an `NpmInstall` too; its bare message
            // goes inside so the prefix is not doubled.
            let retry_error = match &failure.error {
                ManagedToolError::NpmInstall(message) => message.clone(),
                other => other.to_string(),
            };
            return Err(ManagedToolError::NpmInstall(gateway_retry_failed_message(
                fallback,
                &retry_error,
            )));
        }
    }

    let staged_entrypoint = npm_entrypoint(&staging_dir, tool.package);
    if !staged_entrypoint.is_file() {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(ManagedToolError::Incomplete(format!(
            "{}: bridge entrypoint {} is missing after install",
            tool.package,
            staged_entrypoint.display()
        )));
    }
    on_line(&format!("Validating {} before promotion", tool.package));
    if let Err(error) =
        validation::validate_staged_tool(node_install_dir, &staging_dir, tool, validation_timeout)
            .await
    {
        let _ = std::fs::remove_dir_all(&staging_dir);
        return Err(error);
    }
    let version = installed_version(&staging_dir, tool.package).unwrap_or_default();

    // Atomically replace the live tree with the verified staging tree, keeping
    // the previous tree aside as `.old` to roll back to if the rename fails.
    swap_into_place(&staging_dir, &install_dir)
        .map_err(|error| ManagedToolError::Io(format!("install staged bridge: {error}")))?;

    let entrypoint = npm_entrypoint(&install_dir, tool.package);
    write_shim(
        &shim_bin_dir(packages_root),
        tool.binary,
        &shim_contents(&node_binary(node_install_dir), &entrypoint),
    )
    .map_err(|error| ManagedToolError::Io(format!("write bridge shim: {error}")))?;

    let mut state = read_state(packages_root);
    state.tools.insert(
        tool.id.to_string(),
        InstalledToolPin {
            binary: tool.binary.to_string(),
            version: version.clone(),
            node_version: node_version.to_string(),
        },
    );
    write_state(packages_root, &state)
        .map_err(|error| ManagedToolError::Io(format!("write state.json: {error}")))?;
    on_line(&format!(
        "{}@{} is ready",
        tool.package,
        if version.is_empty() {
            "latest"
        } else {
            version.as_str()
        }
    ));
    Ok(())
}

/// Remove `dir` if it exists, then recreate it empty — a clean scratch prefix
/// for a fresh floating install (a stale staging tree from a crashed run must
/// not seed the next one).
fn reset_dir(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)
}

/// Atomically replace `final_dir` with `staging_dir`: stage any previous tree
/// aside as `<final_dir>.old`, rename the verified staging tree into place,
/// and roll the previous tree back if that rename fails. Mirrors
/// `managed_node`'s runtime swap; both dirs are siblings under `tools/`, so
/// each rename is a single same-filesystem operation.
fn swap_into_place(staging_dir: &Path, final_dir: &Path) -> std::io::Result<()> {
    if let Some(parent) = final_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let old_dir = final_dir.with_extension("old");
    if old_dir.exists() {
        std::fs::remove_dir_all(&old_dir)?;
    }
    if final_dir.exists() {
        std::fs::rename(final_dir, &old_dir)?;
    }
    if let Err(error) = std::fs::rename(staging_dir, final_dir) {
        if old_dir.exists() {
            let _ = std::fs::rename(&old_dir, final_dir);
        }
        let _ = std::fs::remove_dir_all(staging_dir);
        return Err(error);
    }
    let _ = std::fs::remove_dir_all(&old_dir);
    Ok(())
}

/// `<install-dir>/node_modules/<package>/dist/index.js` — the bridge
/// entrypoint convention both managed bridges follow.
fn npm_entrypoint(install_dir: &Path, package: &str) -> PathBuf {
    package_dir(install_dir, package)
        .join("dist")
        .join("index.js")
}

fn package_dir(install_dir: &Path, package: &str) -> PathBuf {
    package
        .split('/')
        .fold(install_dir.join("node_modules"), |dir, part| dir.join(part))
}

fn node_binary(node_install_dir: &Path) -> PathBuf {
    node_install_dir.join("bin").join("node")
}

/// The version npm resolved for the just-installed package, from its
/// `package.json`. Best-effort: the state record is informational, so an
/// unreadable version does not fail the install.
fn installed_version(install_dir: &Path, package: &str) -> Option<String> {
    let json =
        std::fs::read_to_string(package_dir(install_dir, package).join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// A failed floating install: the error to surface plus what npm wrote to
/// stderr, kept so the caller can classify the failure for a registry retry.
struct NpmInstallFailure {
    error: ManagedToolError,
    stderr: String,
}

/// How much of npm's stderr is kept for the retry classifier. npm writes its
/// `npm error code …` block *last*, after any deprecation and peer warnings,
/// so the capture keeps the newest lines (see [`push_captured_line`]) — a
/// warning-heavy install must not push the error block out of view.
const NPM_STDERR_CAPTURE_LIMIT: usize = 64 * 1024;

/// Append `line` to the capture, dropping whole lines from the *front* once
/// it outgrows [`NPM_STDERR_CAPTURE_LIMIT`] (a single oversized line keeps
/// its tail).
fn push_captured_line(capture: &mut String, line: &str) {
    capture.push_str(line);
    capture.push('\n');
    while capture.len() > NPM_STDERR_CAPTURE_LIMIT {
        match capture.find('\n') {
            Some(end) if end + 1 < capture.len() => {
                capture.drain(..=end);
            }
            _ => {
                let mut cut = capture.len() - NPM_STDERR_CAPTURE_LIMIT;
                while !capture.is_char_boundary(cut) {
                    cut += 1;
                }
                capture.drain(..cut);
            }
        }
    }
}

/// How many `npm error` detail lines after the `npm error code` line are
/// kept in the recorded error, and how long each may be.
const NPM_ERROR_SUMMARY_LINES: usize = 3;
const NPM_ERROR_SUMMARY_LINE_LIMIT: usize = 240;

/// npm's own account of why it failed, from its captured stderr: the last
/// `npm error code <CODE>` line and the first few `npm error` detail lines
/// after it (minus `errno`, which repeats the code, and the log-file pointer).
/// Appended to the recorded install error so `state.json` and Doctor can say
/// *why* — the exit status alone says nothing about a block page, a 404, or
/// an unreachable mirror. `None` when npm printed no error block.
fn npm_error_summary(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr.lines().map(str::trim_end).collect();
    let code_index = lines.iter().rposition(|line| {
        line.starts_with("npm error code ") || line.starts_with("npm ERR! code ")
    })?;
    let mut summary = vec![clip_line(lines[code_index])];
    let details = lines[code_index + 1..]
        .iter()
        .filter(|line| line.starts_with("npm error") || line.starts_with("npm ERR!"))
        .map(|line| {
            line.trim_start_matches("npm error")
                .trim_start_matches("npm ERR!")
                .trim()
        })
        .filter(|detail| {
            !detail.is_empty()
                && !detail.starts_with("errno ")
                && !detail.starts_with("A complete log")
        })
        .take(NPM_ERROR_SUMMARY_LINES)
        .map(clip_line);
    summary.extend(details);
    Some(summary.join("; "))
}

fn clip_line(line: &str) -> String {
    if line.len() <= NPM_ERROR_SUMMARY_LINE_LIMIT {
        return line.to_string();
    }
    let mut cut = NPM_ERROR_SUMMARY_LINE_LIMIT;
    while !line.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &line[..cut])
}

#[allow(clippy::too_many_arguments)]
async fn run_floating_npm_install(
    packages_root: &Path,
    node_install_dir: &Path,
    install_dir: &Path,
    tool: &ManagedTool,
    env_vars: &[(String, String)],
    registry: Option<&str>,
    on_line: &InstallLineFn<'_>,
) -> Result<(), NpmInstallFailure> {
    let mut stderr_capture = String::new();
    run_floating_npm_install_capturing(
        packages_root,
        node_install_dir,
        install_dir,
        tool,
        env_vars,
        registry,
        on_line,
        &mut stderr_capture,
    )
    .await
    .map_err(|error| NpmInstallFailure {
        error,
        stderr: stderr_capture,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_floating_npm_install_capturing(
    packages_root: &Path,
    node_install_dir: &Path,
    install_dir: &Path,
    tool: &ManagedTool,
    env_vars: &[(String, String)],
    registry: Option<&str>,
    on_line: &InstallLineFn<'_>,
    stderr_capture: &mut String,
) -> Result<(), ManagedToolError> {
    let node_bin_dir = node_install_dir.join("bin");
    let mut command = tokio::process::Command::new(node_bin_dir.join("npm"));
    command
        .arg("install")
        .arg("--prefix")
        .arg(install_dir)
        .args([
            "--omit=dev",
            "--include=optional",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ]);
    if let Some(registry) = registry {
        command.arg("--registry").arg(registry);
    }
    // `@latest` floats to the newest published version; npm on the managed
    // runtime resolves the platform-native optional dependency for the
    // running machine on its own, so no `--os`/`--cpu` pinning is needed.
    command.arg(format!("{}@latest", tool.package));

    // Exactly the snapshot, not the Tauri process env: launched from Finder
    // the process carries none of the user's shell rc, so an
    // `NPM_CONFIG_REGISTRY`/`npm_config_registry` exported there would steer
    // the freshness probe and doctor's fixes but not this install. The
    // snapshot is what all of those run under.
    command.env_clear();
    for (key, value) in env_vars {
        command.env(key, value);
    }
    // npm's own `#!/usr/bin/env node` shebang must resolve the managed node:
    // its bin dir goes first on the snapshot's PATH (the process PATH when
    // the snapshot carries none).
    let inherited_path = snapshot_var(env_vars, "PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let mut paths = vec![node_bin_dir.clone()];
    paths.extend(std::env::split_paths(&inherited_path));
    if let Ok(path_value) = std::env::join_paths(paths) {
        command.env("PATH", path_value);
    }
    // Share the private prefix's download cache; `--prefix` on the command
    // line outranks any inherited prefix config.
    let cache = packages_root.join("npm-prefix").join("cache");
    command.env("NPM_CONFIG_CACHE", &cache);
    command.env("npm_config_cache", &cache);
    command
        .current_dir(install_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = command
        .spawn()
        .map_err(|error| ManagedToolError::NpmInstall(format!("spawn managed npm: {error}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let forward_out = async {
        if let Some(stream) = stdout {
            let mut lines = tokio::io::BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                on_line(&line);
            }
        }
    };
    let forward_err = async {
        if let Some(stream) = stderr {
            let mut lines = tokio::io::BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                on_line(&line);
                push_captured_line(stderr_capture, &line);
            }
        }
    };
    let label = registry_label(registry);
    let wait = async {
        match tokio::time::timeout(NPM_INSTALL_TIMEOUT, child.wait()).await {
            Ok(result) => result
                .map_err(|error| ManagedToolError::NpmInstall(format!("wait on npm: {error}"))),
            Err(_) => {
                let _ = child.kill().await;
                Err(ManagedToolError::NpmInstall(format!(
                    "timed out after {} seconds (registry: {label})",
                    NPM_INSTALL_TIMEOUT.as_secs()
                )))
            }
        }
    };
    let (status, (), ()) = tokio::join!(wait, forward_out, forward_err);
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        let summary = npm_error_summary(stderr_capture)
            .map(|summary| format!(": {summary}"))
            .unwrap_or_default();
        Err(ManagedToolError::NpmInstall(format!(
            "npm install exited with {status} (registry: {label}){summary}"
        )))
    }
}

/// Shim body for a managed bridge. Both paths are absolute, so the shim runs
/// with no `node` on PATH at all.
fn shim_contents(node: &Path, entrypoint: &Path) -> String {
    format!(
        "#!/bin/sh\n# Written by Staged's managed ACP tools installer; do not edit.\nexec {} {} \"$@\"\n",
        sh_quote(node),
        sh_quote(entrypoint)
    )
}

fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

fn write_shim(bin_dir: &Path, binary: &str, contents: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(bin_dir)?;
    let path = bin_dir.join(binary);
    let temp = bin_dir.join(format!(".{binary}.tmp"));
    std::fs::write(&temp, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&temp, &path)
}

// =============================================================================
// Doctor readouts — installed and latest versions of a managed bridge
// =============================================================================

/// The ACP package version installed in the live managed prefix for `tool`,
/// read from the package's own `package.json` — the tree the shim actually
/// execs. Falls back to the version `state.json` recorded at install time when
/// that tree is unreadable; state is supporting information, not the
/// authority, since it can be missing (a state write that failed after the
/// swap) or stale (a tree swapped in by another Staged instance).
pub fn installed_tool_version(packages_root: &Path, tool: &ManagedTool) -> Option<String> {
    installed_version(&tool_install_dir(packages_root, tool.id), tool.package).or_else(|| {
        read_state(packages_root)
            .tools
            .get(tool.id)
            .map(|pin| pin.version.clone())
            .filter(|version| !version.is_empty())
    })
}

/// How long a registry answer is reused before the registry is asked again.
/// Mirrors the doctor crate's freshness cache: a Doctor re-run within the
/// hour costs no network, and a real release still shows up within one.
const LATEST_VERSION_CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// A registry lookup that hangs must not hold the freshness pass open — the
/// same bound the doctor crate puts on its own `npm view` probes.
const LATEST_VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Latest published versions by package, with the time each was fetched. Only
/// answers are cached: a failed lookup is retried on the next pass rather than
/// pinned as "unknown" for an hour.
#[derive(Debug, Default)]
struct LatestVersionCache {
    entries: HashMap<String, (String, Instant)>,
}

impl LatestVersionCache {
    fn get_fresh(&self, package: &str, now: Instant) -> Option<String> {
        let (version, fetched_at) = self.entries.get(package)?;
        (now.saturating_duration_since(*fetched_at) <= LATEST_VERSION_CACHE_TTL)
            .then(|| version.clone())
    }

    fn insert(&mut self, package: &str, version: String, now: Instant) {
        self.entries.insert(package.to_string(), (version, now));
    }
}

fn latest_version_cache() -> &'static Mutex<LatestVersionCache> {
    static CACHE: OnceLock<Mutex<LatestVersionCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(LatestVersionCache::default()))
}

/// The newest published version of `tool`'s npm package on the configured
/// registry, or `None` when it cannot be determined — offline, registry
/// unreachable, no `npm` to run, or the probe timed out. The caller must keep
/// `None` as *unknown*, never read it as "up to date".
///
/// Runs `npm view <pkg> version` under the same env snapshot doctor's own
/// probes and the managed install itself run under (see
/// [`install_managed_tool`]), so the managed runtime's `npm` is preferred and
/// the lookup resolves the registry the install will — `~/.npmrc` and any
/// `NPM_CONFIG_REGISTRY` the user's shell rc exports alike — with the same
/// override → npm config → gateway-blocked fallback precedence. Bounded by
/// [`LATEST_VERSION_PROBE_TIMEOUT`] and cached for [`LATEST_VERSION_CACHE_TTL`].
pub async fn latest_published_version(
    tool: &ManagedTool,
    env_vars: &[(String, String)],
) -> Option<String> {
    let now = Instant::now();
    let cached = latest_version_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_fresh(tool.package, now);
    if cached.is_some() {
        return cached;
    }
    let npm = find_on_env_path(env_vars, "npm")?;
    let version = probe_latest_version(
        &npm,
        tool.package,
        npm_registry_override().as_deref(),
        fallback_npm_registry(),
        env_vars,
        LATEST_VERSION_PROBE_TIMEOUT,
    )
    .await?;
    latest_version_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(tool.package, version.clone(), now);
    Some(version)
}

/// One uncached latest-version lookup with the registry precedence applied:
/// `override_registry` if given, else npm's own config, and — only when that
/// fails as a gateway block of `registry.npmjs.org` and no override was
/// given — one retry through `fallback_registry` within whatever is left of
/// `timeout`.
async fn probe_latest_version(
    npm_path: &Path,
    package: &str,
    override_registry: Option<&str>,
    fallback_registry: Option<&str>,
    env_vars: &[(String, String)],
    timeout: Duration,
) -> Option<String> {
    let started = Instant::now();
    let failure =
        match npm_view_version(npm_path, package, override_registry, env_vars, timeout).await {
            Ok(version) => return Some(version),
            Err(failure) => failure,
        };
    let NpmViewFailure::Exit { stderr } = failure else {
        return None;
    };
    let fallback = gateway_fallback_registry(override_registry, fallback_registry, &stderr)?;
    let remaining = timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())?;
    log::info!(
        "npm view {package} version: registry.npmjs.org is gateway-blocked; retrying through {fallback}"
    );
    npm_view_version(npm_path, package, Some(fallback), env_vars, remaining)
        .await
        .ok()
}

/// The first `<dir>/<name>` that is a file along the snapshot's `PATH` —
/// resolved by hand because `Command::new("npm")` would search the *parent*
/// process's PATH, not the snapshot's, and the snapshot is what puts the
/// managed runtime's bin dir first.
pub(crate) fn find_on_env_path(env_vars: &[(String, String)], name: &str) -> Option<PathBuf> {
    std::env::split_paths(snapshot_var(env_vars, "PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The last value of `key` in an env snapshot — the one a `Command` applying
/// the snapshot in order would end up with.
fn snapshot_var<'a>(env_vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env_vars
        .iter()
        .rev()
        .find(|(existing, _)| existing == key)
        .map(|(_, value)| value.as_str())
}

/// Why an `npm view` probe produced no version.
#[derive(Debug)]
enum NpmViewFailure {
    /// npm ran and exited non-zero; its stderr is kept for classification.
    Exit { stderr: String },
    /// npm could not be started, timed out, or printed something that is
    /// not a version.
    Other,
}

/// `npm view <package> version [--registry <url>]` with `npm` at `npm_path`,
/// under exactly the `env_vars` snapshot, killed at `timeout`. `Ok` only for
/// a clean exit whose stdout is a version-shaped token.
///
/// npm's own retry/timeout defaults (`fetch-retries=2`, backoff up to 60 s)
/// would let a dead registry host run ~70 s; a probe is not worth that, so
/// retries are off and npm's fetch timeout is pinned to the probe budget.
/// Installs keep npm's defaults.
async fn npm_view_version(
    npm_path: &Path,
    package: &str,
    registry: Option<&str>,
    env_vars: &[(String, String)],
    timeout: Duration,
) -> Result<String, NpmViewFailure> {
    let mut command = tokio::process::Command::new(npm_path);
    command.args(["view", package, "version", "--fetch-retries=0"]);
    command.arg(format!("--fetch-timeout={}", timeout.as_millis()));
    if let Some(registry) = registry {
        command.args(["--registry", registry]);
    }
    command.env_clear();
    for (key, value) in env_vars {
        command.env(key, value);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            log::debug!("npm view {package} version failed to run: {error}");
            return Err(NpmViewFailure::Other);
        }
        Err(_) => {
            log::debug!(
                "npm view {package} version timed out after {}s (registry: {})",
                timeout.as_secs(),
                registry_label(registry)
            );
            return Err(NpmViewFailure::Other);
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        log::debug!(
            "npm view {package} version exited with {} (registry: {}): {stderr}",
            output.status,
            registry_label(registry)
        );
        return Err(NpmViewFailure::Exit { stderr });
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    version
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit())
        .then_some(version)
        .ok_or(NpmViewFailure::Other)
}

/// How long Doctor waits on `npm config get registry`.
const NPM_CONFIG_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The registry npm would use under the `env_vars` snapshot with no
/// `--registry` — `npm config get registry` asked of the snapshot's own
/// `npm` (the managed runtime's, when installed), under exactly that env, so
/// the answer is the one a managed install resolves. `None` when the
/// snapshot has no `npm`, the probe fails or times out, or the answer is not
/// a URL; Doctor then says "npm default" rather than guessing.
pub(crate) async fn configured_npm_registry(env_vars: &[(String, String)]) -> Option<String> {
    let npm_path = find_on_env_path(env_vars, "npm")?;
    let mut command = tokio::process::Command::new(npm_path);
    command.args(["config", "get", "registry"]);
    command.env_clear();
    for (key, value) in env_vars {
        command.env(key, value);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(NPM_CONFIG_PROBE_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let registry = String::from_utf8_lossy(&output.stdout).trim().to_string();
    registry.starts_with("http").then_some(registry)
}

// =============================================================================
// Reconcile epilogue — prune stale ids, record the outcome, gate the Node prune
// =============================================================================

/// Reconcile epilogue: drop installs for ids no longer in the managed set
/// (their shims, tool dirs, and state entries), record the run's outcome in
/// `state.json`, and — only when every managed bridge installed cleanly —
/// prune superseded managed Node runtimes. Takes the install mutex and the
/// cross-process flock so it cannot race an in-flight install in this or any
/// other Staged process.
pub(crate) async fn finish_reconcile(managed: &[ManagedTool], errors: Vec<String>) {
    let Some(packages_root) = crate::paths::packages_dir() else {
        return;
    };
    finish_reconcile_at(&packages_root, managed, errors).await;
}

async fn finish_reconcile_at(packages_root: &Path, managed: &[ManagedTool], errors: Vec<String>) {
    let all_installed = errors.is_empty();
    {
        let _guard = tool_install_lock().lock().await;
        let _packages_lock = match managed_node::lock_packages_dir(packages_root).await {
            Ok(lock) => lock,
            Err(error) => {
                log::warn!("skipping ACP tools reconcile epilogue: {error}");
                return;
            }
        };
        prune_stale_managed_tools(packages_root, managed);
        record_reconcile(packages_root, errors);
    }
    // Success-gated Node prune: `errors` empty means every managed bridge
    // reinstalled this run, so every shim now embeds the pinned runtime's
    // path and superseded runtimes are unreferenced. On partial failure the
    // old runtime is kept — the failed bridge's un-rewritten shim still
    // resolves a real Node, so an offline launch never breaks a working
    // bridge. Runs outside the scope above: the prune takes the same flock
    // itself, and nesting would deadlock (see `lock_packages_dir`).
    if all_installed {
        managed_node::prune_superseded_node_runtimes(packages_root).await;
    }
}

pub(crate) fn prune_stale_managed_tools(packages_root: &Path, managed: &[ManagedTool]) {
    let managed_ids: Vec<&str> = managed.iter().map(|tool| tool.id).collect();
    let managed_binaries: Vec<&str> = managed.iter().map(|tool| tool.binary).collect();

    let mut state = read_state(packages_root);
    let stale: Vec<String> = state
        .tools
        .keys()
        .filter(|id| !managed_ids.contains(&id.as_str()))
        .cloned()
        .collect();
    for id in &stale {
        if let Some(pin) = state.tools.remove(id) {
            let _ = std::fs::remove_file(shim_bin_dir(packages_root).join(&pin.binary));
        }
    }
    if !stale.is_empty() {
        if let Err(error) = write_state(packages_root, &state) {
            log::warn!("failed to write ACP tools state after prune: {error}");
        }
    }

    // Tool dirs with no state entry (crashed installs) and shims for binaries
    // no longer managed. `<packages>/bin` holds only Staged-written shims, so
    // pruning by name is safe.
    if let Ok(entries) = std::fs::read_dir(tools_root(packages_root)) {
        for entry in entries.flatten() {
            if !managed_ids.contains(&entry.file_name().to_string_lossy().as_ref()) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(shim_bin_dir(packages_root)) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') && !managed_binaries.contains(&name.as_str()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

pub(crate) fn record_reconcile(packages_root: &Path, errors: Vec<String>) {
    let mut state = read_state(packages_root);
    state.last_reconcile = Some(ReconcileRecord {
        at_ms: now_ms(),
        ok: errors.is_empty(),
        errors,
    });
    if let Err(error) = write_state(packages_root, &state) {
        log::warn!("failed to record ACP tools reconcile result: {error}");
    }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    mod validation;
    use super::*;

    // -- registry policy -----------------------------------------------------

    /// `STAGED_NPM_REGISTRY` is honored only when it carries a value; unset,
    /// empty, or whitespace all mean "npm decides".
    #[test]
    fn npm_registry_override_is_the_trimmed_non_empty_env_value() {
        assert_eq!(registry_from_env_value(None), None);
        assert_eq!(registry_from_env_value(Some("")), None);
        assert_eq!(registry_from_env_value(Some("   \n")), None);
        assert_eq!(
            registry_from_env_value(Some("  https://registry.example.test/npm/ ")),
            Some("https://registry.example.test/npm/".to_string())
        );
        assert_eq!(
            npm_registry_override(),
            registry_from_env_value(std::env::var(NPM_REGISTRY_ENV).ok().as_deref())
        );
    }

    /// The fallback is whatever the build injected — no URL lives in source,
    /// so a from-source build has none and a value is never blank.
    #[test]
    fn fallback_npm_registry_is_build_time_only() {
        let expected = option_env!("STAGED_FALLBACK_NPM_REGISTRY")
            .map(str::trim)
            .filter(|value| !value.is_empty());
        assert_eq!(fallback_npm_registry(), expected);
        if let Some(fallback) = fallback_npm_registry() {
            assert!(!fallback.is_empty());
            assert_eq!(fallback, fallback.trim());
        }
    }

    #[test]
    fn registry_label_names_npm_config_when_no_registry_is_passed() {
        assert_eq!(
            registry_label(Some("https://registry.example.test/npm/")),
            "https://registry.example.test/npm/"
        );
        assert_eq!(registry_label(None), "npm default (per npm config)");
    }

    /// Gateway-blocked only for the block page itself, or a fetch/JSON failure
    /// on a request npm made *to* `registry.npmjs.org`. The public registry's
    /// hostname appearing elsewhere in stderr proves nothing — mirrors name
    /// their upstream in their own error bodies.
    #[test]
    fn classify_npm_failure_spots_gateway_block_pages_only() {
        let blocked = [
            // The real thing: Cloudflare Gateway redirects the packument fetch
            // to its block page, and npm reports the redirect target.
            "npm error code FETCH_ERROR\nnpm error errno FETCH_ERROR\nnpm error request to https://registry.npmjs.org/@agentclientprotocol%2fclaude-agent-acp failed: https://blocked.teams.cloudflare.com?block_reason=...",
            // The block page served inline as the response body.
            "npm warn deprecated foo@1.0.0: use bar\nnpm error code FETCH_ERROR\nnpm error errno FETCH_ERROR\nnpm error invalid json response body at https://registry.npmjs.org/@agentclientprotocol%2fclaude-agent-acp reason: Unexpected token '<', \"<!DOCTYPE \"... is not valid JSON",
            // The `registry:` detail line npm prints for some fetch errors.
            "npm error code EJSONPARSE\nnpm error Unexpected token < in JSON at position 0\nnpm error registry: https://registry.npmjs.org/",
            // A truncated body from the public registry is also FETCH_ERROR
            // (minipass-fetch's catch-all); it is accepted as blocked because
            // the only cost is one extra attempt through the fallback.
            "npm error code FETCH_ERROR\nnpm error errno FETCH_ERROR\nnpm error request to https://registry.npmjs.org/foo/-/foo-1.0.0.tgz failed, reason: aborted",
        ];
        for stderr in blocked {
            assert_eq!(
                classify_npm_failure(stderr),
                NpmFailure::GatewayBlocked,
                "{stderr}"
            );
        }
        let other = [
            "",
            // A 404 from the public registry: the package is not there.
            "npm error code E404\nnpm error 404 Not Found - GET https://registry.npmjs.org/@agentclientprotocol%2fclaude-agent-acp - Not found",
            "npm error code ETARGET\nnpm error notarget No matching version found for foo@latest",
            // A dead custom mirror from `~/.npmrc`: ENOTFOUND, not a block.
            "npm error code ENOTFOUND\nnpm error syscall getaddrinfo\nnpm error errno ENOTFOUND\nnpm error network request to https://mirror.example.test/npm/foo failed, reason: getaddrinfo ENOTFOUND mirror.example.test",
            "npm error code FETCH_ERROR\nnpm error request to https://mirror.example.test/npm/foo failed, reason: getaddrinfo ENOTFOUND mirror.example.test",
            // An Artifactory error body naming *its* upstream: the request npm
            // made went to the mirror, so this is the mirror failing.
            "npm error code FETCH_ERROR\nnpm error errno FETCH_ERROR\nnpm error invalid json response body at https://artifactory.example.test/api/npm/npm/foo reason: Unexpected token '<', \"<html><body>Failed to fetch from registry.npmjs.org (https://registry.npmjs.org/foo)\"... is not valid JSON",
            // The public registry itself unreachable (offline): a network
            // error, not a block page.
            "npm error code ENOTFOUND\nnpm error network request to https://registry.npmjs.org/foo failed, reason: getaddrinfo ENOTFOUND registry.npmjs.org",
        ];
        for stderr in other {
            assert_eq!(classify_npm_failure(stderr), NpmFailure::Other, "{stderr}");
        }
    }

    #[test]
    fn gateway_retry_failed_message_keeps_the_original_diagnosis() {
        let message = gateway_retry_failed_message(
            "https://fallback.example.test/npm/",
            "npm install exited with exit status: 1 (registry: https://fallback.example.test/npm/)",
        );
        assert_eq!(
            message,
            "npm default registry (registry.npmjs.org) was blocked by a network gateway; \
             fallback https://fallback.example.test/npm/ also failed: npm install exited with \
             exit status: 1 (registry: https://fallback.example.test/npm/)"
        );
    }

    /// The Doctor hint classifies the *recorded* error: the block page (with
    /// or without a fallback having been tried), a 404 from whichever
    /// registry, and an unreachable registry or download host. A validation
    /// failure, a missing entrypoint, and a probe timeout get no hint — there
    /// is nothing network-side to change.
    #[test]
    fn classify_install_failure_names_the_three_fixable_causes() {
        let cases = [
            (
                "npm install failed: npm install exited with exit status: 1 (registry: npm default (per npm config)): npm error code FETCH_ERROR; invalid json response body at https://registry.npmjs.org/@agentclientprotocol%2fclaude-agent-acp reason: Unexpected token '<'",
                Some(InstallFailureHint::GatewayBlocked),
            ),
            (
                "npm install failed: npm default registry (registry.npmjs.org) was blocked by a network gateway; fallback https://fallback.example.test/npm/ also failed: npm install exited with exit status: 1 (registry: https://fallback.example.test/npm/)",
                Some(InstallFailureHint::GatewayBlocked),
            ),
            (
                "npm install failed: npm install exited with exit status: 1 (registry: https://mirror.example.test/npm/): npm error code E404; 404 Not Found - GET https://mirror.example.test/npm/@agentclientprotocol%2fclaude-agent-acp",
                Some(InstallFailureHint::PackageMissing),
            ),
            (
                "npm install failed: npm install exited with exit status: 1 (registry: npm default (per npm config)): npm error code ENOTFOUND; network request to https://mirror.example.test/npm/x failed, reason: getaddrinfo ENOTFOUND mirror.example.test",
                Some(InstallFailureHint::HostUnreachable),
            ),
            (
                "Node.js runtime download failed: https://nodejs.org/dist/v24.21.0/node-v24.21.0-darwin-arm64.tar.gz: error sending request for url",
                Some(InstallFailureHint::HostUnreachable),
            ),
            (
                "npm install failed: timed out after 900 seconds (registry: npm default (per npm config))",
                Some(InstallFailureHint::HostUnreachable),
            ),
            (
                "installed ACP bridge is incomplete: @agentclientprotocol/claude-agent-acp: validation probe timed out after 20 seconds",
                None,
            ),
            (
                "installed ACP bridge is incomplete: bridge entrypoint /x/index.js is missing after install",
                None,
            ),
            ("Node.js runtime download failed: HTTP 503", None),
            ("", None),
        ];
        for (error, expected) in cases {
            assert_eq!(classify_install_failure(error), expected, "{error}");
        }

        let hint = install_failure_hint(
            "npm install failed: … npm error code E404; 404 Not Found",
            "@agentclientprotocol/codex-acp",
        )
        .unwrap();
        assert_eq!(
            hint,
            "the configured registry does not carry @agentclientprotocol/codex-acp"
        );
        assert!(install_failure_hint(
            "npm error code FETCH_ERROR request to https://registry.npmjs.org/x failed",
            "x"
        )
        .unwrap()
        .contains("STAGED_NPM_REGISTRY"));
        assert!(install_failure_hint("getaddrinfo ENOTFOUND mirror", "x")
            .unwrap()
            .contains("STAGED_NODE_DIST_URL"));
        assert!(install_failure_hint("disk full", "x").is_none());
    }

    /// The recorded install error carries npm's own account: the last
    /// `npm error code` line and the detail lines after it, minus the `errno`
    /// repeat and the log-file pointer, each clipped. Without an error block
    /// nothing is appended.
    #[test]
    fn npm_error_summary_keeps_the_code_and_the_first_details() {
        let stderr = "npm warn deprecated foo@1.0.0: use bar\n\
            npm error code FETCH_ERROR\n\
            npm error errno FETCH_ERROR\n\
            npm error invalid json response body at https://registry.npmjs.org/x reason: Unexpected token '<'\n\
            npm error\n\
            npm error A complete log of this run can be found in: /tmp/x.log\n";
        assert_eq!(
            npm_error_summary(stderr).as_deref(),
            Some(
                "npm error code FETCH_ERROR; invalid json response body at \
                 https://registry.npmjs.org/x reason: Unexpected token '<'"
            )
        );
        assert_eq!(
            classify_npm_failure(&npm_error_summary(stderr).unwrap()),
            NpmFailure::GatewayBlocked
        );

        // Older npm spelling, and more details than are kept.
        let stderr = "npm ERR! code E404\nnpm ERR! 404 Not Found - GET https://r/x\nnpm ERR! 404\nnpm ERR! 404 'x@latest' is not in this registry.\nnpm ERR! 404 note\nnpm ERR! 404 more\n";
        assert_eq!(
            npm_error_summary(stderr).as_deref(),
            Some("npm ERR! code E404; 404 Not Found - GET https://r/x; 404; 404 'x@latest' is not in this registry.")
        );

        assert_eq!(npm_error_summary("npm warn only\n"), None);
        assert_eq!(npm_error_summary(""), None);

        let long = format!("npm error code E1\nnpm error {}\n", "y".repeat(1000));
        let summary = npm_error_summary(&long).unwrap();
        assert!(summary.len() < 300, "{}", summary.len());
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn managed_tools_disabled_reason_names_the_cause() {
        assert_eq!(
            managed_tools_disabled_reason_from_parts(None, false, true),
            None
        );
        assert_eq!(
            managed_tools_disabled_reason_from_parts(Some(Path::new("/dev/acp")), false, true)
                .as_deref(),
            Some("STAGED_ACP_TOOLS_DIR is set to /dev/acp")
        );
        assert_eq!(
            managed_tools_disabled_reason_from_parts(None, true, true).as_deref(),
            Some("this build was compiled without managed ACP tools")
        );
        assert_eq!(
            managed_tools_disabled_reason_from_parts(None, false, false).as_deref(),
            Some("no managed Node.js runtime is available for this platform")
        );
        // The override is the reason even on a build that is disabled anyway.
        assert!(
            managed_tools_disabled_reason_from_parts(Some(Path::new("/d")), true, false)
                .unwrap()
                .starts_with("STAGED_ACP_TOOLS_DIR")
        );
    }

    /// The registry probe asks the snapshot's own `npm`, under the snapshot's
    /// env, and reports only a URL-shaped answer.
    #[tokio::test]
    async fn configured_npm_registry_asks_the_snapshots_npm() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        write_executable(
            &bin.join("npm"),
            "#!/bin/sh\n[ \"$1 $2 $3\" = \"config get registry\" ] || exit 2\nprintf '%s\\n' \"${FAKE_REGISTRY:-https://mirror.example.test/npm/}\"\n",
        );
        let env = vec![("PATH".to_string(), bin.display().to_string())];
        assert_eq!(
            configured_npm_registry(&env).await.as_deref(),
            Some("https://mirror.example.test/npm/")
        );

        // The env is the snapshot's, not the process's.
        let mut env_with_registry = env.clone();
        env_with_registry.push((
            "FAKE_REGISTRY".to_string(),
            "https://other.example.test/".to_string(),
        ));
        assert_eq!(
            configured_npm_registry(&env_with_registry).await.as_deref(),
            Some("https://other.example.test/")
        );

        // Not a URL: no answer. No npm on the snapshot PATH: no answer.
        let mut env_garbage = env.clone();
        env_garbage.push(("FAKE_REGISTRY".to_string(), "undefined".to_string()));
        assert_eq!(configured_npm_registry(&env_garbage).await, None);
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(
            configured_npm_registry(&[("PATH".to_string(), empty.display().to_string())]).await,
            None
        );
    }

    /// The capture keeps the *newest* lines: npm's `npm error code …` block
    /// comes last, after however many warnings preceded it.
    #[test]
    fn stderr_capture_keeps_the_tail() {
        let mut capture = String::new();
        let warning = format!("npm warn deprecated {}", "x".repeat(1000));
        for _ in 0..(NPM_STDERR_CAPTURE_LIMIT / warning.len() + 5) {
            push_captured_line(&mut capture, &warning);
        }
        push_captured_line(&mut capture, "npm error code FETCH_ERROR");
        push_captured_line(
            &mut capture,
            "npm error request to https://registry.npmjs.org/x failed: https://blocked.teams.cloudflare.com",
        );
        assert!(capture.len() <= NPM_STDERR_CAPTURE_LIMIT);
        assert!(capture.starts_with("npm warn"), "drops whole lines only");
        assert!(capture.ends_with("blocked.teams.cloudflare.com\n"));
        assert_eq!(classify_npm_failure(&capture), NpmFailure::GatewayBlocked);

        // A single line larger than the whole budget keeps its tail, cut on a
        // char boundary.
        let mut capture = String::new();
        push_captured_line(
            &mut capture,
            &format!("{}é end", "a".repeat(NPM_STDERR_CAPTURE_LIMIT)),
        );
        assert_eq!(capture.len(), NPM_STDERR_CAPTURE_LIMIT);
        assert!(capture.ends_with("é end\n"));
    }

    /// The fallback is consulted only with no override, a compiled-in
    /// fallback, and a gateway-shaped failure — all three at once.
    #[test]
    fn gateway_fallback_registry_requires_no_override_a_fallback_and_a_block_page() {
        let blocked = "npm error code FETCH_ERROR request to https://registry.npmjs.org/x failed";
        let fallback = Some("https://fallback.example.test/npm/");
        assert_eq!(
            gateway_fallback_registry(None, fallback, blocked),
            fallback,
            "no override + fallback + block page retries"
        );
        assert_eq!(
            gateway_fallback_registry(Some("https://mine.example.test/"), fallback, blocked),
            None,
            "an explicit override is never second-guessed"
        );
        assert_eq!(
            gateway_fallback_registry(None, None, blocked),
            None,
            "nothing to retry through"
        );
        assert_eq!(
            gateway_fallback_registry(None, fallback, "npm error code E404"),
            None,
            "a 404 is not a gateway block"
        );
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
    fn managed_tools_require_no_override_no_disable_and_a_supported_target() {
        assert!(managed_tools_enabled_from_parts(false, false, true));
        assert!(!managed_tools_enabled_from_parts(true, false, true));
        assert!(!managed_tools_enabled_from_parts(false, true, true));
        assert!(!managed_tools_enabled_from_parts(false, false, false));
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

    #[test]
    fn managed_tools_table_lists_the_two_bridges() {
        let ids: Vec<&str> = MANAGED_TOOLS.iter().map(|tool| tool.id).collect();
        assert_eq!(ids, vec!["claude-acp", "codex-acp"]);
        for tool in MANAGED_TOOLS {
            assert!(
                tool.package.starts_with("@agentclientprotocol/"),
                "{}",
                tool.package
            );
            assert!(!tool.binary.is_empty(), "{}", tool.id);
        }
    }

    // -- fixtures -----------------------------------------------------------

    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    fn test_tool() -> ManagedTool {
        ManagedTool {
            id: "claude-acp",
            binary: "claude-agent-acp",
            package: "@agentclientprotocol/claude-agent-acp",
        }
    }

    const TEST_NODE_VERSION: &str = "v9.9.9";

    /// The env snapshot the fixture installs run under: enough of the test
    /// process env for a `#!/bin/sh` fake npm to find `cp` and its home.
    fn install_env() -> Vec<(String, String)> {
        ["PATH", "HOME"]
            .into_iter()
            .filter_map(|key| Some((key.to_string(), std::env::var(key).ok()?)))
            .collect()
    }

    fn write_json(path: &Path, value: &serde_json::Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    /// A fixture install tree: the package's `package.json` (with the resolved
    /// version) and its `dist/index.js` entrypoint.
    fn write_fixture_install(install_dir: &Path, tool: &ManagedTool, version: &str) {
        write_json(
            &package_dir(install_dir, tool.package).join("package.json"),
            &serde_json::json!({ "name": tool.package, "version": version }),
        );
        let entrypoint = npm_entrypoint(install_dir, tool.package);
        std::fs::create_dir_all(entrypoint.parent().unwrap()).unwrap();
        std::fs::write(&entrypoint, "// bridge\n").unwrap();
    }

    // -- shims --------------------------------------------------------------

    #[test]
    fn shim_contents_execs_absolute_paths_and_quotes_spaces() {
        let contents = shim_contents(
            Path::new("/data/dir with spaces/packages/node/v1/plat/bin/node"),
            Path::new("/data/dir with spaces/packages/tools/claude-acp/node_modules/@scope/claude-acp/dist/index.js"),
        );
        assert!(contents.starts_with("#!/bin/sh\n"));
        assert!(contents.ends_with(
            "exec '/data/dir with spaces/packages/node/v1/plat/bin/node' '/data/dir with spaces/packages/tools/claude-acp/node_modules/@scope/claude-acp/dist/index.js' \"$@\"\n"
        ));
    }

    #[test]
    fn write_shim_is_executable() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        write_shim(&bin_dir, "claude-agent-acp", "#!/bin/sh\nexec true\n").unwrap();
        let shim = bin_dir.join("claude-agent-acp");
        assert!(is_executable(&shim));
        assert_eq!(
            std::fs::read_to_string(&shim).unwrap(),
            "#!/bin/sh\nexec true\n"
        );
    }

    // -- state --------------------------------------------------------------

    #[test]
    fn state_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = ManagedToolsState::default();
        state.tools.insert(
            "claude-acp".to_string(),
            InstalledToolPin {
                binary: "claude-agent-acp".to_string(),
                version: "1.2.3".to_string(),
                node_version: TEST_NODE_VERSION.to_string(),
            },
        );
        state.last_reconcile = Some(ReconcileRecord {
            at_ms: 42,
            ok: false,
            errors: vec!["codex-acp: boom".to_string()],
        });
        write_state(dir.path(), &state).unwrap();
        assert_eq!(read_state(dir.path()), state);

        // Missing and corrupt files read as the empty state.
        assert_eq!(
            read_state(&dir.path().join("absent")),
            ManagedToolsState::default()
        );
        std::fs::write(state_path(dir.path()), "not json").unwrap();
        assert_eq!(read_state(dir.path()), ManagedToolsState::default());
    }

    // -- install flow (fake npm) --------------------------------------------

    /// A fake managed-node install dir whose `npm` copies a pre-built fixture
    /// tree into the `--prefix` dir, standing in for a real floating install.
    fn write_fake_node_with_npm(node_install_dir: &Path, template: &Path, exit_code: i32) {
        let bin = node_install_dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("node"), "#!/bin/sh\necho v9.9.9\n").unwrap();
        let npm = format!(
            "#!/bin/sh\nprefix=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--prefix\" ]; then prefix=\"$arg\"; fi\n  prev=\"$arg\"\ndone\ncp -R '{}/.' \"$prefix/\"\necho \"added 3 packages\"\nexit {exit_code}\n",
            template.display()
        );
        std::fs::write(bin.join("npm"), npm).unwrap();
        use std::os::unix::fs::PermissionsExt;
        for name in ["node", "npm"] {
            std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
    }

    /// A fake managed-node `npm` that wipes its `--prefix` node_modules before
    /// exiting — stands in for a floating upgrade npm aborts mid-reify. Proves
    /// the swap fix: a live install this touched would be broken, so the test
    /// installs into a staging prefix and the live tree must survive.
    fn write_fake_node_with_destructive_npm(node_install_dir: &Path, exit_code: i32) {
        let bin = node_install_dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("node"), "#!/bin/sh\necho v9.9.9\n").unwrap();
        let npm = format!(
            "#!/bin/sh\nprefix=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--prefix\" ]; then prefix=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nrm -rf \"$prefix/node_modules\"\nexit {exit_code}\n"
        );
        std::fs::write(bin.join("npm"), npm).unwrap();
        use std::os::unix::fs::PermissionsExt;
        for name in ["node", "npm"] {
            std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
    }

    #[tokio::test]
    async fn install_npm_tool_installs_shims_and_records_version() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path().join("packages");
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let tool = test_tool();

        let template = dir.path().join("template");
        std::fs::create_dir_all(&template).unwrap();
        write_fixture_install(&template, &tool, "1.2.3");
        write_fake_node_with_npm(&node_install_dir, &template, 0);

        let lines = std::sync::Mutex::new(Vec::new());
        let on_line = |line: &str| lines.lock().unwrap().push(line.to_string());
        install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            None,
            None,
            super::validation::VALIDATION_TIMEOUT,
            &on_line,
        )
        .await
        .unwrap();

        let shim = shim_bin_dir(&packages_root).join(tool.binary);
        let entrypoint = npm_entrypoint(&tool_install_dir(&packages_root, tool.id), tool.package);
        assert!(is_executable(&shim));
        assert_eq!(
            std::fs::read_to_string(&shim).unwrap(),
            shim_contents(&node_binary(&node_install_dir), &entrypoint)
        );
        assert_eq!(
            read_state(&packages_root).tools.get(tool.id),
            Some(&InstalledToolPin {
                binary: tool.binary.to_string(),
                version: "1.2.3".to_string(),
                node_version: TEST_NODE_VERSION.to_string(),
            })
        );
        assert!(entrypoint.is_file());

        let recorded = lines.lock().unwrap().clone();
        assert!(recorded
            .iter()
            .any(|line| line.contains("added 3 packages")));
        assert!(recorded.iter().any(|line| line.contains("1.2.3 is ready")));
    }

    #[tokio::test]
    async fn failed_npm_install_writes_no_shim_and_no_state() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path().join("packages");
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let tool = test_tool();

        let template = dir.path().join("template");
        std::fs::create_dir_all(&template).unwrap();
        write_fixture_install(&template, &tool, "1.2.3");
        write_fake_node_with_npm(&node_install_dir, &template, 7);

        let error = install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            None,
            None,
            super::validation::VALIDATION_TIMEOUT,
            &|_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
        assert!(!shim_bin_dir(&packages_root).join(tool.binary).exists());
        assert!(read_state(&packages_root).tools.is_empty());
    }

    #[tokio::test]
    async fn install_without_entrypoint_fails_incomplete_before_shims() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path().join("packages");
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let tool = test_tool();

        // A clean npm exit that produced no bridge entrypoint (empty template).
        let template = dir.path().join("template");
        std::fs::create_dir_all(&template).unwrap();
        write_fake_node_with_npm(&node_install_dir, &template, 0);

        let error = install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            None,
            None,
            super::validation::VALIDATION_TIMEOUT,
            &|_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ManagedToolError::Incomplete(_)), "{error}");
        assert!(!shim_bin_dir(&packages_root).join(tool.binary).exists());
        assert!(read_state(&packages_root).tools.is_empty());
    }

    #[tokio::test]
    async fn failed_upgrade_preserves_the_previous_install() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path().join("packages");
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let tool = test_tool();

        // A healthy previously-installed bridge: tree + shim + state at 1.2.3.
        write_installed_tool(&packages_root, &node_install_dir, &tool);
        let install_dir = tool_install_dir(&packages_root, tool.id);
        let entrypoint = npm_entrypoint(&install_dir, tool.package);
        let shim = shim_bin_dir(&packages_root).join(tool.binary);
        let shim_before = std::fs::read_to_string(&shim).unwrap();

        // An upgrade whose npm destroys its --prefix tree and then fails. It
        // only ever touches the staging prefix, so the live install survives.
        write_fake_node_with_destructive_npm(&node_install_dir, 7);
        let error = install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            None,
            None,
            super::validation::VALIDATION_TIMEOUT,
            &|_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
        assert!(
            entrypoint.is_file(),
            "live entrypoint clobbered by failed upgrade"
        );
        assert_eq!(std::fs::read_to_string(&shim).unwrap(), shim_before);
        assert_eq!(
            read_state(&packages_root)
                .tools
                .get(tool.id)
                .map(|pin| pin.version.clone()),
            Some("1.2.3".to_string())
        );
        // No scratch dirs are left behind for the next reconcile to trip over.
        assert!(!staging_install_dir(&packages_root, tool.id).exists());
        assert!(!install_dir.with_extension("old").exists());
        // And the version Doctor reads is still the working install's.
        assert_eq!(
            installed_tool_version(&packages_root, &tool).as_deref(),
            Some("1.2.3")
        );
    }

    /// A managed update writes only under the packages root: nothing lands in
    /// a global prefix, and the fixture's neighbours are left alone.
    #[tokio::test]
    async fn install_touches_only_the_packages_root() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path().join("packages");
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let tool = test_tool();

        let template = dir.path().join("template");
        std::fs::create_dir_all(&template).unwrap();
        write_fixture_install(&template, &tool, "1.2.4");
        write_fake_node_with_npm(&node_install_dir, &template, 0);
        let outside = dir.path().join("global-bin");
        std::fs::create_dir_all(&outside).unwrap();

        install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            None,
            None,
            super::validation::VALIDATION_TIMEOUT,
            &|_| {},
        )
        .await
        .unwrap();

        let mut entries: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        assert_eq!(entries, vec!["global-bin", "packages", "template"]);
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        assert_eq!(
            installed_tool_version(&packages_root, &tool).as_deref(),
            Some("1.2.4")
        );
    }

    // -- install flow: registry fallback -------------------------------------

    /// A fake managed-node `npm` standing in for a laptop whose network
    /// gateway blocks `registry.npmjs.org`: without `--registry` it prints
    /// npm's block-page failure and exits 1; with one it performs the install
    /// from `template`. Every invocation's argv is appended to `argv_log`.
    fn write_fake_node_with_gateway_blocked_npm(
        node_install_dir: &Path,
        template: &Path,
        argv_log: &Path,
        stderr_without_registry: &str,
    ) {
        let bin = node_install_dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("node"), "#!/bin/sh\necho v9.9.9\n").unwrap();
        let npm = format!(
            "#!/bin/sh\necho \"$*\" >> '{log}'\nprefix=\"\"\nregistry=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [ \"$prev\" = \"--prefix\" ]; then prefix=\"$arg\"; fi\n  if [ \"$prev\" = \"--registry\" ]; then registry=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nif [ -z \"$registry\" ]; then\n  printf '%s\\n' '{stderr}' >&2\n  exit 1\nfi\ncp -R '{template}/.' \"$prefix/\"\necho \"added 3 packages\"\nexit 0\n",
            log = argv_log.display(),
            stderr = stderr_without_registry.replace('\'', "'\\''"),
            template = template.display(),
        );
        std::fs::write(bin.join("npm"), npm).unwrap();
        use std::os::unix::fs::PermissionsExt;
        for name in ["node", "npm"] {
            std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
    }

    const GATEWAY_BLOCK_STDERR: &str = "npm error code FETCH_ERROR\nnpm error errno FETCH_ERROR\nnpm error invalid json response body at https://registry.npmjs.org/@agentclientprotocol%2fclaude-agent-acp reason: Unexpected token < in JSON at position 0";

    fn logged_argv(argv_log: &Path) -> Vec<String> {
        std::fs::read_to_string(argv_log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// No override, a compiled-in fallback, and npm failing on a gateway
    /// block page: the install is retried once through the fallback from a
    /// fresh staging prefix, and succeeds.
    #[tokio::test]
    async fn gateway_blocked_install_retries_once_through_the_fallback_registry() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path().join("packages");
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let tool = test_tool();
        let template = dir.path().join("template");
        std::fs::create_dir_all(&template).unwrap();
        write_fixture_install(&template, &tool, "1.2.5");
        let argv_log = dir.path().join("npm-argv.log");
        write_fake_node_with_gateway_blocked_npm(
            &node_install_dir,
            &template,
            &argv_log,
            GATEWAY_BLOCK_STDERR,
        );

        let lines = std::sync::Mutex::new(Vec::new());
        let on_line = |line: &str| lines.lock().unwrap().push(line.to_string());
        install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            None,
            Some("https://fallback.example.test/npm/"),
            super::validation::VALIDATION_TIMEOUT,
            &on_line,
        )
        .await
        .unwrap();

        let argv = logged_argv(&argv_log);
        assert_eq!(argv.len(), 2, "{argv:?}");
        assert!(!argv[0].contains("--registry"), "{}", argv[0]);
        assert!(
            argv[1].contains("--registry https://fallback.example.test/npm/"),
            "{}",
            argv[1]
        );
        assert_eq!(
            installed_tool_version(&packages_root, &tool).as_deref(),
            Some("1.2.5")
        );
        let recorded = lines.lock().unwrap().clone();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains("retrying through https://fallback.example.test/npm/")),
            "{recorded:?}"
        );
        assert!(!staging_install_dir(&packages_root, tool.id).exists());
    }

    /// When the fallback attempt fails too, the surfaced error still says why
    /// the fallback was contacted: the default registry was gateway-blocked.
    #[tokio::test]
    async fn gateway_blocked_install_names_both_failures_when_the_fallback_fails() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path().join("packages");
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let tool = test_tool();
        // Fails whatever registry it is given: a block page without one, a
        // plain exit 7 with one.
        let bin = node_install_dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        write_executable(&bin.join("node"), "#!/bin/sh\necho v9.9.9\n");
        write_executable(
            &bin.join("npm"),
            &format!(
                "#!/bin/sh\ncase \" $* \" in\n  *\" --registry \"*) echo 'npm error code E500' >&2; exit 7 ;;\nesac\nprintf '%s\\n' '{}' >&2\nexit 1\n",
                GATEWAY_BLOCK_STDERR.replace('\'', "'\\''")
            ),
        );

        let error = install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            None,
            Some("https://fallback.example.test/npm/"),
            super::validation::VALIDATION_TIMEOUT,
            &|_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
        let message = error.to_string();
        // The retry's own `npm error code` line rides along, so the recorded
        // error says what the fallback answered, not just that it failed.
        assert_eq!(
            message,
            "npm install failed: npm default registry (registry.npmjs.org) was blocked by a \
             network gateway; fallback https://fallback.example.test/npm/ also failed: \
             npm install exited with exit status: 7 (registry: https://fallback.example.test/npm/): \
             npm error code E500"
        );
        assert!(
            message.contains(
                "fallback https://fallback.example.test/npm/ also failed: npm install exited with"
            ),
            "{message}"
        );
        assert!(
            message.contains("(registry: https://fallback.example.test/npm/)"),
            "{message}"
        );
        assert!(!staging_install_dir(&packages_root, tool.id).exists());
    }

    /// npm runs under the env snapshot, not the Tauri process env: a registry
    /// the user's shell rc exports reaches the install exactly as it reaches
    /// the freshness probe, while the managed node bin leads PATH and the
    /// private cache outranks whatever cache the snapshot named.
    #[tokio::test]
    async fn install_runs_npm_under_the_env_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path().join("packages");
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let tool = test_tool();
        let template = dir.path().join("template");
        std::fs::create_dir_all(&template).unwrap();
        write_fixture_install(&template, &tool, "1.2.3");
        write_fake_node_with_npm(&node_install_dir, &template, 0);
        // The fake npm also dumps the env it saw.
        let env_log = dir.path().join("npm-env.log");
        let npm = node_install_dir.join("bin").join("npm");
        let script = std::fs::read_to_string(&npm).unwrap();
        write_executable(
            &npm,
            &script.replacen(
                "#!/bin/sh\n",
                &format!("#!/bin/sh\nenv > '{}'\n", env_log.display()),
                1,
            ),
        );

        let mut env_vars = install_env();
        env_vars.push((
            "NPM_CONFIG_REGISTRY".to_string(),
            "https://mine.example.test/npm/".to_string(),
        ));
        env_vars.push((
            "npm_config_registry".to_string(),
            "https://mine.example.test/npm/".to_string(),
        ));
        env_vars.push((
            "NPM_CONFIG_CACHE".to_string(),
            "/somewhere/else/cache".to_string(),
        ));
        env_vars.push(("STAGED_TEST_SNAPSHOT_ONLY".to_string(), "1".to_string()));
        install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &env_vars,
            None,
            None,
            super::validation::VALIDATION_TIMEOUT,
            &|_| {},
        )
        .await
        .unwrap();

        let seen: std::collections::HashMap<String, String> = std::fs::read_to_string(&env_log)
            .unwrap()
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            seen.get("NPM_CONFIG_REGISTRY").map(String::as_str),
            Some("https://mine.example.test/npm/")
        );
        assert_eq!(
            seen.get("npm_config_registry").map(String::as_str),
            Some("https://mine.example.test/npm/")
        );
        assert_eq!(
            seen.get("STAGED_TEST_SNAPSHOT_ONLY").map(String::as_str),
            Some("1"),
            "snapshot-only vars reach npm"
        );
        let expected_cache = packages_root.join("npm-prefix").join("cache");
        assert_eq!(
            seen.get("NPM_CONFIG_CACHE").map(Path::new),
            Some(expected_cache.as_path()),
            "the managed cache overlay wins over the snapshot's"
        );
        let first_on_path = std::env::split_paths(seen.get("PATH").unwrap()).next();
        assert_eq!(
            first_on_path.as_deref(),
            Some(node_install_dir.join("bin").as_path())
        );
    }

    /// An explicit `STAGED_NPM_REGISTRY` is never second-guessed, and a
    /// failure that is not a gateway block page is not retried either; both
    /// surface the first failure, naming the registry npm used.
    #[tokio::test]
    async fn install_does_not_retry_with_an_override_or_for_other_failures() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path().join("packages");
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let tool = test_tool();
        let template = dir.path().join("template");
        std::fs::create_dir_all(&template).unwrap();
        write_fixture_install(&template, &tool, "1.2.5");
        let fallback = Some("https://fallback.example.test/npm/");

        // A gateway block page, but no fallback compiled in: plain failure.
        let argv_log = dir.path().join("no-fallback.log");
        write_fake_node_with_gateway_blocked_npm(
            &node_install_dir,
            &template,
            &argv_log,
            GATEWAY_BLOCK_STDERR,
        );
        let error = install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            None,
            None,
            super::validation::VALIDATION_TIMEOUT,
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
        assert!(
            error
                .to_string()
                .contains("(registry: npm default (per npm config))"),
            "{error}"
        );
        assert_eq!(logged_argv(&argv_log).len(), 1);

        // A 404 from whichever registry npm used: a different registry cannot
        // fix that, so no retry even with a fallback available.
        let argv_log = dir.path().join("not-found.log");
        write_fake_node_with_gateway_blocked_npm(
            &node_install_dir,
            &template,
            &argv_log,
            "npm error code E404\nnpm error 404 Not Found - GET https://mirror.example.test/npm/@agentclientprotocol%2fclaude-agent-acp",
        );
        let error = install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            None,
            fallback,
            super::validation::VALIDATION_TIMEOUT,
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(error, ManagedToolError::NpmInstall(_)), "{error}");
        assert_eq!(logged_argv(&argv_log).len(), 1);

        // An explicit override: the one and only call carries it (the fake
        // succeeds on any `--registry`), and the fallback is never consulted.
        let argv_log = dir.path().join("override.log");
        write_fake_node_with_gateway_blocked_npm(
            &node_install_dir,
            &template,
            &argv_log,
            GATEWAY_BLOCK_STDERR,
        );
        install_npm_tool(
            &packages_root,
            &node_install_dir,
            TEST_NODE_VERSION,
            &tool,
            &install_env(),
            Some("https://mine.example.test/npm/"),
            fallback,
            super::validation::VALIDATION_TIMEOUT,
            &|_| {},
        )
        .await
        .unwrap();
        let argv = logged_argv(&argv_log);
        assert_eq!(argv.len(), 1, "{argv:?}");
        assert!(
            argv[0].contains("--registry https://mine.example.test/npm/"),
            "{}",
            argv[0]
        );
    }

    // -- doctor readouts ------------------------------------------------------

    /// The live tree's `package.json` is the authority for the installed ACP
    /// version; `state.json` only stands in when that tree cannot be read.
    #[test]
    fn installed_tool_version_prefers_the_live_package_over_state() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path();
        let tool = test_tool();

        // Nothing installed, no state: unknown.
        assert_eq!(installed_tool_version(packages_root, &tool), None);

        // State alone (tree missing or unreadable): the recorded version.
        let mut state = ManagedToolsState::default();
        state.tools.insert(
            tool.id.to_string(),
            InstalledToolPin {
                binary: tool.binary.to_string(),
                version: "1.2.3".to_string(),
                node_version: TEST_NODE_VERSION.to_string(),
            },
        );
        write_state(packages_root, &state).unwrap();
        assert_eq!(
            installed_tool_version(packages_root, &tool).as_deref(),
            Some("1.2.3")
        );

        // A live tree at another version wins over stale state.
        write_fixture_install(&tool_install_dir(packages_root, tool.id), &tool, "1.3.0");
        assert_eq!(
            installed_tool_version(packages_root, &tool).as_deref(),
            Some("1.3.0")
        );

        // An empty recorded version (unreadable package.json at install
        // time) is not a version.
        std::fs::remove_dir_all(tool_install_dir(packages_root, tool.id)).unwrap();
        state.tools.get_mut(tool.id).unwrap().version.clear();
        write_state(packages_root, &state).unwrap();
        assert_eq!(installed_tool_version(packages_root, &tool), None);
    }

    /// A fake `npm` on a PATH dir: prints `version` for `npm view <pkg>
    /// version`, exits `exit_code`, and refuses unless `--registry` is passed
    /// exactly when `expect_registry` says so.
    fn write_fake_npm(bin_dir: &Path, version: &str, exit_code: i32, expect_registry: bool) {
        std::fs::create_dir_all(bin_dir).unwrap();
        let registry_check = if expect_registry {
            "case \" $* \" in *\" --registry \"*) ;; *) echo 'missing --registry' >&2; exit 43;; esac\n"
        } else {
            "case \" $* \" in *\" --registry \"*) echo 'unexpected --registry' >&2; exit 43;; esac\n"
        };
        write_executable(
            &bin_dir.join("npm"),
            &format!(
                "#!/bin/sh\ntest \"$1\" = view || exit 44\ntest \"$3\" = version || exit 45\n{registry_check}echo '{version}'\nexit {exit_code}\n"
            ),
        );
    }

    /// Writes `contents` to `path` as a 0755 script and returns only once it
    /// can actually be exec'd (see [`wait_until_executable`]).
    fn write_executable(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        wait_until_executable(path);
    }

    /// Returns only once the freshly written script at `path` can actually be
    /// exec'd. On Linux, a test on another thread that forks while our write
    /// handle is open inherits it until its own exec, and exec of a file
    /// anyone still holds open for writing fails with `ETXTBSY` — which the
    /// code under test would faithfully report and fail the test. Probe-spawn
    /// (and immediately kill) the script until that window has passed, so the
    /// script must be harmless to run with no arguments.
    fn wait_until_executable(path: &Path) {
        for _ in 0..1000 {
            match std::process::Command::new(path)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("{} is not executable: {error}", path.display()),
            }
        }
        panic!("{} stayed text-busy", path.display());
    }

    fn path_env(bin_dir: &Path) -> Vec<(String, String)> {
        vec![(
            "PATH".to_string(),
            format!("{}:/usr/bin:/bin", bin_dir.display()),
        )]
    }

    #[tokio::test]
    async fn npm_view_version_reads_the_registry_answer() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        write_fake_npm(&bin, "4.5.6", 0, true);
        let env = path_env(&bin);
        let npm = find_on_env_path(&env, "npm").expect("npm resolves from the snapshot PATH");
        assert_eq!(npm, bin.join("npm"));

        let version = npm_view_version(
            &npm,
            "@agentclientprotocol/claude-agent-acp",
            Some("https://registry.example.test/npm/"),
            &env,
            Duration::from_secs(10),
        )
        .await
        .ok();
        assert_eq!(version.as_deref(), Some("4.5.6"));
    }

    #[tokio::test]
    async fn npm_view_version_omits_the_registry_flag_when_none_is_configured() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        write_fake_npm(&bin, "4.5.6", 0, false);
        let env = path_env(&bin);

        let version = npm_view_version(
            &bin.join("npm"),
            "@agentclientprotocol/codex-acp",
            None,
            &env,
            Duration::from_secs(10),
        )
        .await
        .ok();
        assert_eq!(version.as_deref(), Some("4.5.6"));
    }

    /// Probes must not inherit npm's ~70 s dead-host retry schedule: retries
    /// are off and npm's own fetch timeout is pinned to the probe budget.
    #[tokio::test]
    async fn npm_view_version_bounds_npm_fetch_retries_and_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let argv_log = dir.path().join("argv.log");
        write_executable(
            &bin.join("npm"),
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\necho 1.0.0\n",
                argv_log.display()
            ),
        );
        let env = path_env(&bin);
        npm_view_version(
            &bin.join("npm"),
            "@agentclientprotocol/codex-acp",
            None,
            &env,
            Duration::from_secs(15),
        )
        .await
        .unwrap();
        let argv = std::fs::read_to_string(&argv_log).unwrap();
        assert!(argv.contains(" --fetch-retries=0"), "{argv}");
        assert!(argv.contains(" --fetch-timeout=15000"), "{argv}");
    }

    /// A fake `npm view` that reports a gateway block page unless `--registry`
    /// is passed, logging each invocation's argv.
    fn write_fake_gateway_blocked_npm_view(bin_dir: &Path, argv_log: &Path) {
        std::fs::create_dir_all(bin_dir).unwrap();
        write_executable(
            &bin_dir.join("npm"),
            &format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \" $* \" in *\" --registry \"*) echo 7.8.9; exit 0;; esac\necho 'npm error code FETCH_ERROR' >&2\necho 'npm error request to https://registry.npmjs.org/x failed: https://blocked.teams.cloudflare.com?block_reason=x' >&2\nexit 1\n",
                argv_log.display()
            ),
        );
    }

    /// The freshness probe follows the install's precedence: npm's own config
    /// first, then — for a gateway block with no override — one retry through
    /// the fallback.
    #[tokio::test]
    async fn probe_latest_version_retries_a_gateway_block_through_the_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let argv_log = dir.path().join("argv.log");
        write_fake_gateway_blocked_npm_view(&bin, &argv_log);
        let env = path_env(&bin);
        let package = "@agentclientprotocol/claude-agent-acp";
        let fallback = Some("https://fallback.example.test/npm/");

        let version = probe_latest_version(
            &bin.join("npm"),
            package,
            None,
            fallback,
            &env,
            Duration::from_secs(15),
        )
        .await;
        assert_eq!(version.as_deref(), Some("7.8.9"));
        let argv: Vec<String> = std::fs::read_to_string(&argv_log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(argv.len(), 2, "{argv:?}");
        assert!(!argv[0].contains("--registry"), "{}", argv[0]);
        assert!(
            argv[1].contains("--registry https://fallback.example.test/npm/"),
            "{}",
            argv[1]
        );

        // No fallback compiled in: the block page is simply unknown.
        std::fs::remove_file(&argv_log).unwrap();
        assert_eq!(
            probe_latest_version(
                &bin.join("npm"),
                package,
                None,
                None,
                &env,
                Duration::from_secs(15)
            )
            .await,
            None
        );
        assert_eq!(
            std::fs::read_to_string(&argv_log).unwrap().lines().count(),
            1
        );

        // An override is used as-is and never falls back.
        std::fs::remove_file(&argv_log).unwrap();
        assert_eq!(
            probe_latest_version(
                &bin.join("npm"),
                package,
                Some("https://mine.example.test/npm/"),
                fallback,
                &env,
                Duration::from_secs(15)
            )
            .await
            .as_deref(),
            Some("7.8.9")
        );
        let argv = std::fs::read_to_string(&argv_log).unwrap();
        assert_eq!(argv.lines().count(), 1, "{argv}");
        assert!(argv.contains("--registry https://mine.example.test/npm/"));
    }

    /// Every failure mode reads as unknown — never as a version.
    #[tokio::test]
    async fn npm_view_version_is_unknown_on_failure_garbage_or_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let env = path_env(&bin);
        let package = "@agentclientprotocol/claude-agent-acp";

        // A registry error: npm exits non-zero.
        write_fake_npm(&bin, "4.5.6", 1, false);
        assert_eq!(
            npm_view_version(
                &bin.join("npm"),
                package,
                None,
                &env,
                Duration::from_secs(10)
            )
            .await
            .ok(),
            None
        );

        // A clean exit with non-version output (an npm notice, an HTML page).
        write_fake_npm(&bin, "npm notice: try again later", 0, false);
        assert_eq!(
            npm_view_version(
                &bin.join("npm"),
                package,
                None,
                &env,
                Duration::from_secs(10)
            )
            .await
            .ok(),
            None
        );

        // A hung lookup is cut off at the bound.
        write_executable(&bin.join("npm"), "#!/bin/sh\nsleep 30\necho 9.9.9\n");
        let started = Instant::now();
        assert_eq!(
            npm_view_version(
                &bin.join("npm"),
                package,
                None,
                &env,
                Duration::from_millis(200)
            )
            .await
            .ok(),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(10));

        // No npm anywhere on the snapshot PATH: nothing to ask.
        let empty = vec![("PATH".to_string(), "/nonexistent/dir".to_string())];
        assert_eq!(find_on_env_path(&empty, "npm"), None);
        assert_eq!(find_on_env_path(&[], "npm"), None);
    }

    #[test]
    fn latest_version_cache_expires_after_the_ttl() {
        let mut cache = LatestVersionCache::default();
        let now = Instant::now();
        assert_eq!(cache.get_fresh("pkg", now), None);

        cache.insert("pkg", "1.2.3".to_string(), now);
        assert_eq!(
            cache
                .get_fresh("pkg", now + Duration::from_secs(60))
                .as_deref(),
            Some("1.2.3")
        );
        assert_eq!(cache.get_fresh("other", now), None);
        assert_eq!(
            cache.get_fresh(
                "pkg",
                now + LATEST_VERSION_CACHE_TTL + Duration::from_secs(1)
            ),
            None
        );
    }

    // -- reconcile epilogue --------------------------------------------------

    /// Lay down a complete healthy install (tree + shim + state) for `tool`.
    fn write_installed_tool(packages_root: &Path, node_install_dir: &Path, tool: &ManagedTool) {
        let install_dir = tool_install_dir(packages_root, tool.id);
        write_fixture_install(&install_dir, tool, "1.2.3");
        let entrypoint = npm_entrypoint(&install_dir, tool.package);
        write_shim(
            &shim_bin_dir(packages_root),
            tool.binary,
            &shim_contents(&node_binary(node_install_dir), &entrypoint),
        )
        .unwrap();
        let mut state = read_state(packages_root);
        state.tools.insert(
            tool.id.to_string(),
            InstalledToolPin {
                binary: tool.binary.to_string(),
                version: "1.2.3".to_string(),
                node_version: TEST_NODE_VERSION.to_string(),
            },
        );
        write_state(packages_root, &state).unwrap();
    }

    #[test]
    fn prune_removes_installs_dropped_from_the_managed_set() {
        let dir = tempfile::tempdir().unwrap();
        let packages_root = dir.path();
        let node_install_dir = packages_root.join("node").join("v9.9.9").join("plat");
        let kept = test_tool();
        let dropped = ManagedTool {
            id: "codex-acp",
            binary: "codex-acp",
            package: "@agentclientprotocol/codex-acp",
        };
        write_installed_tool(packages_root, &node_install_dir, &kept);
        write_installed_tool(packages_root, &node_install_dir, &dropped);
        // A crashed install with no state entry.
        std::fs::create_dir_all(tools_root(packages_root).join("ghost-acp")).unwrap();

        prune_stale_managed_tools(packages_root, std::slice::from_ref(&kept));

        let state = read_state(packages_root);
        assert!(state.tools.contains_key(kept.id));
        assert!(!state.tools.contains_key(dropped.id));
        assert!(shim_bin_dir(packages_root).join(kept.binary).exists());
        assert!(!shim_bin_dir(packages_root).join(dropped.binary).exists());
        assert!(tools_root(packages_root).join(kept.id).exists());
        assert!(!tools_root(packages_root).join(dropped.id).exists());
        assert!(!tools_root(packages_root).join("ghost-acp").exists());
    }

    #[test]
    fn record_reconcile_stamps_the_state() {
        let dir = tempfile::tempdir().unwrap();
        record_reconcile(dir.path(), vec!["codex-acp: boom".to_string()]);
        let record = read_state(dir.path()).last_reconcile.unwrap();
        assert!(!record.ok);
        assert_eq!(record.errors, vec!["codex-acp: boom".to_string()]);
        assert!(record.at_ms > 0);

        record_reconcile(dir.path(), Vec::new());
        let record = read_state(dir.path()).last_reconcile.unwrap();
        assert!(record.ok);
        assert!(record.errors.is_empty());
    }

    /// A packages root with the pinned Node runtime dir plus a superseded
    /// version left over from before a Node pin bump. Returns
    /// `(packages_root, pinned_dir, superseded_dir)`.
    fn write_node_bump_leftovers(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let packages_root = dir.join("packages");
        let node_root = packages_root.join("node");
        let pinned_dir = managed_node::pinned_install_dir(&node_root).unwrap();
        let superseded_dir = node_root.join("v0.0.1").join("plat");
        std::fs::create_dir_all(&pinned_dir).unwrap();
        std::fs::create_dir_all(&superseded_dir).unwrap();
        (packages_root, pinned_dir, superseded_dir)
    }

    #[tokio::test]
    async fn clean_reconcile_prunes_the_superseded_node_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let (packages_root, pinned_dir, superseded_dir) = write_node_bump_leftovers(dir.path());
        let tool = test_tool();
        write_installed_tool(&packages_root, &pinned_dir, &tool);

        finish_reconcile_at(&packages_root, std::slice::from_ref(&tool), Vec::new()).await;

        assert!(pinned_dir.exists());
        assert!(!superseded_dir.exists());
        assert!(read_state(&packages_root).last_reconcile.unwrap().ok);
    }

    #[tokio::test]
    async fn partial_failure_keeps_the_superseded_node_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let (packages_root, pinned_dir, superseded_dir) = write_node_bump_leftovers(dir.path());
        let tool = test_tool();
        // The failed bridge's shim was never rewritten: it still execs the
        // superseded runtime, which must therefore survive the epilogue.
        write_installed_tool(&packages_root, &superseded_dir, &tool);

        finish_reconcile_at(
            &packages_root,
            std::slice::from_ref(&tool),
            vec![format!("{}: npm install failed", tool.id)],
        )
        .await;

        assert!(pinned_dir.exists());
        assert!(superseded_dir.exists());
        let shim = std::fs::read_to_string(shim_bin_dir(&packages_root).join(tool.binary)).unwrap();
        assert!(shim.contains(&superseded_dir.to_string_lossy().into_owned()));
        assert!(!read_state(&packages_root).last_reconcile.unwrap().ok);
    }
}
