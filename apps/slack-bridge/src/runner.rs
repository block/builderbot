//! Agent execution.
//!
//! Bridges `acp-client`'s streaming [`MessageWriter`] onto a [`ProgressSink`]
//! (e.g. a Slack thread message). The ACP driver produces text chunks and
//! tool-call events; we fold them into a [`ProgressState`] and push throttled
//! snapshots to the sink so the thread updates live without hitting Slack's
//! rate limits on every token.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use acp_client::{AcpDriver, AgentDriver, MessageWriter, Store};

use crate::progress::ProgressState;

/// Minimum gap between sink updates while streaming.
const DEFAULT_THROTTLE: Duration = Duration::from_millis(800);

/// Receives rendered progress snapshots. Implemented by the Slack client (to
/// edit a thread message) and by test/dev sinks.
///
/// `Send + Sync` because the ACP [`MessageWriter`] that drives it requires it;
/// implementors (e.g. a `reqwest`-backed Slack client) are already thread-safe.
#[async_trait]
pub trait ProgressSink: Send + Sync {
    /// Replace the current progress message with `body`.
    async fn update(&self, body: &str) -> Result<()>;
}

/// No-op store: the bridge runs one-shot tasks and does not persist sessions.
struct NoOpStore;

#[async_trait]
impl Store for NoOpStore {
    fn set_agent_session_id(
        &self,
        _session_id: &str,
        _agent_session_id: &str,
    ) -> Result<(), String> {
        Ok(())
    }
}

struct WriterInner {
    state: ProgressState,
    last_flush: Option<Instant>,
}

/// A [`MessageWriter`] that folds ACP events into a [`ProgressState`] and
/// forwards throttled renders to a [`ProgressSink`].
struct StreamingWriter {
    inner: Arc<Mutex<WriterInner>>,
    sink: Arc<dyn ProgressSink>,
    throttle: Duration,
}

impl StreamingWriter {
    async fn flush(&self, force: bool) {
        let body = {
            let mut guard = self.inner.lock().await;
            let now = Instant::now();
            let due = force
                || guard
                    .last_flush
                    .map(|t| now.duration_since(t) >= self.throttle)
                    .unwrap_or(true);
            if !due {
                return;
            }
            guard.last_flush = Some(now);
            guard.state.render()
        };
        // Sink errors are non-fatal for streaming updates; the final flush in
        // `run_task` surfaces the terminal state and any hard failure there.
        if let Err(e) = self.sink.update(&body).await {
            log_sink_error(&e);
        }
    }
}

#[async_trait]
impl MessageWriter for StreamingWriter {
    async fn append_text(&self, text: &str) {
        self.inner.lock().await.state.append_text(text);
        self.flush(false).await;
    }

    async fn finalize(&self) {
        self.flush(true).await;
    }

    async fn record_tool_call(
        &self,
        _tool_call_id: &str,
        title: &str,
        _raw_input: Option<&serde_json::Value>,
    ) {
        self.inner.lock().await.state.record_tool_call(title);
        self.flush(true).await;
    }

    async fn update_tool_call_title(
        &self,
        _tool_call_id: &str,
        _title: Option<&str>,
        _raw_input: Option<&serde_json::Value>,
    ) {
        // Title refinements are cosmetic; the original call line is enough.
    }

    async fn record_tool_result(&self, content: &str) {
        self.inner.lock().await.state.record_tool_result(content);
        self.flush(false).await;
    }
}

fn log_sink_error(err: &anyhow::Error) {
    eprintln!("slack-bridge: progress update failed: {err:#}");
}

/// Run one agent task to completion, streaming progress to `sink`.
///
/// `agent_id` is an ACP provider id (e.g. `codex`, `claude`). This future is
/// `!Send` because the ACP driver uses a `LocalSet`; run it via
/// [`run_task_blocking`] from a multi-threaded context.
pub async fn run_task(
    agent_id: &str,
    working_dir: &Path,
    prompt: &str,
    sink: Arc<dyn ProgressSink>,
    cancel: CancellationToken,
) -> Result<()> {
    let inner = Arc::new(Mutex::new(WriterInner {
        state: ProgressState::new(),
        last_flush: None,
    }));

    // Initial placeholder so the thread shows activity immediately.
    let initial = inner.lock().await.state.render();
    sink.update(&initial).await?;

    let driver = AcpDriver::new(agent_id).map_err(|e| anyhow!(e))?;
    let writer: Arc<dyn MessageWriter> = Arc::new(StreamingWriter {
        inner: Arc::clone(&inner),
        sink: Arc::clone(&sink),
        throttle: DEFAULT_THROTTLE,
    });
    let store: Arc<dyn Store> = Arc::new(NoOpStore);

    let result = driver
        .run(
            "slack-task",
            prompt,
            &[],
            working_dir,
            &store,
            &writer,
            &cancel,
            None,
        )
        .await;

    let body = {
        let mut guard = inner.lock().await;
        match &result {
            Ok(()) => guard.state.mark_done(),
            Err(e) => guard.state.mark_failed(e),
        }
        guard.state.render()
    };
    sink.update(&body).await?;

    result.map_err(|e| anyhow!(e))
}

/// Run [`run_task`] on a dedicated current-thread runtime + `LocalSet`.
///
/// The ACP driver's futures are `!Send`, so they need a `LocalSet`. This helper
/// lets a multi-threaded caller drive a task by handing it an owned working dir,
/// prompt, and a factory that builds the sink *inside* the worker thread (so any
/// runtime-bound resources, like an HTTP client, are created on that runtime).
pub fn run_task_blocking<F>(
    agent_id: String,
    working_dir: PathBuf,
    prompt: String,
    make_sink: F,
    cancel: CancellationToken,
) -> Result<()>
where
    F: FnOnce() -> Result<Arc<dyn ProgressSink>> + Send + 'static,
{
    std::thread::scope(|scope| {
        scope
            .spawn(move || -> Result<()> {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    let sink = make_sink()?;
                    run_task(&agent_id, &working_dir, &prompt, sink, cancel).await
                })
            })
            .join()
            .map_err(|_| anyhow!("agent worker thread panicked"))?
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sink that records every update body it receives.
    #[derive(Default)]
    struct RecordingSink {
        updates: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ProgressSink for RecordingSink {
        async fn update(&self, body: &str) -> Result<()> {
            self.updates.lock().await.push(body.to_string());
            Ok(())
        }
    }

    fn writer_with(sink: Arc<RecordingSink>, throttle: Duration) -> StreamingWriter {
        StreamingWriter {
            inner: Arc::new(Mutex::new(WriterInner {
                state: ProgressState::new(),
                last_flush: None,
            })),
            sink,
            throttle,
        }
    }

    #[tokio::test]
    async fn forwards_tool_calls_immediately() {
        let sink = Arc::new(RecordingSink::default());
        let writer = writer_with(Arc::clone(&sink), Duration::from_secs(60));

        writer.record_tool_call("tc1", "cargo test", None).await;

        let updates = sink.updates.lock().await;
        assert_eq!(updates.len(), 1);
        assert!(updates[0].contains("🔧 cargo test"));
    }

    #[tokio::test]
    async fn throttles_text_chunks_then_forces_on_finalize() {
        let sink = Arc::new(RecordingSink::default());
        // Large throttle so streamed chunks coalesce.
        let writer = writer_with(Arc::clone(&sink), Duration::from_secs(60));

        // First append flushes (no prior flush), later ones are throttled.
        writer.append_text("one ").await;
        writer.append_text("two ").await;
        writer.append_text("three").await;

        let mid = sink.updates.lock().await.len();
        assert_eq!(mid, 1, "only the first append should flush under throttle");

        writer.finalize().await;
        let updates = sink.updates.lock().await;
        assert_eq!(updates.len(), 2, "finalize forces a flush");
        assert!(updates.last().unwrap().contains("one two three"));
    }
}
