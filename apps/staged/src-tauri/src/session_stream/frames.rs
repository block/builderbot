//! What goes over the wire: the interest a client declares, and the frames
//! the hub sends back. None of it is stored.

use serde::{Deserialize, Serialize};

use crate::store::{SessionFull, SessionPollRequest, SessionSummary};

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
    /// Everything the chat view shows: what the pane's poll returns, sent
    /// when it changes. A session can be asked for at this level and at one
    /// of the other two together, when it is both an open chat and a row;
    /// the two are then served side by side, each with its own snapshot and
    /// deltas.
    Full,
}

/// One session a client wants, and at which level.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInterest {
    pub session_id: String,
    pub level: SessionStreamLevel,
    /// For the full level: what the client holds of the session, as the
    /// cursors of a poll. Giving one asks for a snapshot read from it,
    /// whether or not the session was already held at this level; that is
    /// how a client answers a `Resync`, and how a second pane joins. Rows
    /// the client was still owed a delta of are read with it.
    ///
    /// Left out, a session already held at the full level carries on with
    /// deltas. A session that is owed a snapshot and comes without one (it
    /// is new, or the stream is starting over) is read from the last cursor
    /// the client gave for it, or from the start of the transcript if it
    /// gave none.
    ///
    /// Not read at the other levels.
    #[serde(default)]
    pub cursor: Option<SessionPollRequest>,
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
    /// The session's state at the level asked for. At the title and summary
    /// levels the client replaces what it holds. At the full level it is
    /// read from the client's cursor, and the client merges it as it would
    /// the poll with that cursor.
    Snapshot,
    /// The current version of what changed: the client replaces the session
    /// fields and upserts the rows by id. Applying one twice is harmless.
    Delta,
    /// Another process has written the database and the hub cannot say
    /// which of the session's rows changed: the client declares the session
    /// again with the cursor of what it holds now, and gets a snapshot read
    /// from it. The frame carries nothing else, and no other is sent until
    /// that cursor arrives.
    ///
    /// For the full level only, where the rows a client holds are too many
    /// to compare here. The title and summary levels get a delta of what
    /// differs instead.
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
    /// A frame is about one level: a snapshot or a delta carries `summary`
    /// or `full`, never both, and `kind` says which of the two it is for
    /// that level.
    pub kind: SessionStreamFrameKind,
    /// The title or summary level. At the title level only the status and
    /// title are set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SessionSummary>,
    /// The full level. A snapshot holds all four parts, read from the
    /// client's cursor. A delta holds the session row and the queue only
    /// when one was written, and the rows that were written, to upsert by
    /// id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<SessionFull>,
}
