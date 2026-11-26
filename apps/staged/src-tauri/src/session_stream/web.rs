//! A browser client's sink: a bounded queue of serialized frames, which the
//! client's events socket forwards (`handle_ws` in `web_server.rs`).
//!
//! The broadcast channel that socket also carries reaches every client, so
//! it cannot hold frames: they contain message text, and each client asked
//! for different sessions.

use std::sync::Arc;

use tokio::sync::mpsc;

use super::{FrameSink, SessionStreamFrame, SinkError};

/// The name frames go out under in the socket's `{event, payload}` envelope.
pub(crate) const SESSION_STREAM_EVENT: &str = "session-stream";

/// How many frames wait for a slow socket before the hub skips its client.
/// Each is one session's update, so this is a burst of that many sessions.
pub(super) const WEB_QUEUE_FRAMES: usize = 64;

struct WebQueue(mpsc::Sender<String>);

impl FrameSink for WebQueue {
    fn room(&self) -> usize {
        self.0.capacity()
    }

    fn send(&self, frame: SessionStreamFrame) -> Result<(), SinkError> {
        let text =
            crate::web_server::serialize_web_event(SESSION_STREAM_EVENT, &frame).map_err(|e| {
                log::warn!("Session stream frame did not serialize: {e}");
                SinkError::Dropped
            })?;
        self.0.try_send(text).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => SinkError::Dropped,
            mpsc::error::TrySendError::Closed(_) => SinkError::Closed,
        })
    }
}

/// A sink for the hub and the receiving end for the socket.
pub(crate) fn web_queue() -> (Arc<dyn FrameSink>, mpsc::Receiver<String>) {
    let (tx, rx) = mpsc::channel(WEB_QUEUE_FRAMES);
    (Arc::new(WebQueue(tx)), rx)
}
