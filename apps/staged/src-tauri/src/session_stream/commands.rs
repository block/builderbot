//! The stream's Tauri commands, for a native window. A browser client sends
//! the same two through `web_server::dispatch`, with its client id as an
//! argument and its events socket as the sink.
//!
//! Both only change or read the hub's in-memory state. The store is read by
//! the hub task, on the blocking pool.

use std::sync::Arc;

use tauri::ipc::{Channel, JavaScriptChannelId};
use tauri::{Runtime, State, WebviewWindow};

use crate::pr_poll_scheduler::TAURI_CLIENT_PREFIX;

use super::{FrameSink, SessionStreamFrame, SessionStreamHub, SessionStreamInterest, SinkError};

/// A native window's client id, from its label and not from an argument, so
/// one window cannot declare or drop another's interest.
fn window_client_id<R: Runtime>(window: &WebviewWindow<R>) -> String {
    format!("{TAURI_CLIENT_PREFIX}{}", window.label())
}

/// A native window's sink. A channel numbers its messages and delivers them
/// in order, and gives no feedback, so it always has room.
struct ChannelSink(Channel<SessionStreamFrame>);

impl FrameSink for ChannelSink {
    fn room(&self) -> usize {
        usize::MAX
    }

    fn send(&self, frame: SessionStreamFrame) -> Result<(), SinkError> {
        // A send fails once the webview behind the channel is gone. If it
        // is not, its next `check_session_stream` finds no stream and the
        // page opens a new one.
        self.0.send(frame).map_err(|e| {
            log::debug!("Session stream channel closed: {e}");
            SinkError::Closed
        })
    }
}

/// Declare what this window wants from the session stream, replacing what it
/// declared before.
///
/// `channel` is where the frames go. The page passes one on its first call,
/// which opens the stream (and drops the interest of any stream the window
/// had, as after a reload), and leaves it out afterwards. A call with no
/// channel and no open stream fails, so the page knows to open one.
///
/// It is taken as the id a `Channel` serializes to and not as a `Channel`
/// argument, which cannot be optional.
#[tauri::command(rename_all = "camelCase")]
pub fn set_session_stream_interest<R: Runtime>(
    window: WebviewWindow<R>,
    hub: State<'_, Arc<SessionStreamHub>>,
    interest: SessionStreamInterest,
    channel: Option<JavaScriptChannelId>,
) -> Result<(), String> {
    let client_id = window_client_id(&window);
    if let Some(channel) = channel {
        let channel = channel.channel_on(window.as_ref().clone());
        hub.connect(&client_id, Arc::new(ChannelSink(channel)));
    }
    hub.set_interest(&client_id, interest)
}

/// Whether the hub's last frame for this window in `generation` is number
/// `last_seq`, the last one the page received. `false` means the page should
/// declare its interest again under a new generation: a frame was lost, or
/// the hub no longer has the stream. A frame still on its way also reads as
/// `false`; asking again then costs one round of snapshots.
#[tauri::command(rename_all = "camelCase")]
pub fn check_session_stream<R: Runtime>(
    window: WebviewWindow<R>,
    hub: State<'_, Arc<SessionStreamHub>>,
    generation: u64,
    last_seq: u64,
) -> bool {
    hub.check(&window_client_id(&window), generation, last_seq)
}
