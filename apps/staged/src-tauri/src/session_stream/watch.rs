//! Watching for another process's writes.
//!
//! The session feed sees the writes of this process's store. A second Staged
//! process on the same database (a dev build beside a release build) writes
//! through its own, and nothing here is marked. So for every session a
//! client tracks, the hub keeps a [`Watch`]: whether this process is the one
//! writing it, and if not, the database's `data_version` when it was last
//! read. While any unpaused client tracks a session that is not this
//! process's, the hub reads `data_version` once per [`FOREIGN_CHECK`]. A
//! value other than the one a session was read at means some other process
//! has committed something since, and the session is read again.
//!
//! **Which sessions are ours.** Only a session the last read found `running`
//! with this process's pid in `owner_pid`. Every move into `running` stamps
//! the pid of the process making it, and while that process lives no other
//! takes the row over, so the transcript of such a session is written
//! through this process's store alone. `owner_pid` says nothing reliable in
//! any other state: a `queued` or `disconnected` row has none and any
//! instance may claim it, and an ended or paused row keeps the pid of
//! whoever ran it last while any instance may resume it. A session with no
//! row, and one not read yet, may be created by anyone. All of those are
//! watched. That is the cautious side: a watched session costs one pragma a
//! second, and an unwatched one that another process writes would show
//! stale state until this process next wrote it.
//!
//! One gap is left at the title and summary levels. Another instance can
//! still write the *session row* of a session running here: its Stop ends a
//! `running` row it has no runner for. That write is not seen until this
//! process next writes the session, when the frame for that write carries
//! the row's current fields. The full level does see it, within a second
//! (`full.rs`): there the version is read for a session running here too,
//! and a change has the session row read again.
//!
//! **What a re-read sends.** `data_version` says that something was written,
//! not what, so the session's newest rows are read as for a snapshot and
//! compared with what the client was last sent, row by row, as fingerprints.
//! Only the rows that differ go out, as a delta. A commit to some other
//! session costs the read and sends nothing. That is for the summary level;
//! at the full level the rows a client holds are too many to compare, and it
//! is asked to read the session again (`full.rs`).
//!
//! **Which reads are tagged.** A read is tagged with the version taken
//! before it only when it covered everything of the session that the level
//! sends. A delta of the rows this process marked did not: tagged, it would
//! claim the session was current as of that version and hide a row another
//! process wrote just before.
//!
//! With one Staged process the value never changes, so the cost is the
//! pragma: about a microsecond a second, and only while such a session is
//! subscribed.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::store::{SessionStatus, SummaryRow};

use super::hub::fingerprint;

/// How often the database's `data_version` is read while a client tracks a
/// session this process is not running.
pub(super) const FOREIGN_CHECK: Duration = Duration::from_secs(1);

/// Whether every write to the session's transcript goes through this
/// process's store. See the module docs for why nothing else counts.
pub(super) fn is_ours(status: &SessionStatus, owner_pid: Option<u32>) -> bool {
    *status == SessionStatus::Running && owner_pid == Some(std::process::id())
}

/// A row as a client holds it, reduced to a number.
pub(super) fn row_print(row: &SummaryRow) -> u64 {
    fingerprint((row.role.as_str(), &row.content, &row.image_ids))
}

/// Who writes one session one client tracks, and what that client holds of
/// it.
#[derive(Default)]
pub(super) struct Watch {
    /// The last read found the session running under this process. `false`
    /// until a read says so.
    ours: bool,
    /// The database's `data_version` just before the session was last read.
    /// `None` when it was not taken for that read.
    read_version: Option<i64>,
    /// Another process has committed since the last read, so the next frame
    /// reads the newest rows whichever they are.
    refresh: bool,
    /// The fingerprints of the newest rows the client was sent, by row id,
    /// trimmed to its window: what it holds, since it trims the same way.
    sent_rows: BTreeMap<i64, u64>,
}

impl Watch {
    pub(super) fn is_ours(&self) -> bool {
        self.ours
    }

    /// The session was read, as far as its level sends it. `version` is the
    /// `data_version` taken before the read, so a commit that lands between
    /// the two is never missed: it makes the next value differ.
    pub(super) fn read(&mut self, ours: bool, version: Option<i64>) {
        self.ours = ours;
        self.read_version = version;
    }

    /// The session row was read, and not everything a client holds of the
    /// session: who writes it is known, and the version it was last read in
    /// full at stands.
    pub(super) fn seen(&mut self, ours: bool) {
        self.ours = ours;
    }

    /// Whether another process may have written the session since it was
    /// read, given the database's `data_version` now.
    pub(super) fn missed(&self, version: i64) -> bool {
        !self.ours && self.read_version != Some(version)
    }

    pub(super) fn ask_refresh(&mut self) {
        self.refresh = true;
    }

    pub(super) fn take_refresh(&mut self) -> bool {
        std::mem::take(&mut self.refresh)
    }

    pub(super) fn cancel_refresh(&mut self) {
        self.refresh = false;
    }

    /// Whether the client holds `row` as it is now.
    pub(super) fn holds(&self, row_id: i64, print: u64) -> bool {
        self.sent_rows.get(&row_id) == Some(&print)
    }

    /// Rows went out in a frame. A snapshot replaces what the client holds.
    pub(super) fn sent(
        &mut self,
        snapshot: bool,
        rows: impl IntoIterator<Item = (i64, u64)>,
        window: usize,
    ) {
        if snapshot {
            self.sent_rows.clear();
        }
        self.sent_rows.extend(rows);
        while self.sent_rows.len() > window {
            self.sent_rows.pop_first();
        }
    }

    /// The client no longer holds any row of the session.
    pub(super) fn forget_rows(&mut self) {
        self.sent_rows.clear();
    }
}
