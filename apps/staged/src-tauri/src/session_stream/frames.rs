//! What goes over the wire: the interest a client declares, and the frames
//! the hub sends back. None of it is stored.

use serde::{Deserialize, Serialize};

use crate::store::SessionSummary;

/// How much of a session a client wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStreamLevel {
    /// Status and title.
    Title,
    /// Enough to derive the one-line subtitle as well: the working
    /// directory, the pipeline, the opening prompt's id and the transcript
    /// rows the subtitle reads.
    Summary,
    /// Everything the chat view shows. Not served yet: an interest that asks
    /// for it is refused.
    Full,
}

/// One session a client wants, and at which level.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInterest {
    pub session_id: String,
    pub level: SessionStreamLevel,
}

/// A client's whole interest. Each call replaces the one before.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStreamInterest {
    /// Chosen by the client and echoed on every frame, so it can discard
    /// frames that answer an interest it has since replaced. A new value
    /// starts the stream over: the sequence restarts at 1 and every session
    /// gets a snapshot.
    pub generation: u64,
    /// The client is not showing anything (a hidden window). Its interest is
    /// kept and nothing is sent. The call that clears it gets snapshots.
    #[serde(default)]
    pub paused: bool,
    /// How many of a session's newest visible rows a summary snapshot
    /// carries. The client owns the number because it owns the rule that
    /// reads them.
    pub summary_window: u32,
    pub sessions: Vec<SessionInterest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStreamFrameKind {
    /// The session's state at the level asked for: the client replaces what
    /// it holds.
    Snapshot,
    /// The current version of what changed: the client replaces the session
    /// fields and upserts the rows by id. Applying one twice is harmless.
    Delta,
    /// Another process has written the database and the hub cannot say
    /// which of the session's rows changed: the client reads the session
    /// again from its cursor. It carries nothing else.
    ///
    /// For the full level, where the rows a client holds are too many to
    /// compare here. That level is not served yet, so nothing sends this
    /// yet; the title and summary levels get a delta of what differs
    /// instead.
    Resync,
}

/// One session's update for one client.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStreamFrame {
    pub generation: u64,
    /// Counts this client's frames within the generation, from 1. A jump
    /// means a frame was lost, and the client asks again with a new
    /// generation.
    pub seq: u64,
    pub session_id: String,
    pub kind: SessionStreamFrameKind,
    /// At the title level only the status and title are set. Left out of a
    /// `Resync`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SessionSummary>,
}
