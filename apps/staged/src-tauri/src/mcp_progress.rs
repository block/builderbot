//! Progress for long-running MCP calls, including sessionless HTTP requests.

use std::time::Duration;

use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::{Peer, RoleServer};

/// Open the response stream immediately, then report elapsed seconds every 30s.
/// rmcp holds sessionless HTTP headers until the handler's first message, so
/// sleeping before the first notification risks a response-header timeout.
///
/// Run under `select!` against the work and cancellation. This never completes,
/// even without a token: progress may only reference a token supplied by the
/// caller. Notifications keep idle timers alive, not absolute client deadlines.
pub(crate) async fn send_progress_keepalives(
    peer: &Peer<RoleServer>,
    progress_token: Option<ProgressToken>,
    message: &str,
) {
    let Some(progress_token) = progress_token else {
        return std::future::pending().await;
    };
    let started = tokio::time::Instant::now();
    let mut interval = tokio::time::interval_at(started, Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let elapsed_secs = started.elapsed().as_secs();
        let notification =
            ProgressNotificationParam::new(progress_token.clone(), elapsed_secs as f64)
                .with_message(format!("{message} ({elapsed_secs}s elapsed)."));
        if let Err(e) = peer.notify_progress(notification).await {
            log::debug!("[mcp_progress] failed to send progress keep-alive: {e}");
            // The transport is gone. Let the request's work/cancellation settle
            // the call, without retrying a closed notification channel.
            return std::future::pending().await;
        }
    }
}

#[cfg(test)]
mod tests;
