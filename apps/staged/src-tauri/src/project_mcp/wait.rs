use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub(super) fn duration(seconds: u64, has_progress_token: bool) -> Duration {
    // Sessionless HTTP holds response headers until the first message. A
    // caller without a progress token gets a bounded polling response.
    Duration::from_secs(if has_progress_token {
        seconds
    } else {
        seconds.min(30)
    })
}

pub(super) async fn wait_for_completion(
    wait: Duration,
    request_ct: &CancellationToken,
    parent_ct: &CancellationToken,
    mut read_payload: impl FnMut() -> Result<Value, String>,
) -> String {
    let started = tokio::time::Instant::now();
    loop {
        match read_payload() {
            Ok(payload) => {
                let state = payload
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                if matches!(state, "completed" | "cancelled" | "failed")
                    || started.elapsed() >= wait
                {
                    return payload.to_string();
                }
            }
            Err(e) => return e,
        }

        // Subtraction also handles u64::MAX without overflowing an Instant,
        // and a short requested wait does not round up to the polling interval.
        tokio::select! {
            _ = tokio::time::sleep(wait.saturating_sub(started.elapsed()).min(Duration::from_secs(2))) => {}
            _ = request_ct.cancelled() => break,
            _ = parent_ct.cancelled() => break,
        }
    }
    match read_payload() {
        Ok(payload) => payload.to_string(),
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test(start_paused = true)]
    async fn waits_respect_requested_duration_and_cap_without_progress() {
        for (seconds, token, expected) in [
            (0, false, 0),
            (1, true, 1),
            (u64::MAX, false, 30),
            (330, true, 330),
        ] {
            let started = tokio::time::Instant::now();
            let result = wait_for_completion(
                duration(seconds, token),
                &CancellationToken::new(),
                &CancellationToken::new(),
                || Ok(json!({"state": "running"})),
            )
            .await;
            assert_eq!(result, r#"{"state":"running"}"#);
            assert_eq!(started.elapsed(), Duration::from_secs(expected));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn terminal_states_and_errors_return_immediately() {
        for payload in [
            Ok(json!({"state":"completed"})),
            Ok(json!({"state":"cancelled"})),
            Ok(json!({"state":"failed"})),
            Err("missing session".into()),
        ] {
            let started = tokio::time::Instant::now();
            let expected = payload.clone().map(|v| v.to_string()).unwrap_or_else(|e| e);
            let result = wait_for_completion(
                duration(u64::MAX, true),
                &CancellationToken::new(),
                &CancellationToken::new(),
                || payload.clone(),
            )
            .await;
            assert_eq!(result, expected);
            assert_eq!(started.elapsed(), Duration::ZERO);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn request_and_parent_cancellation_return_the_latest_payload() {
        for cancel_parent in [false, true] {
            let request = CancellationToken::new();
            let parent = CancellationToken::new();
            let mut reads = 0;
            let result = wait_for_completion(duration(u64::MAX, true), &request, &parent, || {
                reads += 1;
                if cancel_parent {
                    parent.cancel();
                } else {
                    request.cancel();
                }
                Ok(json!({"state":"running", "reads": reads}))
            })
            .await;
            assert_eq!(
                serde_json::from_str::<Value>(&result).unwrap(),
                json!({"state":"running", "reads":2})
            );
        }
    }
}
