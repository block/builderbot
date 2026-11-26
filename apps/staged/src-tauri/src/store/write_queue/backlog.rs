//! The write queue's backlog, reported in the log.
//!
//! The queue has no limit: a limit would make a session thread wait, which
//! is what the queue exists to prevent. So when the writer thread falls
//! behind (another thread is holding the write connection, or sessions
//! queue faster than it stores), writes pile up and what appears on screen
//! lags, and nothing else says so. [`Backlog`] says so: it is shown the
//! queue each time a write joins it or is taken off, and gives back a line
//! to log when the queue is deep or its oldest write has waited long, at
//! most once per [`REPORT_INTERVAL`], and one more when the queue has
//! emptied again.
//!
//! It is only shown the queue when something moves. A writer thread stuck
//! on one write while nothing new is queued reports nothing until the next
//! write joins.

use std::time::{Duration, Instant};

/// The queue is behind when its oldest write has waited this long.
pub(super) const BACKLOG_AGE: Duration = Duration::from_secs(1);

/// The queue is behind when this many writes are waiting.
pub(super) const BACKLOG_DEPTH: usize = 1000;

/// The shortest time between two reports of a backlog.
pub(super) const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// The queue at one moment.
pub(super) struct QueueSnapshot {
    pub(super) now: Instant,
    /// Writes waiting, not counting one running.
    pub(super) waiting: usize,
    /// How many of those set a transcript row. The rest are writes that
    /// cannot fold.
    pub(super) row_writes: usize,
    /// When the write at the front joined the queue.
    pub(super) oldest_queued_at: Option<Instant>,
    /// When the writer thread took the write it is running, if it is
    /// running one.
    pub(super) running_since: Option<Instant>,
}

/// A line for the log, and the level to log it at.
#[derive(Debug, PartialEq)]
pub(super) struct Report {
    pub(super) level: log::Level,
    pub(super) text: String,
}

impl Report {
    /// Log it. Not to be called with the queue's lock held: a session
    /// thread queueing a write waits on that lock.
    pub(super) fn log(self) {
        log::log!(self.level, "{}", self.text);
    }
}

/// What has been reported about the queue's backlog so far.
#[derive(Default)]
pub(super) struct Backlog {
    /// When the backlog now being reported was first reported. `None`
    /// while the queue keeps up, and again once it has emptied.
    since: Option<Instant>,
    /// When the last report was made, across backlogs, so a queue that
    /// empties and backs up again many times a second still reports once
    /// per interval.
    last_report: Option<Instant>,
    /// Row writes folded into a waiting one since the queue was last empty.
    folded: u64,
    /// The most writes seen waiting during the backlog being reported.
    deepest: usize,
    /// The longest the front write was seen to have waited during it.
    longest_wait: Duration,
}

impl Backlog {
    /// A row write was folded into a waiting one instead of joining the
    /// queue.
    pub(super) fn folded(&mut self) {
        self.folded += 1;
    }

    /// Look at the queue. A report if it is behind and none was made in the
    /// last [`REPORT_INTERVAL`].
    pub(super) fn observe(&mut self, queue: &QueueSnapshot) -> Option<Report> {
        let waited = queue
            .oldest_queued_at
            .map(|at| queue.now.saturating_duration_since(at))
            .unwrap_or_default();
        if self.since.is_some() {
            self.deepest = self.deepest.max(queue.waiting);
            self.longest_wait = self.longest_wait.max(waited);
        }
        if waited < BACKLOG_AGE && queue.waiting < BACKLOG_DEPTH {
            return None;
        }
        if self
            .last_report
            .is_some_and(|at| queue.now.saturating_duration_since(at) < REPORT_INTERVAL)
        {
            return None;
        }
        self.last_report = Some(queue.now);
        if self.since.is_none() {
            self.since = Some(queue.now);
            self.deepest = queue.waiting;
            self.longest_wait = waited;
        }

        let mut text = format!(
            "Store write queue is backed up: {} waiting ({} of them row writes), the oldest \
             for {:.1}s",
            writes(queue.waiting),
            queue.row_writes,
            waited.as_secs_f64(),
        );
        if let Some(since) = queue.running_since {
            text.push_str(&format!(
                "; the writer thread has been on its current write for {:.1}s",
                queue.now.saturating_duration_since(since).as_secs_f64()
            ));
        }
        if self.folded > 0 {
            text.push_str(&format!(
                "; {} folded into waiting ones since the queue was last empty",
                row_writes(self.folded)
            ));
        }
        Some(Report {
            level: log::Level::Warn,
            text,
        })
    }

    /// The queue is empty. A report if a backlog had been reported.
    pub(super) fn drained(&mut self, now: Instant) -> Option<Report> {
        self.folded = 0;
        let since = self.since.take()?;
        let text = format!(
            "Store write queue caught up {:.1}s after it was first reported backed up: at most \
             {} waiting, the oldest for {:.1}s",
            now.saturating_duration_since(since).as_secs_f64(),
            writes(self.deepest),
            self.longest_wait.as_secs_f64(),
        );
        self.deepest = 0;
        self.longest_wait = Duration::ZERO;
        Some(Report {
            level: log::Level::Info,
            text,
        })
    }
}

fn writes(count: usize) -> String {
    match count {
        1 => "1 write".to_string(),
        count => format!("{count} writes"),
    }
}

fn row_writes(count: u64) -> String {
    match count {
        1 => "1 row write".to_string(),
        count => format!("{count} row writes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(now: Instant, waiting: usize, oldest_waited: Duration) -> QueueSnapshot {
        QueueSnapshot {
            now,
            waiting,
            row_writes: 0,
            oldest_queued_at: (waiting > 0).then(|| now - oldest_waited),
            running_since: None,
        }
    }

    fn secs(secs: f64) -> Duration {
        Duration::from_secs_f64(secs)
    }

    /// A queue that keeps up is never reported, and emptying it says
    /// nothing either.
    #[test]
    fn a_queue_that_keeps_up_reports_nothing() {
        let start = Instant::now() + secs(100.0);
        let mut backlog = Backlog::default();
        assert_eq!(backlog.observe(&queue(start, 0, secs(0.0))), None);
        assert_eq!(
            backlog.observe(&queue(start, BACKLOG_DEPTH - 1, secs(0.9))),
            None
        );
        assert_eq!(backlog.drained(start), None);
    }

    /// The report gives the depth, how many are row writes, the oldest
    /// write's wait, how long the writer thread has been on its current
    /// write and how many row writes were folded.
    #[test]
    fn an_old_write_is_reported_with_the_queue_around_it() {
        let now = Instant::now() + secs(100.0);
        let mut backlog = Backlog::default();
        for _ in 0..5321 {
            backlog.folded();
        }
        let report = backlog
            .observe(&QueueSnapshot {
                now,
                waiting: 1532,
                row_writes: 1200,
                oldest_queued_at: Some(now - secs(12.3)),
                running_since: Some(now - secs(12.1)),
            })
            .unwrap();
        assert_eq!(report.level, log::Level::Warn);
        assert_eq!(
            report.text,
            "Store write queue is backed up: 1532 writes waiting (1200 of them row writes), \
             the oldest for 12.3s; the writer thread has been on its current write for 12.1s; \
             5321 row writes folded into waiting ones since the queue was last empty"
        );
    }

    /// Depth alone is enough, with every write young; with the writer
    /// thread between writes and nothing folded the report leaves those
    /// parts out.
    #[test]
    fn a_deep_queue_of_young_writes_is_reported() {
        let now = Instant::now() + secs(100.0);
        let mut backlog = Backlog::default();
        let report = backlog
            .observe(&queue(now, BACKLOG_DEPTH, secs(0.2)))
            .unwrap();
        assert_eq!(
            report.text,
            "Store write queue is backed up: 1000 writes waiting (0 of them row writes), the \
             oldest for 0.2s"
        );
    }

    /// One write waiting a long time reads as one write.
    #[test]
    fn a_single_old_write_is_reported_in_the_singular() {
        let now = Instant::now() + secs(100.0);
        let mut backlog = Backlog::default();
        backlog.folded();
        let report = backlog.observe(&queue(now, 1, secs(1.0))).unwrap();
        assert_eq!(
            report.text,
            "Store write queue is backed up: 1 write waiting (0 of them row writes), the oldest \
             for 1.0s; 1 row write folded into waiting ones since the queue was last empty"
        );
    }

    /// A backlog that lasts is reported once per interval, not on every
    /// write.
    #[test]
    fn a_lasting_backlog_is_reported_once_per_interval() {
        let start = Instant::now() + secs(100.0);
        let mut backlog = Backlog::default();
        assert!(backlog.observe(&queue(start, 10, secs(2.0))).is_some());
        for step in 1..50 {
            let now = start + secs(0.1 * f64::from(step));
            assert_eq!(
                backlog.observe(&queue(now, 10 + step as usize, secs(2.0))),
                None
            );
        }
        let later = start + REPORT_INTERVAL;
        assert!(backlog.observe(&queue(later, 80, secs(7.0))).is_some());
    }

    /// Once the queue empties, one line says how long the backlog lasted
    /// and how bad it got, counting what was seen between reports. Then
    /// nothing more until the queue is behind again.
    #[test]
    fn catching_up_is_reported_once_with_the_worst_seen() {
        let start = Instant::now() + secs(100.0);
        let mut backlog = Backlog::default();
        assert!(backlog.observe(&queue(start, 40, secs(1.5))).is_some());
        // Seen between reports, so not logged itself.
        assert_eq!(
            backlog.observe(&queue(start + secs(2.0), 2345, secs(3.5))),
            None
        );
        assert_eq!(
            backlog.observe(&queue(start + secs(30.0), 12, secs(30.1))),
            Some(Report {
                level: log::Level::Warn,
                text: "Store write queue is backed up: 12 writes waiting (0 of them row \
                       writes), the oldest for 30.1s"
                    .to_string(),
            })
        );
        let report = backlog.drained(start + secs(34.2)).unwrap();
        assert_eq!(report.level, log::Level::Info);
        assert_eq!(
            report.text,
            "Store write queue caught up 34.2s after it was first reported backed up: at most \
             2345 writes waiting, the oldest for 30.1s"
        );
        assert_eq!(backlog.drained(start + secs(35.0)), None);
    }

    /// A queue that empties and backs up again straight away does not get
    /// a second report inside the interval, and the folded count starts
    /// again from the empty queue.
    #[test]
    fn a_queue_that_backs_up_again_waits_out_the_interval() {
        let start = Instant::now() + secs(100.0);
        let mut backlog = Backlog::default();
        backlog.folded();
        assert!(backlog.observe(&queue(start, 3, secs(1.2))).is_some());
        assert!(backlog.drained(start + secs(0.5)).is_some());

        assert_eq!(
            backlog.observe(&queue(start + secs(2.0), 3, secs(1.2))),
            None
        );
        // Not reported, so there is no backlog to report the end of.
        assert_eq!(backlog.drained(start + secs(2.5)), None);

        let report = backlog
            .observe(&queue(start + secs(5.0), 3, secs(1.2)))
            .unwrap();
        assert!(!report.text.contains("folded"), "{}", report.text);
    }
}
