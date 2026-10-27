//! npm registry policy.
//!
//! Staged passes no `--registry` of its own: npm resolves the registry from
//! its normal config chain (`NPM_CONFIG_REGISTRY`, `~/.npmrc` including
//! `@scope:registry` entries, then `registry.npmjs.org`). That keeps a
//! corporate laptop whose `~/.npmrc` already points at an internal mirror —
//! with whatever auth token it needs — on that mirror, and gives everyone
//! else their own registry; a hardcoded URL served exactly one network and
//! could not even override a scoped `~/.npmrc` entry.
//!
//! Two escape hatches sit around that default. `STAGED_NPM_REGISTRY` is an
//! explicit override passed to every Staged-run npm command as `--registry`
//! *and* as the package's `--@scope:registry` ([`npm_registry_args`]): npm
//! picks a scoped package's registry from its scope entry before the default,
//! so `--registry` alone could never pull `@agentclientprotocol/*` off a
//! scoped `~/.npmrc` mirror. The fallback below is passed the same way.
//! `STAGED_FALLBACK_NPM_REGISTRY` is a *build-time* registry (compiled in via
//! `option_env!`, never written into source) for the one population npm's own
//! config fails: Block laptops with no `~/.npmrc` behind the Cloudflare
//! Gateway, where `registry.npmjs.org` answers with a redirect to an HTML
//! block page and npm reports `FETCH_ERROR`/invalid JSON. Only a failure that
//! reads as that block page is retried through the fallback — a 404 from a
//! mirror, a dead host, or an unresolvable version is not a registry problem a
//! different registry can fix, and an explicit override is never second-
//! guessed.

#[cfg(test)]
#[path = "tests/registry.rs"]
mod tests;

/// Explicit npm registry override, passed as `--registry` (and the managed
/// package's `@scope:registry`, see [`npm_registry_args`]) when set.
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
pub(super) fn registry_label(registry: Option<&str>) -> String {
    registry
        .map(str::to_string)
        .unwrap_or_else(|| "npm default (per npm config)".to_string())
}

/// The flags that force a Staged-run npm command for `package` onto
/// `registry`. `--registry` alone only sets npm's *default* registry: a
/// scoped package resolves through its `@scope:registry` config first, so a
/// `~/.npmrc` carrying `@agentclientprotocol:registry=<stale mirror>` would
/// keep winning over `STAGED_NPM_REGISTRY` and the gateway fallback alike.
/// For a scoped package the scope's own key is overridden too.
pub(super) fn npm_registry_args(package: &str, registry: &str) -> Vec<String> {
    let mut args = vec!["--registry".to_string(), registry.to_string()];
    if let Some(scope) = npm_package_scope(package) {
        args.push(format!("--{scope}:registry={registry}"));
    }
    args
}

/// The `@scope` of a scoped package name (`@scope/name`), `@` included.
fn npm_package_scope(package: &str) -> Option<&str> {
    if !package.starts_with('@') {
        return None;
    }
    package.split_once('/').map(|(scope, _)| scope)
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
    /// The Node.js dist host answered the runtime download with a 4xx — the
    /// URL (`STAGED_NODE_DIST_URL`) points at the wrong path or host.
    NodeDownloadRejected(u16),
}

/// Classify a recorded install error (see [`InstallFailureHint`]). `None`
/// when the text names no network or registry cause Doctor has advice for —
/// a validation failure, a missing entrypoint, a disk error.
pub(crate) fn classify_install_failure(error: &str) -> Option<InstallFailureHint> {
    // Checked first: `HTTP 404` from the dist host is a download URL problem,
    // not a registry without the package. A 5xx is the host's, not the
    // user's, so it gets no hint.
    if let Some(status) = node_download_http_status(error) {
        return (400..500)
            .contains(&status)
            .then_some(InstallFailureHint::NodeDownloadRejected(status));
    }
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

/// The status in a `Node.js runtime download failed: HTTP <status>` error
/// (`ManagedNodeError::HttpStatus`'s wording), if that is what `error` is.
fn node_download_http_status(error: &str) -> Option<u16> {
    let (_, rest) = error.split_once(NODE_DOWNLOAD_HTTP_STATUS_PREFIX)?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// How `ManagedNodeError::HttpStatus` renders, up to the status code.
const NODE_DOWNLOAD_HTTP_STATUS_PREFIX: &str = "Node.js runtime download failed: HTTP ";

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
        InstallFailureHint::NodeDownloadRejected(status) => {
            format!("the Node.js download URL returned HTTP {status} — check STAGED_NODE_DIST_URL")
        }
    })
}
