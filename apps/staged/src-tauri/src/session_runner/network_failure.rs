//! Was a failed prompt caused by the network?
//!
//! A turn that fails because the agent could not reach its model provider is
//! worth parking until the network returns; every other failure is not. This
//! module answers that one question from the structured ACP error stored at
//! `provider_metadata.acp.promptError` and the session's provider. It is a
//! pure function: it reads nothing but its arguments and decides nothing
//! about what happens next.
//!
//! Structured fields exclude every failure class that is not the network
//! (wrong code, a lost remote transport, a rate limit, a billing or sign-in
//! error). No version of the protocol or of the bridges marks a failure as
//! network-caused on the failed prompt itself, though, so the last split
//! rests on the error message. That fallback is confined to
//! [`matched_fallback_signal`] below. **No other file may match error prose
//! to decide whether a failure was the network.**

use acp_client::PROMPT_FAILED_PREFIX;

/// The JSON-RPC "internal error" code. Both bridges report a failed provider
/// request with it; `-32000` (authentication required) and every other code
/// are not network failures.
const INTERNAL_ERROR_CODE: i64 = -32603;

/// `data.reason` on the error Staged's own driver writes when a remote
/// session's transport closes. That loss has its own recovery.
const INCOMING_TRANSPORT_CLOSED_REASON: &str = "incoming_transport_closed";

/// The `errorKind` values the Claude bridge can put on a network failure.
/// The CLI files connection errors, timeouts and real HTTP 5xx responses
/// alike under `server_error`, and a gateway's HTML page under `unknown`.
/// Every other kind, and a missing kind, is not a network failure.
const CLAUDE_NETWORK_ERROR_KINDS: &[&str] = &["server_error", "unknown"];

/// `data.reason` Goose puts on every failed turn, network or not.
const GOOSE_TURN_FAILED_REASON: &str = "turn_failed";

/// The signal that identified a failed prompt as a network failure, for logs
/// and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetworkFailure {
    /// Claude: the CLI printed this errno token from its allow-list.
    ClaudeErrno(&'static str),
    /// Claude: the CLI's request timeout, which carries no token.
    ClaudeRequestTimeout,
    /// Claude: a proxy or gateway answered HTTP 200 with a page that is not
    /// an API response.
    ClaudeGatewayPage,
    /// Goose: its `NetworkError` variant.
    GooseNetworkError,
}

impl NetworkFailure {
    /// A stable name for the matched signal.
    pub(crate) fn signal(&self) -> &'static str {
        match self {
            Self::ClaudeErrno(token) => token,
            Self::ClaudeRequestTimeout => "request_timed_out",
            Self::ClaudeGatewayPage => "gateway_page",
            Self::GooseNetworkError => "goose_network_error",
        }
    }
}

/// Classify a failed run as a network failure, or `None` for anything else.
///
/// `driver_error` is the run's error as the driver reported it and
/// `provider_metadata` the session's stored metadata. The checks run in this
/// order, and only the last one reads error prose:
///
/// 1. The run failed on the prompt. `promptError` can be left on the row by an
///    earlier event of the same turn, so a failure that is not a prompt
///    failure must not be judged by it.
/// 2. The error code is `-32603`.
/// 3. `data.reason` is not `incoming_transport_closed`.
/// 4. The agent's own structured field allows a network cause.
/// 5. A row of the fallback table matches for that agent.
///
/// A rate limit never reaches step 5: its `errorKind` fails step 4. Callers
/// still run the rate-limit check first.
pub(crate) fn classify_network_failure(
    provider: Option<&str>,
    driver_error: &str,
    provider_metadata: Option<&serde_json::Value>,
) -> Option<NetworkFailure> {
    let agent = NetworkAgent::from_provider(provider?)?;
    if !failed_on_the_prompt(driver_error) {
        return None;
    }
    let prompt_error = provider_metadata?.pointer("/acp/promptError")?;
    if prompt_error.get("code").and_then(|code| code.as_i64()) != Some(INTERNAL_ERROR_CODE) {
        return None;
    }
    let data = prompt_error.get("data");
    let data_str = |field: &str| {
        data.and_then(|data| data.get(field))
            .and_then(|value| value.as_str())
    };
    if data_str("reason") == Some(INCOMING_TRANSPORT_CLOSED_REASON) {
        return None;
    }
    let structured_gate_passes = match agent {
        NetworkAgent::Claude => {
            data_str("errorKind").is_some_and(|kind| CLAUDE_NETWORK_ERROR_KINDS.contains(&kind))
        }
        NetworkAgent::Goose => data_str("reason") == Some(GOOSE_TURN_FAILED_REASON),
    };
    if !structured_gate_passes {
        return None;
    }
    let message = prompt_error.get("message").and_then(|m| m.as_str())?;
    matched_fallback_signal(agent, message)
}

/// The agents whose failed prompts can be classified. Codex is absent on
/// purpose: it retries a failed connection inside the turn without limit, and
/// its other failures end the turn as text, not as an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkAgent {
    Claude,
    Goose,
}

impl NetworkAgent {
    fn from_provider(provider: &str) -> Option<Self> {
        match provider {
            "claude" => Some(Self::Claude),
            "goose" => Some(Self::Goose),
            _ => None,
        }
    }
}

/// Whether the driver reported a failed `session/prompt`.
///
/// The driver's text starts with its own [`PROMPT_FAILED_PREFIX`]. The
/// connection that carried the prompt usually hands it on wrapped once, as
/// `ACP protocol failed: Error { ..., data: Some(String("Prompt failed: ...")) }`,
/// so the outermost `data` string is accepted too. Both forms are Staged's
/// own rendering, not agent prose. A deeper occurrence does not count: that
/// would be an agent quoting the words.
fn failed_on_the_prompt(driver_error: &str) -> bool {
    if driver_error.starts_with(PROMPT_FAILED_PREFIX) {
        return true;
    }
    // Outermost first, and only an unescaped (top-level) string counts.
    const WRAPPED: &str = "data: Some(String(\"";
    driver_error
        .find(WRAPPED)
        .is_some_and(|at| driver_error[at + WRAPPED.len()..].starts_with(PROMPT_FAILED_PREFIX))
}

// ---------------------------------------------------------------------------
// The fallback table: the only place that reads error prose.
//
// Each row applies only after the structured checks in
// `classify_network_failure` have passed, and only for its own agent. Each
// exists because the agent forwards no structured cause for that failure.
// When the Claude bridge starts copying the SDK's `api_error_status` into the
// prompt error's `data`, check that field ahead of this table and delete the
// Claude rows it covers.
// ---------------------------------------------------------------------------

/// Claude row 1: an errno token in parentheses at the end of the message.
///
/// The CLI takes the raw `code` from the error chain and prints it in
/// parentheses, so the token is a machine string even though it sits inside
/// prose; the bridge forwards no cause field. Only these tokens count. TLS
/// certificate codes and `ERR_PROXY_TUNNEL` are left out because waiting for
/// the network does not fix them: they need the user.
const CLAUDE_NETWORK_ERRNO_TOKENS: &[&str] = &[
    "ENOTFOUND",
    "EAI_AGAIN",
    "FailedToOpenSocket",
    "ECONNREFUSED",
    "ConnectionRefused",
    "ECONNRESET",
    "EPIPE",
    "ECONNABORTED",
    "ConnectionClosed",
    "ERR_SOCKET_CLOSED",
    "UND_ERR_SOCKET",
    "ENETUNREACH",
    "ENETDOWN",
    "EHOSTUNREACH",
    "EHOSTDOWN",
    "ETIMEDOUT",
];

/// The prefix the Claude bridge puts on every failed provider request.
const CLAUDE_INTERNAL_ERROR_PREFIX: &str = "Internal error: ";

/// Claude row 2: the whole message after [`CLAUDE_INTERNAL_ERROR_PREFIX`].
/// A fixed English constant in the CLI, which emits no token for a timeout.
const CLAUDE_REQUEST_TIMED_OUT: &str = "Request timed out";

/// Claude row 3: both must appear. A gateway or captive-portal page answered
/// in place of the API; the bridge has no kind for it beyond `unknown`.
const CLAUDE_GATEWAY_PAGE_MARKERS: [&str; 2] = ["(HTTP 200)", "empty or malformed response"];

/// Goose row: the `Display` prefix of Goose's internal `NetworkError`
/// variant. `data.reason` is `turn_failed` for every Goose failure, so
/// nothing structured separates this from a 5xx or a rate limit.
const GOOSE_NETWORK_ERROR_PREFIX: &str = "Network error: ";

fn matched_fallback_signal(agent: NetworkAgent, message: &str) -> Option<NetworkFailure> {
    let message = message.trim();
    match agent {
        NetworkAgent::Claude => {
            if let Some(token) = trailing_parenthesized_token(message)
                .and_then(|token| CLAUDE_NETWORK_ERRNO_TOKENS.iter().find(|t| **t == token))
            {
                return Some(NetworkFailure::ClaudeErrno(token));
            }
            if message.strip_prefix(CLAUDE_INTERNAL_ERROR_PREFIX) == Some(CLAUDE_REQUEST_TIMED_OUT)
            {
                return Some(NetworkFailure::ClaudeRequestTimeout);
            }
            if CLAUDE_GATEWAY_PAGE_MARKERS
                .iter()
                .all(|marker| message.contains(marker))
            {
                return Some(NetworkFailure::ClaudeGatewayPage);
            }
            None
        }
        NetworkAgent::Goose => message
            .starts_with(GOOSE_NETWORK_ERROR_PREFIX)
            .then_some(NetworkFailure::GooseNetworkError),
    }
}

/// The text inside a `(...)` that ends the message, if the message ends
/// with one.
fn trailing_parenthesized_token(message: &str) -> Option<&str> {
    let inner = message.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    Some(&inner[open + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DNS_MESSAGE: &str = "Internal error: API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)";
    const REFUSED_MESSAGE: &str = "Internal error: API Error: Connection refused — a firewall or proxy may be blocking it (ECONNREFUSED)";
    const TIMEOUT_MESSAGE: &str = "Internal error: Request timed out";
    const GATEWAY_MESSAGE: &str = "Internal error: API Error: API returned an empty or malformed response (HTTP 200) — check for a proxy or gateway intercepting the request. Response: content-type html, body is an HTML page, 62825 bytes, request-id absent, server cloudflare, intermediary headers cf-ray content-encoding transfer-encoding. This was the non-streaming retry of streaming request (no Anthropic request-id), which failed with: no_events, StreamNoEventsError; 0 stream events received.";

    fn metadata(prompt_error: serde_json::Value) -> serde_json::Value {
        json!({ "acp": { "promptError": prompt_error } })
    }

    fn claude_error(kind: &str, message: &str) -> serde_json::Value {
        metadata(json!({ "code": -32603, "message": message, "data": { "errorKind": kind } }))
    }

    fn goose_error(message: &str) -> serde_json::Value {
        metadata(json!({ "code": -32603, "message": message, "data": { "reason": "turn_failed" } }))
    }

    /// The driver error as the runner receives it today: the prompt failure
    /// wrapped once by the connection that carried it.
    fn wrapped_driver_error(message: &str) -> String {
        let inner = format!(
            r#"Prompt failed: Error {{ code: -32603: Internal error, message: {message:?}, data: None }}"#
        );
        format!(
            r#"ACP protocol failed: Error {{ code: -32603: Internal error, message: "Internal error", data: Some(String({inner:?})) }}"#
        )
    }

    fn classify(provider: &str, metadata: &serde_json::Value) -> Option<NetworkFailure> {
        let message = metadata
            .pointer("/acp/promptError/message")
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        classify_network_failure(
            Some(provider),
            &wrapped_driver_error(message),
            Some(metadata),
        )
    }

    #[test]
    fn the_four_stored_claude_network_payloads_classify_as_network() {
        assert_eq!(
            classify("claude", &claude_error("server_error", DNS_MESSAGE)),
            Some(NetworkFailure::ClaudeErrno("ENOTFOUND"))
        );
        assert_eq!(
            classify("claude", &claude_error("server_error", REFUSED_MESSAGE)),
            Some(NetworkFailure::ClaudeErrno("ECONNREFUSED"))
        );
        // The stored timeout row carries `unknown`; `server_error` is what
        // the CLI assigns today.
        for kind in ["unknown", "server_error"] {
            assert_eq!(
                classify("claude", &claude_error(kind, TIMEOUT_MESSAGE)),
                Some(NetworkFailure::ClaudeRequestTimeout),
                "{kind}"
            );
        }
        assert_eq!(
            classify("claude", &claude_error("unknown", GATEWAY_MESSAGE)),
            Some(NetworkFailure::ClaudeGatewayPage)
        );
    }

    #[test]
    fn every_allow_listed_errno_token_matches_and_names_itself() {
        for token in CLAUDE_NETWORK_ERRNO_TOKENS {
            let message = format!("Internal error: API Error: Connection error ({token})");
            let failure = classify("claude", &claude_error("server_error", &message))
                .unwrap_or_else(|| panic!("{token} should classify"));
            assert_eq!(failure, NetworkFailure::ClaudeErrno(token));
            assert_eq!(failure.signal(), *token);
        }
    }

    #[test]
    fn the_bare_and_the_wrapped_driver_error_both_count_as_a_prompt_failure() {
        let metadata = claude_error("server_error", DNS_MESSAGE);
        let bare = format!(
            r#"Prompt failed: Error {{ code: -32603: Internal error, message: {DNS_MESSAGE:?}, data: Some(Object {{"errorKind": String("server_error")}}) }}"#
        );
        for driver_error in [bare, wrapped_driver_error(DNS_MESSAGE)] {
            assert_eq!(
                classify_network_failure(Some("claude"), &driver_error, Some(&metadata)),
                Some(NetworkFailure::ClaudeErrno("ENOTFOUND")),
                "{driver_error}"
            );
        }
    }

    #[test]
    fn a_real_5xx_without_a_token_is_not_a_network_failure() {
        for message in [
            "Internal error: API Error: 500 Internal server error",
            "Internal error: API Error: Repeated 529 Overloaded errors",
            // A token that is not at the end, or not in parentheses, is prose.
            "Internal error: API Error: upstream said ECONNRESET while proxying",
            "Internal error: API Error: (ECONNRESET) reported by upstream, HTTP 502",
            // Not the whole message.
            "Internal error: Request timed out while waiting for tool output",
            "Internal error: API Error: Request timed out",
            // One gateway marker without the other.
            "Internal error: API Error: unexpected body (HTTP 200)",
        ] {
            assert_eq!(
                classify("claude", &claude_error("server_error", message)),
                None,
                "{message}"
            );
        }
    }

    #[test]
    fn a_tls_or_proxy_tunnel_code_needs_the_user_and_is_not_paused() {
        for token in [
            "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
            "SELF_SIGNED_CERT_IN_CHAIN",
            "CERT_HAS_EXPIRED",
            "ERR_PROXY_TUNNEL",
        ] {
            let message = format!("Internal error: API Error: Connection error ({token})");
            assert_eq!(
                classify("claude", &claude_error("server_error", &message)),
                None,
                "{token}"
            );
        }
    }

    #[test]
    fn every_other_claude_error_kind_is_excluded_even_with_a_network_message() {
        for kind in [
            "authentication_failed",
            "oauth_org_not_allowed",
            "account_on_hold",
            "verification_required",
            "billing_error",
            "rate_limit",
            "overloaded",
            "invalid_request",
            "model_not_found",
            "max_output_tokens",
            "cloud_credential_error",
            "incomplete_tool_call",
            "no_result",
            "worker_shutdown",
            "transport_lost",
        ] {
            assert_eq!(
                classify("claude", &claude_error(kind, DNS_MESSAGE)),
                None,
                "{kind}"
            );
        }
    }

    #[test]
    fn a_missing_or_non_string_claude_error_kind_is_excluded() {
        for data in [
            json!(null),
            json!({}),
            json!({ "errorKind": 7 }),
            json!("server_error"),
        ] {
            let metadata = metadata(json!({
                "code": -32603,
                "message": DNS_MESSAGE,
                "data": data,
            }));
            assert_eq!(classify("claude", &metadata), None, "{data}");
        }
        let no_data = metadata(json!({ "code": -32603, "message": TIMEOUT_MESSAGE }));
        assert_eq!(classify("claude", &no_data), None);
    }

    #[test]
    fn a_rate_limit_is_never_a_network_failure() {
        let metadata = metadata(json!({
            "code": -32603,
            "message": "Internal error: API Error: Rate limited (ETIMEDOUT)",
            "data": { "errorKind": "rate_limit", "rateLimit": { "resetsAt": 1_800_000_000 } },
        }));
        assert_eq!(classify("claude", &metadata), None);
    }

    #[test]
    fn any_code_other_than_internal_error_is_excluded() {
        let auth = metadata(json!({ "code": -32000, "message": "Authentication required" }));
        assert_eq!(classify("claude", &auth), None);
        for code in [
            json!(-32000),
            json!(-32602),
            json!(-32002),
            json!("-32603"),
            json!(null),
        ] {
            let metadata = metadata(json!({
                "code": code,
                "message": DNS_MESSAGE,
                "data": { "errorKind": "server_error" },
            }));
            assert_eq!(classify("claude", &metadata), None, "{code}");
        }
    }

    #[test]
    fn a_lost_remote_transport_is_excluded() {
        // As stored: written by Staged's own driver, with no error kind.
        let stored = metadata(json!({
            "code": -32603,
            "message": "Incoming transport closed",
            "data": { "reason": "incoming_transport_closed", "method": "session/prompt" },
        }));
        // Even if a kind and a network message rode along with it.
        let with_kind = metadata(json!({
            "code": -32603,
            "message": DNS_MESSAGE,
            "data": { "reason": "incoming_transport_closed", "errorKind": "server_error" },
        }));
        for provider in ["claude", "goose"] {
            assert_eq!(classify(provider, &stored), None, "{provider}");
            assert_eq!(classify(provider, &with_kind), None, "{provider}");
        }
    }

    #[test]
    fn a_stale_prompt_error_is_ignored_when_the_run_did_not_fail_on_the_prompt() {
        let metadata = claude_error("server_error", DNS_MESSAGE);
        for driver_error in [
            "Timed out waiting for ACP protocol startup after 90s".to_string(),
            "ACP protocol failed: server shut down unexpectedly".to_string(),
            format!(r#"Prompt failed after interrupt: Error {{ message: {DNS_MESSAGE:?} }}"#),
            format!(r#"Prompt failed after cancellation: Error {{ message: {DNS_MESSAGE:?} }}"#),
            r#"ACP protocol failed: Error { code: -32603: Internal error, message: "Internal error", data: Some(String("ACP authentication is required to run the prompt. Sign this agent in, then retry.")) }"#.to_string(),
            // The words quoted deeper down are an agent's, not the driver's.
            r#"ACP protocol failed: Error { code: -32603: Internal error, message: "Internal error", data: Some(String("Failed to load session: Error { data: Some(String(\"Prompt failed: nope\")) }")) }"#.to_string(),
            r#"Failed to load session: Error { message: "Prompt failed: (ENOTFOUND)", data: None }"#.to_string(),
            String::new(),
        ] {
            assert_eq!(
                classify_network_failure(Some("claude"), &driver_error, Some(&metadata)),
                None,
                "{driver_error}"
            );
        }
    }

    #[test]
    fn a_prompt_failure_without_a_stored_prompt_error_is_not_classified() {
        let driver_error = wrapped_driver_error(DNS_MESSAGE);
        assert_eq!(
            classify_network_failure(Some("claude"), &driver_error, None),
            None
        );
        assert_eq!(
            classify_network_failure(Some("claude"), &driver_error, Some(&json!({ "acp": {} }))),
            None
        );
    }

    #[test]
    fn goose_network_errors_classify_and_goose_server_errors_do_not() {
        let network = goose_error(
            "Network error: error sending request for url (https://api.example.com/v1/messages)",
        );
        let failure = classify("goose", &network);
        assert_eq!(failure, Some(NetworkFailure::GooseNetworkError));
        assert_eq!(failure.unwrap().signal(), "goose_network_error");

        for message in [
            "Server error: 503 Service Unavailable",
            "Rate limit exceeded: Network error: try later",
            "Request failed: Network error: mentioned later (ENOTFOUND)",
        ] {
            assert_eq!(classify("goose", &goose_error(message)), None, "{message}");
        }
    }

    #[test]
    fn goose_needs_its_own_turn_failed_reason() {
        for data in [
            json!({ "reason": "credits_exhausted" }),
            json!({ "errorKind": "server_error" }),
            json!(null),
        ] {
            let metadata = metadata(json!({
                "code": -32603,
                "message": "Network error: connection reset",
                "data": data,
            }));
            assert_eq!(classify("goose", &metadata), None, "{data}");
        }
    }

    #[test]
    fn each_fallback_row_applies_only_to_its_own_agent() {
        // Goose's prefix under Claude's gate, and Claude's rows under Goose's.
        assert_eq!(
            classify(
                "claude",
                &claude_error("server_error", "Network error: connection reset")
            ),
            None
        );
        for message in [DNS_MESSAGE, TIMEOUT_MESSAGE, GATEWAY_MESSAGE] {
            assert_eq!(classify("goose", &goose_error(message)), None, "{message}");
        }
    }

    #[test]
    fn codex_and_unknown_providers_are_never_classified() {
        let mut metadata = claude_error("server_error", DNS_MESSAGE);
        metadata["acp"]["promptError"]["data"]["reason"] = json!("turn_failed");
        let driver_error = wrapped_driver_error(DNS_MESSAGE);
        for provider in [Some("codex"), Some("amp"), Some(""), None] {
            assert_eq!(
                classify_network_failure(provider, &driver_error, Some(&metadata)),
                None,
                "{provider:?}"
            );
        }
    }
}
