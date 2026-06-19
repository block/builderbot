//! Slack integration: Web API client, Socket Mode listener, and the
//! [`ProgressSink`] that edits a thread message as the agent works.
//!
//! Socket Mode is implemented directly over a WebSocket (`tokio-tungstenite`)
//! plus a few Web API calls (`reqwest`) rather than pulling in a full Slack
//! framework — the envelope loop is small and we avoid a large dependency tree.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use reqwest::header::RETRY_AFTER;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::config::Config;
use crate::events::{parse_app_mention, AgentTask};
use crate::runner::{run_task_blocking, ProgressSink};

const WEB_API_BASE: &str = "https://slack.com/api";
const RECONNECT_DELAY: Duration = Duration::from_secs(3);
const WEB_API_MAX_ATTEMPTS: usize = 3;
const DEFAULT_RATE_LIMIT_DELAY: Duration = Duration::from_secs(1);

// =============================================================================
// Web API client
// =============================================================================

/// Thin Slack Web API client scoped to a bot token.
#[derive(Clone)]
pub struct SlackWeb {
    client: reqwest::Client,
    bot_token: String,
}

#[derive(Deserialize)]
struct ApiResult {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

impl SlackWeb {
    pub fn new(bot_token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            bot_token,
        }
    }

    /// Post a message into a thread, returning the new message's `ts`.
    pub async fn post_message(&self, channel: &str, thread_ts: &str, text: &str) -> Result<String> {
        let res: ApiResult = self
            .call(
                "chat.postMessage",
                &json!({ "channel": channel, "thread_ts": thread_ts, "text": text }),
            )
            .await?;
        res.ts
            .ok_or_else(|| anyhow!("chat.postMessage succeeded but returned no ts"))
    }

    /// Edit an existing message in place.
    pub async fn update_message(&self, channel: &str, ts: &str, text: &str) -> Result<()> {
        let _: ApiResult = self
            .call(
                "chat.update",
                &json!({ "channel": channel, "ts": ts, "text": text }),
            )
            .await?;
        Ok(())
    }

    async fn call(&self, method: &str, body: &serde_json::Value) -> Result<ApiResult> {
        for attempt in 1..=WEB_API_MAX_ATTEMPTS {
            let response = self
                .client
                .post(format!("{WEB_API_BASE}/{method}"))
                .bearer_auth(&self.bot_token)
                .json(body)
                .send()
                .await
                .with_context(|| format!("{method} request failed"))?;

            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let delay = response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_retry_after)
                    .unwrap_or(DEFAULT_RATE_LIMIT_DELAY);

                if attempt < WEB_API_MAX_ATTEMPTS {
                    tokio::time::sleep(delay).await;
                    continue;
                }

                return Err(anyhow!("{method} rate limited after {attempt} attempts"));
            }

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow!("{method} failed with HTTP {status}: {body}"));
            }

            let res: ApiResult = response
                .json()
                .await
                .with_context(|| format!("{method} returned an unparseable response"))?;

            if !res.ok {
                return Err(anyhow!(
                    "{method} failed: {}",
                    res.error.as_deref().unwrap_or("unknown error")
                ));
            }
            return Ok(res);
        }

        unreachable!("WEB_API_MAX_ATTEMPTS is non-zero")
    }
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    value
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

// =============================================================================
// Progress sink
// =============================================================================

/// A [`ProgressSink`] that lazily posts a thread message on first update, then
/// edits that same message on every subsequent update.
pub struct SlackSink {
    web: SlackWeb,
    channel: String,
    thread_ts: String,
    message_ts: Mutex<Option<String>>,
}

impl SlackSink {
    pub fn new(web: SlackWeb, channel: String, thread_ts: String) -> Self {
        Self {
            web,
            channel,
            thread_ts,
            message_ts: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ProgressSink for SlackSink {
    async fn update(&self, body: &str) -> Result<()> {
        let mut ts_guard = self.message_ts.lock().await;
        match ts_guard.as_ref() {
            Some(ts) => self.web.update_message(&self.channel, ts, body).await,
            None => {
                let ts = self
                    .web
                    .post_message(&self.channel, &self.thread_ts, body)
                    .await?;
                *ts_guard = Some(ts);
                Ok(())
            }
        }
    }
}

// =============================================================================
// Socket Mode envelope types
// =============================================================================

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
    envelope_id: Option<String>,
    payload: Option<serde_json::Value>,
}

// =============================================================================
// Socket Mode connection loop
// =============================================================================

/// Open a Socket Mode connection and dispatch mentions until disconnected,
/// reconnecting on error. Runs forever.
pub async fn run(config: Config, dry_run: bool) -> Result<()> {
    let web = SlackWeb::new(config.bot_token.clone());
    loop {
        match connect_and_serve(&config, &web, dry_run).await {
            Ok(()) => {
                eprintln!("slack-bridge: server requested reconnect");
            }
            Err(e) => {
                eprintln!("slack-bridge: connection error: {e:#}; retrying in {RECONNECT_DELAY:?}");
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        }
    }
}

/// Ask Slack for a Socket Mode WebSocket URL using the app-level token.
async fn open_connection_url(app_token: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct OpenResult {
        ok: bool,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        url: Option<String>,
    }

    let res: OpenResult = reqwest::Client::new()
        .post(format!("{WEB_API_BASE}/apps.connections.open"))
        .bearer_auth(app_token)
        .send()
        .await
        .context("apps.connections.open request failed")?
        .json()
        .await
        .context("apps.connections.open returned an unparseable response")?;

    if !res.ok {
        return Err(anyhow!(
            "apps.connections.open failed: {}",
            res.error.as_deref().unwrap_or("unknown error")
        ));
    }
    res.url
        .ok_or_else(|| anyhow!("apps.connections.open returned no url"))
}

async fn connect_and_serve(config: &Config, web: &SlackWeb, dry_run: bool) -> Result<()> {
    let url = open_connection_url(&config.app_token).await?;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .context("failed to connect Socket Mode WebSocket")?;
    eprintln!("slack-bridge: connected (dry_run={dry_run})");

    while let Some(message) = ws.next().await {
        match message.context("WebSocket read error")? {
            Message::Text(text) => {
                if handle_envelope(&text, config, web, dry_run, &mut ws).await? {
                    return Ok(());
                }
            }
            Message::Ping(payload) => {
                ws.send(Message::Pong(payload)).await.ok();
            }
            Message::Close(_) => return Ok(()),
            _ => {}
        }
    }
    Ok(())
}

/// Process one envelope. Returns `true` when the server asked us to reconnect.
async fn handle_envelope<S>(
    text: &str,
    config: &Config,
    web: &SlackWeb,
    dry_run: bool,
    ws: &mut S,
) -> Result<bool>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::error::Error + Send + Sync + 'static,
{
    let envelope: Envelope = match serde_json::from_str(text) {
        Ok(e) => e,
        Err(_) => return Ok(false), // ignore non-envelope frames (e.g. "hello")
    };

    // Always acknowledge envelopes with an id, immediately, before any work.
    if let Some(id) = &envelope.envelope_id {
        let ack = json!({ "envelope_id": id }).to_string();
        ws.send(Message::Text(ack.into()))
            .await
            .context("failed to send ack")?;
    }

    match envelope.kind.as_str() {
        "disconnect" => return Ok(true),
        "events_api" => {
            if let Some(task) = envelope
                .payload
                .as_ref()
                .and_then(|p| p.get("event"))
                .and_then(parse_app_mention)
            {
                dispatch_task(task, config, web, dry_run);
            }
        }
        _ => {}
    }
    Ok(false)
}

/// Kick off handling for a parsed task without blocking the WebSocket loop.
fn dispatch_task(task: AgentTask, config: &Config, web: &SlackWeb, dry_run: bool) {
    if dry_run {
        let web = web.clone();
        tokio::spawn(async move {
            if let Err(e) = run_dry_run(web, task).await {
                eprintln!("slack-bridge: dry-run task failed: {e:#}");
            }
        });
        return;
    }

    let agent = config.agent.clone();
    let workdir = config.workdir.clone();
    let bot_token = config.bot_token.clone();
    let AgentTask {
        channel,
        thread_ts,
        prompt,
        ..
    } = task;

    // The ACP driver is `!Send` and needs its own runtime; run it on a blocking
    // thread. The sink (and its HTTP client) is built inside that thread.
    tokio::task::spawn_blocking(move || {
        let cancel = tokio_util::sync::CancellationToken::new();
        let make_sink = {
            let bot_token = bot_token.clone();
            let channel = channel.clone();
            let thread_ts = thread_ts.clone();
            move || -> Result<Arc<dyn ProgressSink>> {
                Ok(Arc::new(SlackSink::new(
                    SlackWeb::new(bot_token),
                    channel,
                    thread_ts,
                )))
            }
        };
        if let Err(e) = run_task_blocking(agent, workdir, prompt, make_sink, cancel) {
            eprintln!("slack-bridge: agent task failed: {e:#}");
        }
    });
}

/// Live dry-run: echo the parsed prompt and a canned progress sequence into the
/// thread, exercising the full Slack round-trip without invoking an agent.
async fn run_dry_run(web: SlackWeb, task: AgentTask) -> Result<()> {
    use crate::progress::ProgressState;

    let sink = SlackSink::new(web, task.channel.clone(), task.thread_ts.clone());

    let mut state = ProgressState::new();
    sink.update(&state.render()).await?;

    state.record_tool_call("(dry-run) would start the agent");
    state.append_text(&format!("Received prompt:\n> {}", task.prompt));
    sink.update(&state.render()).await?;

    state.mark_done();
    sink.update(&state.render()).await?;
    Ok(())
}
