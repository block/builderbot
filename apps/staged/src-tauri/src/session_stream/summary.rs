//! The title and summary levels: what one client is owed of one session it
//! shows as a row, and the frame that pays it.
//!
//! The hub (`hub.rs`) decides when a frame is due and does the reading and
//! the sending. This module holds one entry's state between those steps and
//! the rules for what goes in its frame.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use tokio::time::Instant;

use crate::store::{SessionDirty, SessionSummary, Store, SummaryRowsQuery};

use super::frames::{SessionStreamFrameKind, SessionStreamLevel};
use super::hub::{fingerprint, GATHER, READ_RETRY, SUMMARY_INTERVAL};
use super::watch::{self, Watch};

/// One session one client wants at the title or summary level, and what it
/// is still owed.
pub(super) struct Tracked {
    /// `Title` or `Summary`.
    level: SessionStreamLevel,
    /// Changes whenever an interest call makes or alters this entry, so a
    /// frame planned for the entry as it was is not sent for what it became.
    revision: u64,
    /// The next frame is a snapshot: none has been sent yet, or the one that
    /// was due found no row to read.
    needs_snapshot: bool,
    /// From when what is owed may be sent, before the cadence is applied.
    /// `None` when nothing is owed.
    ready_at: Option<Instant>,
    /// Rows written since the last frame. Not kept while a snapshot is owed:
    /// the snapshot reads the newest rows whichever they are.
    rows: BTreeSet<i64>,
    last_sent: Option<Instant>,
    /// The session fields of the last frame, as a fingerprint. A delta that
    /// would repeat them and carry no row is not sent: most writes to a
    /// session row touch fields this level does not carry.
    sent_fields: Option<u64>,
    /// The opening prompt's id once a read has found it, so later reads skip
    /// the lookup.
    opening_prompt_id: Option<i64>,
    /// Whether another process may write the session, and the rows the
    /// client holds, for finding what such a write changed.
    watch: Watch,
}

impl Tracked {
    pub(super) fn new(level: SessionStreamLevel, revision: u64, ready_at: Option<Instant>) -> Self {
        Self {
            level,
            revision,
            needs_snapshot: true,
            ready_at,
            rows: BTreeSet::new(),
            last_sent: None,
            sent_fields: None,
            opening_prompt_id: None,
            watch: Watch::default(),
        }
    }

    pub(super) fn level(&self) -> SessionStreamLevel {
        self.level
    }

    /// The entry at a lower level: the rows are no longer wanted, and
    /// whatever else is owed still is.
    pub(super) fn lowered(mut self, level: SessionStreamLevel, revision: u64) -> Self {
        self.watch.forget_rows();
        Self {
            level,
            revision,
            rows: BTreeSet::new(),
            sent_fields: None,
            ..self
        }
    }

    /// Take a drained mark. Queue and metadata marks belong to the full
    /// level, and row marks do not reach the title level.
    pub(super) fn mark(&mut self, dirty: &SessionDirty, now: Instant) {
        let rows_matter = self.level == SessionStreamLevel::Summary && !dirty.rows.is_empty();
        if !dirty.session && !rows_matter {
            return;
        }
        if self.needs_snapshot {
            // A snapshot that found no row waits for a write like this one.
            self.ready_at.get_or_insert(now);
            return;
        }
        if rows_matter {
            self.rows.extend(&dirty.rows);
        }
        self.ready_at.get_or_insert(now + GATHER);
    }

    /// Whether this process is the one writing the session, as far as the
    /// last read could tell.
    pub(super) fn is_ours(&self) -> bool {
        self.watch.is_ours()
    }

    /// Another process may have written the session since it was read,
    /// given the database's `data_version` now: read it again. The version
    /// cannot say which rows, so the read is of the newest ones, as for a
    /// snapshot.
    pub(super) fn check_version(&mut self, version: i64, now: Instant) {
        if !self.watch.missed(version) {
            return;
        }
        if !self.needs_snapshot {
            self.watch.ask_refresh();
        }
        self.ready_at.get_or_insert(now);
    }

    /// The client is paused: it is owed nothing until it asks again.
    pub(super) fn pause(&mut self) {
        self.ready_at = None;
        self.rows.clear();
        self.watch.cancel_refresh();
    }

    pub(super) fn due_at(&self) -> Option<Instant> {
        let ready = self.ready_at?;
        Some(match self.last_sent {
            Some(sent) if !self.needs_snapshot => ready.max(sent + SUMMARY_INTERVAL),
            _ => ready,
        })
    }

    /// Take what is owed out for a frame.
    pub(super) fn take(&mut self, client_id: &str, session_id: &str, window: u32) -> Delivery {
        self.ready_at = None;
        let refresh = self.watch.take_refresh();
        Delivery {
            client_id: client_id.to_owned(),
            session_id: session_id.to_owned(),
            revision: self.revision,
            level: self.level,
            snapshot: self.needs_snapshot,
            refresh: refresh && !self.needs_snapshot,
            ours: self.watch.is_ours(),
            window,
            rows: std::mem::take(&mut self.rows),
            opening_prompt_id: self.opening_prompt_id,
        }
    }

    /// Build the frame for `delivery` from what was read, or `None` when
    /// there is nothing to send.
    ///
    /// `version` is the database's `data_version` from before the read, when
    /// the round took it.
    pub(super) fn deliver(
        &mut self,
        delivery: &Delivery,
        loaded: Option<&Loaded>,
        version: Option<i64>,
        now: Instant,
    ) -> Option<(SessionStreamFrameKind, SessionSummary)> {
        if self.revision != delivery.revision {
            // The interest changed this entry while the read ran. What was
            // taken out for the frame is covered by a snapshot of the entry
            // as it is now.
            self.needs_snapshot = true;
            self.ready_at.get_or_insert(now);
            return None;
        }
        let Some(Loaded::Summary(read)) = loaded else {
            // No row: a deleted session, or one not written yet. Nothing is
            // sent and nothing is retried; if the row is ever written, that
            // write brings a snapshot, whichever process makes it. A read
            // that failed is tried again.
            self.needs_snapshot = true;
            self.ready_at = match loaded {
                Some(Loaded::Failed) => Some(now + READ_RETRY),
                _ => None,
            };
            if let Some(Loaded::Gone) = loaded {
                self.watch.read(false, version);
            }
            return None;
        };
        let LoadedSummary {
            summary,
            title_fields,
            summary_fields,
            row_prints,
            ours,
        } = &**read;
        let title_only = delivery.level == SessionStreamLevel::Title;
        // The read is tagged with the version only if it covered everything
        // of the session this level sends: the fields alone at the title
        // level, the newest rows at the summary level. A delta of the rows
        // this process marked did not, and tagging it would hide a row
        // another process wrote just before.
        if title_only || delivery.snapshot || delivery.refresh {
            self.watch.read(*ours, version);
        } else {
            self.watch.seen(*ours);
        }
        let opening_prompt_id = summary.opening_prompt_id.or(self.opening_prompt_id);
        self.opening_prompt_id = opening_prompt_id;
        let fields = if title_only {
            *title_fields
        } else {
            *summary_fields
        };
        // A read of the newest rows holds at least this client's window of
        // them, and anything else it holds is older.
        let window = delivery.window as usize;
        let newest = summary.rows.len().saturating_sub(window);
        let picked: Vec<usize> = if title_only {
            Vec::new()
        } else if delivery.snapshot {
            (newest..summary.rows.len()).collect()
        } else {
            // The rows this process marked, and after another process's
            // write the newest rows the client does not hold as they are
            // now.
            (0..summary.rows.len())
                .filter(|&index| {
                    let id = summary.rows[index].id;
                    delivery.rows.contains(&id)
                        || (delivery.refresh
                            && index >= newest
                            && !self.watch.holds(id, row_prints[index]))
                })
                .collect()
        };
        if !delivery.snapshot && picked.is_empty() && self.sent_fields == Some(fields) {
            return None;
        }
        if !title_only {
            self.watch.sent(
                delivery.snapshot,
                picked
                    .iter()
                    .map(|&index| (summary.rows[index].id, row_prints[index])),
                window,
            );
        }
        let rows = picked
            .into_iter()
            .map(|index| summary.rows[index].clone())
            .collect();
        let frame_summary = SessionSummary {
            status: summary.status.clone(),
            acp_title: summary.acp_title.clone(),
            owner_pid: summary.owner_pid,
            working_dir: summary.working_dir.clone().filter(|_| !title_only),
            pipeline: summary.pipeline.clone().filter(|_| !title_only),
            opening_prompt_id: opening_prompt_id.filter(|_| !title_only),
            rows,
        };
        self.needs_snapshot = false;
        self.last_sent = Some(now);
        self.sent_fields = Some(fields);
        let kind = if delivery.snapshot {
            SessionStreamFrameKind::Snapshot
        } else {
            SessionStreamFrameKind::Delta
        };
        Some((kind, frame_summary))
    }
}

/// A frame the task has decided to build, with what it took out of the
/// client's state to do so.
pub(super) struct Delivery {
    pub(super) client_id: String,
    pub(super) session_id: String,
    revision: u64,
    level: SessionStreamLevel,
    snapshot: bool,
    /// Not a snapshot, and still a read of the newest rows: another process
    /// wrote the database, and the rows that differ from what the client
    /// holds are sent.
    refresh: bool,
    /// Whether the session was this process's when the frame was planned.
    pub(super) ours: bool,
    window: u32,
    rows: BTreeSet<i64>,
    opening_prompt_id: Option<i64>,
}

/// One session's read for a round of deliveries, shared by every client that
/// is owed a frame for it.
pub(super) struct SummaryRead {
    session_id: String,
    /// `None` when no delivery reads the transcript.
    rows: Option<SummaryRowsQuery>,
}

pub(super) enum Loaded {
    Summary(Box<LoadedSummary>),
    /// The session row does not exist.
    Gone,
    Failed,
}

/// A session that was read, with what the hub works out from it once for
/// every client.
pub(super) struct LoadedSummary {
    summary: SessionSummary,
    title_fields: u64,
    summary_fields: u64,
    /// The fingerprint of each of `summary.rows`, in their order.
    row_prints: Vec<u64>,
    /// Whether this process is the one writing the session.
    ours: bool,
}

impl Loaded {
    fn summary(summary: SessionSummary) -> Self {
        let status = summary.status.as_str();
        let pipeline = summary
            .pipeline
            .as_ref()
            .and_then(|pipeline| serde_json::to_string(pipeline).ok());
        Self::Summary(Box::new(LoadedSummary {
            title_fields: fingerprint((status, &summary.acp_title)),
            summary_fields: fingerprint((
                status,
                &summary.acp_title,
                &summary.working_dir,
                &pipeline,
            )),
            row_prints: summary.rows.iter().map(watch::row_print).collect(),
            ours: watch::is_ours(&summary.status, summary.owner_pid),
            summary,
        }))
    }
}

/// One read per session for a round of deliveries: the widest snapshot
/// window any client is owed, and every row any client is owed a delta of.
pub(super) fn reads_for(deliveries: &[Delivery]) -> Vec<SummaryRead> {
    let mut reads: BTreeMap<&str, Option<SummaryRowsQuery>> = BTreeMap::new();
    for delivery in deliveries {
        let rows = reads.entry(delivery.session_id.as_str()).or_default();
        let newest = delivery.snapshot || delivery.refresh;
        if delivery.level != SessionStreamLevel::Summary || (!newest && delivery.rows.is_empty()) {
            continue;
        }
        let query = rows.get_or_insert_with(SummaryRowsQuery::default);
        if newest {
            query.newest = query.newest.max(delivery.window);
        }
        if !delivery.snapshot {
            query.ids.extend(&delivery.rows);
        }
        query.opening_prompt_id = query.opening_prompt_id.or(delivery.opening_prompt_id);
    }
    reads
        .into_iter()
        .map(|(session_id, rows)| SummaryRead {
            session_id: session_id.to_owned(),
            rows,
        })
        .collect()
}

/// Do the reads, each under its own hold of the store's connection, so a
/// writer waits for one session's read at most. Runs on the blocking pool.
pub(super) fn load(store: &Store, reads: Vec<SummaryRead>) -> HashMap<String, Loaded> {
    reads
        .into_iter()
        .map(|read| {
            let loaded = match store.read_session_summary(&read.session_id, read.rows.as_ref()) {
                Ok(Some(summary)) => Loaded::summary(summary),
                Ok(None) => Loaded::Gone,
                Err(e) => {
                    log::warn!(
                        "Session stream could not read session {}: {e}",
                        read.session_id
                    );
                    Loaded::Failed
                }
            };
            (read.session_id, loaded)
        })
        .collect()
}
