//! Counts the session stream's frames: the one part of the harness that
//! needs the stream. On a tree without `session_stream`, leave out this
//! file, its `mod` line and the call in `session_stream/hub.rs`; a run on
//! such a tree reports the stream figures as absent.

use super::metrics;
use crate::session_stream::{SessionStreamFrame, SessionStreamFrameKind};

/// A frame the hub is about to hand a client's sink, to either transport.
/// Serialized a second time into a counter to size it; the title and
/// summary levels share one type and are counted together.
pub(crate) fn sent(frame: &SessionStreamFrame) {
    if !metrics::is_active() {
        return;
    }
    let level = match (&frame.full, &frame.summary) {
        (Some(_), _) => "full",
        (None, Some(_)) => "summary",
        (None, None) => "none",
    };
    let kind = match frame.kind {
        SessionStreamFrameKind::Snapshot => "snapshot",
        SessionStreamFrameKind::Delta => "delta",
        SessionStreamFrameKind::Resync => "resync",
    };
    let mut size = ByteCount(0);
    let _ = serde_json::to_writer(&mut size, frame);
    metrics::stream_frame(level, kind, size.0);
}

struct ByteCount(u64);

impl std::io::Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
