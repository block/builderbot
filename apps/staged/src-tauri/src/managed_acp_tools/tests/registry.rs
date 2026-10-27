use super::*;

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
        (
            "Node.js runtime download failed: HTTP 404",
            Some(InstallFailureHint::NodeDownloadRejected(404)),
        ),
        (
            "Node.js runtime download failed: HTTP 403",
            Some(InstallFailureHint::NodeDownloadRejected(403)),
        ),
        ("Node.js runtime download failed: HTTP 503", None),
        ("", None),
    ];
    for (error, expected) in cases {
        assert_eq!(classify_install_failure(error), expected, "{error}");
    }

    assert_eq!(
        install_failure_hint("Node.js runtime download failed: HTTP 404", "Node.js").as_deref(),
        Some("the Node.js download URL returned HTTP 404 — check STAGED_NODE_DIST_URL"),
        "a 404 from the dist host is a URL problem, never a missing package"
    );

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

/// `--registry` sets npm's default registry only; a scoped package follows
/// its `@scope:registry` entry first, so that key is forced alongside.
#[test]
fn npm_registry_args_override_the_packages_scope_too() {
    assert_eq!(
        npm_registry_args(
            "@agentclientprotocol/claude-agent-acp",
            "https://mine.example.test/npm/"
        ),
        vec![
            "--registry",
            "https://mine.example.test/npm/",
            "--@agentclientprotocol:registry=https://mine.example.test/npm/",
        ]
    );
    assert_eq!(
        npm_registry_args("amp-acp", "https://mine.example.test/npm/"),
        vec!["--registry", "https://mine.example.test/npm/"]
    );
    // A bare `@name` without a slash is not a scope.
    assert_eq!(npm_package_scope("@weird"), None);
    assert_eq!(npm_package_scope("@scope/name"), Some("@scope"));
}
