//! What `rate_limit_recovery_details` makes of the limits that need a wait
//! of hours or days, and of the failures that must not be given one:
//!
//! - a weekly limit, whose reset is days ahead;
//! - a subscription usage limit that reports no reset and no limit type,
//!   known only by its wording;
//! - an API 429 or an overloaded provider, which keep their own kind
//!   whatever rate-limit info the turn left behind, and wait for no reset
//!   more than 48 hours ahead;
//! - a usage limit that only a person can clear, which is not waited out.
//!
//! The prompt errors are shaped as claude-agent-acp 0.109.0 builds them
//! (`turnErrorData`) around the SDK's `SDKRateLimitInfo`, and the messages
//! are the wordings the CLI and the provider APIs are known to use. None is
//! captured from a live limit.

use super::*;

/// 2026-06-30T19:00:00Z in epoch seconds: six days and four hours after
/// [`RESET_TEST_NOW_MILLIS`].
const WEEKLY_RESETS_AT_SECS: i64 = 1_782_846_000;

const API_429_MESSAGE: &str = r#"Internal error: API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your organization's rate limit of 400,000 input tokens per minute."}}"#;
const OVERLOADED_MESSAGE: &str = r#"Internal error: API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;

fn rejected_weekly_limit() -> serde_json::Value {
    serde_json::json!({
        "status": "rejected",
        "resetsAt": WEEKLY_RESETS_AT_SECS,
        "rateLimitType": "seven_day",
        "overageStatus": "rejected",
        "overageDisabledReason": "org_level_disabled",
        "isUsingOverage": false,
    })
}

/// The five-hour window while it is still open: what a `usage_update` earlier
/// in the turn carries when the account is close to its limit, not over it.
fn open_five_hour_window() -> serde_json::Value {
    serde_json::json!({
        "status": "allowed_warning",
        "resetsAt": 1_782_315_900,
        "rateLimitType": "five_hour",
        "utilization": 0.93,
    })
}

/// Provider metadata for a prompt that failed with `message` and the
/// bridge's `error_kind`, with no rate-limit info.
fn prompt_error(error_kind: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "acp": { "promptError": {
        "code": -32603,
        "message": message,
        "data": { "errorKind": error_kind },
    } } })
}

fn recovery(error: &str, metadata: Option<&serde_json::Value>) -> RateLimitRecovery {
    rate_limit_recovery_details(error, metadata, RESET_TEST_NOW_MILLIS)
}

#[test]
fn a_weekly_reset_days_ahead_is_kept() {
    // More than the 48 hours any other kind is trusted for.
    let metadata = claude_limit_metadata(Some(rejected_weekly_limit()), None);
    let error = "Prompt failed: You've hit your weekly limit · resets Jun 30, 7pm (UTC)";
    assert!(is_rate_limit_error(error, Some(&metadata)));

    let details = recovery(error, Some(&metadata));

    assert_eq!(
        details.resets_at.as_deref(),
        Some("2026-06-30T19:00:00.000Z")
    );
    assert_eq!(details.kind.as_deref(), Some("seven_day"));
}

#[test]
fn a_weekly_limit_with_no_reset_keeps_its_limit_type() {
    // `resetsAt` is optional in `SDKRateLimitInfo`. The limit type the
    // bridge names is more exact than the wording, so it stands.
    let metadata = claude_limit_metadata(
        Some(serde_json::json!({ "status": "rejected", "rateLimitType": "seven_day_opus" })),
        None,
    );

    let details = recovery("Prompt failed", Some(&metadata));

    assert_eq!(details.resets_at, None);
    assert_eq!(details.kind.as_deref(), Some("seven_day_opus"));
}

#[test]
fn the_wording_of_a_reached_usage_limit_names_the_kind() {
    for message in [
        "Internal error: You've hit your session limit · resets 3:45pm (UTC)",
        "Internal error: You've hit your weekly limit · resets Jun 30, 7pm (UTC)",
        "Internal error: You've hit your Opus limit · resets Jun 30, 7pm (UTC)",
        "Internal error: You\u{2019}ve hit your session limit",
        "Internal error: You've reached your weekly limit",
        "Internal error: You're out of extra usage · resets 3:45pm (UTC)",
        "Internal error: Claude usage limit reached. Your limit will reset at 3pm (UTC).",
    ] {
        let metadata = prompt_error("rate_limit", message);
        let details = recovery("Prompt failed", Some(&metadata));
        assert_eq!(details.resets_at, None, "{message}");
        assert_eq!(details.kind.as_deref(), Some("usage_limit"), "{message}");
    }
}

#[test]
fn a_bare_status_gives_way_to_the_wording() {
    // Rate-limit info with neither a limit type nor a reset: the message is
    // the better name for the kind.
    let metadata = claude_limit_metadata(Some(serde_json::json!({ "status": "rejected" })), None);

    let details = recovery("Prompt failed", Some(&metadata));

    assert_eq!(details.resets_at, None);
    assert_eq!(details.kind.as_deref(), Some("usage_limit"));
}

#[test]
fn an_api_rate_limit_keeps_its_own_kind() {
    let metadata = prompt_error("rate_limit", API_429_MESSAGE);
    let error = format!("Prompt failed: {API_429_MESSAGE}");
    assert!(is_rate_limit_error(&error, Some(&metadata)));

    let details = recovery(&error, Some(&metadata));

    assert_eq!(details.resets_at, None);
    assert_eq!(details.kind.as_deref(), Some("rate_limit"));
}

#[test]
fn an_overloaded_provider_keeps_its_own_kind() {
    let metadata = prompt_error("overloaded", OVERLOADED_MESSAGE);
    let error = format!("Prompt failed: {OVERLOADED_MESSAGE}");
    assert!(is_rate_limit_error(&error, Some(&metadata)));

    let details = recovery(&error, Some(&metadata));
    assert_eq!(details.resets_at, None);
    assert_eq!(details.kind.as_deref(), Some("overloaded"));

    // The bridge's kind stands even against usage-limit wording.
    let metadata = prompt_error(
        "overloaded",
        "Internal error: You've hit your session limit",
    );
    let details = recovery("Prompt failed", Some(&metadata));
    assert_eq!(details.kind.as_deref(), Some("overloaded"));
}

#[test]
fn the_info_of_a_window_still_open_gives_no_reset_and_no_kind() {
    // The turn sent a usage update while the five-hour window was close to
    // its limit, then failed on something else. That window's end is not
    // when an overloaded provider or an API 429 clears.
    for (error_kind, message) in [
        ("overloaded", OVERLOADED_MESSAGE),
        ("rate_limit", API_429_MESSAGE),
    ] {
        let mut metadata = prompt_error(error_kind, message);
        metadata["acp"]["usageUpdate"] = serde_json::json!({
            "sessionUpdate": "usage_update",
            "used": 51_234,
            "size": 200_000,
            "_meta": { "_claude/rateLimit": open_five_hour_window() },
        });

        let details = recovery("Prompt failed", Some(&metadata));

        assert_eq!(details.resets_at, None, "{error_kind}");
        assert_eq!(details.kind.as_deref(), Some(error_kind), "{error_kind}");
    }

    // The same info on the prompt error itself, and with `allowed`.
    for status in ["allowed", "allowed_warning"] {
        let mut open = open_five_hour_window();
        open["status"] = serde_json::json!(status);
        let mut metadata = prompt_error("rate_limit", API_429_MESSAGE);
        metadata["acp"]["promptError"]["data"]["rateLimit"] = open;

        let details = recovery("Prompt failed", Some(&metadata));

        assert_eq!(details.resets_at, None, "{status}");
        assert_eq!(details.kind.as_deref(), Some("rate_limit"), "{status}");
    }
}

/// The weekly window of an account drawing on extra usage: the base window
/// stays `rejected` while the account's turns run.
fn weekly_window_on_extra_usage(flag: &str) -> serde_json::Value {
    let mut window = rejected_weekly_limit();
    window["overageStatus"] = serde_json::json!("allowed");
    window["isUsingOverage"] = serde_json::json!(false);
    window[flag] = serde_json::json!(true);
    window
}

fn usage_update_with(rate_limit: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "sessionUpdate": "usage_update",
        "used": 51_234,
        "size": 200_000,
        "_meta": { "_claude/rateLimit": rate_limit },
    })
}

#[test]
fn the_info_of_an_account_on_extra_usage_gives_no_reset_and_no_kind() {
    // The turn ran on extra usage and died on an API 429 or a 529. The base
    // window's end, six days on, is not when either clears.
    for flag in ["isUsingOverage", "overageInUse"] {
        for (error_kind, message) in [
            ("rate_limit", API_429_MESSAGE),
            ("overloaded", OVERLOADED_MESSAGE),
        ] {
            let mut metadata = prompt_error(error_kind, message);
            metadata["acp"]["usageUpdate"] = usage_update_with(weekly_window_on_extra_usage(flag));

            let details = recovery("Prompt failed", Some(&metadata));

            assert_eq!(details.resets_at, None, "{flag} {error_kind}");
            assert_eq!(
                details.kind.as_deref(),
                Some(error_kind),
                "{flag} {error_kind}"
            );
        }

        // The same info on the prompt error itself.
        let mut metadata = prompt_error("rate_limit", API_429_MESSAGE);
        metadata["acp"]["promptError"]["data"]["rateLimit"] = weekly_window_on_extra_usage(flag);

        let details = recovery("Prompt failed", Some(&metadata));

        assert_eq!(details.resets_at, None, "{flag}");
        assert_eq!(details.kind.as_deref(), Some("rate_limit"), "{flag}");
    }

    // With both flags false the same window is a reached limit.
    let mut reached = weekly_window_on_extra_usage("isUsingOverage");
    reached["isUsingOverage"] = serde_json::json!(false);
    reached["overageInUse"] = serde_json::json!(false);
    let metadata = claude_limit_metadata(Some(reached), None);
    assert_eq!(
        recovery("Prompt failed", Some(&metadata)).kind.as_deref(),
        Some("seven_day")
    );
}

#[test]
fn an_overloaded_prompt_reads_nothing_from_the_usage_update() {
    // A usage update that says the weekly limit was reached, with no sign
    // of extra usage. The bridge failed the prompt as overloaded, and a 529
    // does not clear when a subscription window ends.
    let mut metadata = prompt_error("overloaded", OVERLOADED_MESSAGE);
    metadata["acp"]["usageUpdate"] = usage_update_with(rejected_weekly_limit());
    let error = format!("Prompt failed: {OVERLOADED_MESSAGE}");

    let details = recovery(&error, Some(&metadata));

    assert_eq!(details.resets_at, None);
    assert_eq!(details.kind.as_deref(), Some("overloaded"));

    // For a `rate_limit` failure the same usage update is the limit.
    let mut metadata = prompt_error("rate_limit", "Internal error");
    metadata["acp"]["usageUpdate"] = usage_update_with(rejected_weekly_limit());

    let details = recovery("Prompt failed", Some(&metadata));

    assert_eq!(
        details.resets_at.as_deref(),
        Some("2026-06-30T19:00:00.000Z")
    );
    assert_eq!(details.kind.as_deref(), Some("seven_day"));
}

#[test]
fn info_with_no_status_is_not_a_reached_limit() {
    // Nothing says the window was closed, so its reset and limit type are
    // not read, on the prompt error or the usage update.
    let no_status = serde_json::json!({
        "resetsAt": WEEKLY_RESETS_AT_SECS,
        "rateLimitType": "seven_day",
    });
    for status in [None, Some(""), Some("  ")] {
        let mut info = no_status.clone();
        if let Some(status) = status {
            info["status"] = serde_json::json!(status);
        }
        let mut metadata = prompt_error("rate_limit", API_429_MESSAGE);
        metadata["acp"]["promptError"]["data"]["rateLimit"] = info.clone();
        metadata["acp"]["usageUpdate"] = usage_update_with(info);

        let details = recovery("Prompt failed", Some(&metadata));

        assert_eq!(details.resets_at, None, "{status:?}");
        assert_eq!(details.kind.as_deref(), Some("rate_limit"), "{status:?}");
    }
}

#[test]
fn a_reset_days_ahead_is_kept_only_for_a_usage_limit() {
    // The reset at the top level of the prompt error's data is read for
    // every kind. Six days on, an API 429 and an overloaded provider do not
    // keep it: nothing would wait for it.
    for (error_kind, message) in [
        ("rate_limit", API_429_MESSAGE),
        ("overloaded", OVERLOADED_MESSAGE),
    ] {
        for field in ["resetsAt", "retryAt"] {
            let mut metadata = prompt_error(error_kind, message);
            metadata["acp"]["promptError"]["data"][field] =
                serde_json::json!(WEEKLY_RESETS_AT_SECS);
            let details = recovery("Prompt failed", Some(&metadata));
            assert_eq!(details.resets_at, None, "{error_kind} {field}");
            assert_eq!(details.kind.as_deref(), Some(error_kind));

            // As an RFC3339 string, which is stored as written.
            metadata["acp"]["promptError"]["data"][field] =
                serde_json::json!("2026-06-30T19:00:00Z");
            let details = recovery("Prompt failed", Some(&metadata));
            assert_eq!(details.resets_at, None, "{error_kind} {field}");

            // Within 48 hours it stands, as it did.
            metadata["acp"]["promptError"]["data"][field] = serde_json::json!(1_782_315_900);
            let details = recovery("Prompt failed", Some(&metadata));
            assert_eq!(
                details.resets_at.as_deref(),
                Some("2026-06-24T15:45:00.000Z"),
                "{error_kind} {field}"
            );
        }
    }

    // A usage limit known by its wording keeps the same reset.
    let mut metadata = prompt_error(
        "rate_limit",
        "Internal error: You've hit your weekly limit · resets Jun 30, 7pm (UTC)",
    );
    metadata["acp"]["promptError"]["data"]["resetsAt"] = serde_json::json!(WEEKLY_RESETS_AT_SECS);
    let details = recovery("Prompt failed", Some(&metadata));
    assert_eq!(
        details.resets_at.as_deref(),
        Some("2026-06-30T19:00:00.000Z")
    );
    assert_eq!(details.kind.as_deref(), Some("usage_limit"));
}

#[test]
fn a_usage_limit_that_needs_a_person_is_not_named_one() {
    // The rest of the SDK's `USAGE_LIMIT_ERROR_PREFIXES` (0.3.328): credits
    // to buy or a setting an admin has to change. Waiting clears none of
    // them, so they keep the kind and the backoff they had.
    for message in [
        "You're out of usage credits",
        "Your org is out of usage · add funds to continue",
        "Your org is out of usage · contact your admin",
        "Your seat type doesn't include usage credits",
        "Your seat type doesn't include usage",
        "Your usage allocation has been disabled by your admin",
        "Your group's usage limit is set to $0",
        "Your group\u{2019}s usage limit is set to $0",
        "Fable 5 requires usage credits",
        "Your seat type doesn't include extra usage",
    ] {
        let metadata = prompt_error("rate_limit", &format!("Internal error: {message}"));
        let details = recovery("Prompt failed", Some(&metadata));
        assert_eq!(details.kind.as_deref(), Some("rate_limit"), "{message}");

        // And as text alone.
        assert_ne!(
            recovery(message, None).kind.as_deref(),
            Some("usage_limit"),
            "{message}"
        );
    }
}

#[test]
fn api_wording_behind_a_usage_limit_phrase_is_still_an_api_limit() {
    for message in [
        "You've hit your rate limit. Please try again later.",
        "You've reached your rate limit for requests per minute",
        "Rate limit reached for requests",
        "5-hour limit reached ∙ resets 3pm",
    ] {
        let metadata = prompt_error("rate_limit", &format!("Internal error: {message}"));
        let details = recovery("Prompt failed", Some(&metadata));
        assert_eq!(details.kind.as_deref(), Some("rate_limit"), "{message}");
    }
}

#[test]
fn a_reached_limit_is_read_past_the_info_of_an_open_window() {
    // The prompt error still describes the five-hour window as open; the
    // usage update is the one that says the weekly limit was reached.
    let metadata =
        claude_limit_metadata(Some(open_five_hour_window()), Some(rejected_weekly_limit()));

    let details = recovery("Prompt failed", Some(&metadata));

    assert_eq!(
        details.resets_at.as_deref(),
        Some("2026-06-30T19:00:00.000Z")
    );
    assert_eq!(details.kind.as_deref(), Some("seven_day"));
}

#[test]
fn the_wording_is_read_from_the_prompt_error_not_the_text_around_it() {
    // The driver error can carry agent stderr and model output. An API 429
    // in a session that happens to be talking about usage limits is still
    // an API 429.
    let metadata = prompt_error("rate_limit", API_429_MESSAGE);
    let error = format!(
        "Prompt failed: {API_429_MESSAGE}\nstderr: test \"You've hit your session limit\" passed"
    );

    let details = recovery(&error, Some(&metadata));

    assert_eq!(details.kind.as_deref(), Some("rate_limit"));
}

#[test]
fn with_no_metadata_the_wording_is_read_from_the_innermost_sentence() {
    // No prompt error was recorded, as for Codex and any agent that fails
    // with text alone. The driver error is the SDK error's `Debug` with the
    // agent's stderr after it, where a session working on limit code quotes
    // the wording. What failed is an API 429.
    let quoting = concat!(
        r#"ACP protocol failed: Error { code: -32603, message: "Internal error", "#,
        r#"data: Some(String("Prompt failed: Error { code: -32000, "#,
        r#"message: \"stream error: HTTP 429 Too Many Requests\", data: None }")) }"#,
        "\nstderr: test \"You've hit your session limit\" passed\n",
        "assistant: the usage limit copy now reads \"You've reached your weekly limit\"",
    );
    assert!(is_rate_limit_error(quoting, None));
    assert_eq!(
        innermost_error_sentence(quoting).as_deref(),
        Some("stream error: HTTP 429 Too Many Requests")
    );

    let details = recovery(quoting, None);

    assert_eq!(details.resets_at, None);
    assert_eq!(details.kind.as_deref(), Some("rate_limit"));

    // The same error with the usage limit as what failed.
    let reached = concat!(
        r#"ACP protocol failed: Error { code: -32603, message: "Internal error", "#,
        r#"data: Some(String("Prompt failed: Error { code: -32000, "#,
        r#"message: \"You've hit your usage limit. Try again at 12:58 PM.\", data: None }")) }"#,
        "\nstderr: HTTP 429 Too Many Requests",
    );
    assert_eq!(recovery(reached, None).kind.as_deref(), Some("usage_limit"));

    // A prompt error recorded without a message is read the same way.
    let no_message = serde_json::json!({ "acp": { "promptError": {
        "code": -32603,
        "data": { "errorKind": "rate_limit" },
    } } });
    assert_eq!(
        recovery(quoting, Some(&no_message)).kind.as_deref(),
        Some("rate_limit")
    );
    assert_eq!(
        recovery(reached, Some(&no_message)).kind.as_deref(),
        Some("usage_limit")
    );
}

#[test]
fn an_error_with_no_metadata_is_named_by_its_text() {
    // Codex reports a subscription limit as text alone.
    let codex =
        "You've hit your usage limit. Upgrade to Pro or try again at Jul 12th, 2026 12:58 PM.";
    assert_eq!(recovery(codex, None).kind.as_deref(), Some("usage_limit"));
    assert_eq!(recovery(codex, None).resets_at, None);

    // API wordings are not usage limits, "quota" included.
    for api in [
        "HTTP 429 returned by provider",
        "Too many requests",
        "You exceeded your current quota",
        "RESOURCE_EXHAUSTED: Quota exceeded for requests per minute",
    ] {
        assert!(is_rate_limit_error(api, None), "{api}");
        assert_eq!(
            recovery(api, None).kind.as_deref(),
            Some("rate_limit"),
            "{api}"
        );
    }
    assert_eq!(
        recovery("provider overloaded, usage limit unknown", None)
            .kind
            .as_deref(),
        Some("overloaded")
    );
}
