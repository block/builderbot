//! Counts the session stream's frames: the one part of the harness that
//! needs the stream. On a tree without `session_stream`, leave out this
//! file, its `mod` line and the call in `session_stream/hub.rs`; a run on
//! such a tree reports the stream figures as absent.

use super::metrics;
use crate::session_stream::{SessionStreamFrame, SessionStreamFrameKind};

/// A frame's level, kind and size, taken before the hub hands it to a
/// client's sink (which consumes it) and counted once the sink has
/// answered. `None` inside while the harness is not running.
pub(crate) struct Measured(Option<(&'static str, &'static str, u64)>);

/// Serialized a second time into a counter to size it; the title and
/// summary levels share one type and are counted together.
pub(crate) fn measure(frame: &SessionStreamFrame) -> Measured {
    if !metrics::is_active() {
        return Measured(None);
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
    Measured(Some((level, kind, size.0)))
}

impl Measured {
    /// The sink took the frame (`delivered`), on either transport, or it
    /// did not: its queue was full or its client had gone. Only a frame a
    /// sink took is a frame sent; the others are counted apart, so the
    /// backend's total still matches the page's when a queue overflows.
    pub(crate) fn sent(self, delivered: bool) {
        if let Some((level, kind, bytes)) = self.0 {
            metrics::stream_frame(level, kind, bytes, delivered);
        }
    }
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
