//! The store's connections: one for writes, a few read-only ones for reads.
//!
//! A SQLite connection serves one thread at a time, so with one connection
//! every store call queued for the same lock: a UI listing projects waited
//! behind a session's streaming write, and the session's next write waited
//! behind the listing. In WAL mode a reader on its own connection reads the
//! last committed state while a writer works, so reads here go to a small
//! pool of read-only connections ([`ReadPool`]) and never queue behind the
//! write connection, nor it behind them.
//!
//! - **What reads where.** A store method that only reads takes a reader
//!   ([`super::Store::read_conn`]). Anything that writes, and any read that
//!   decides a write (a read-modify-write, a read inside a write's
//!   transaction), stays on the write connection.
//! - **Read after write.** A write on the write connection is committed by
//!   the time its store method returns, and a reader's next statement starts
//!   a fresh snapshot, so a read after a write sees it. Writes still on the
//!   write queue (see [`super::write_queue`]) are not committed yet; they
//!   were not visible on the write connection either, and callers that need
//!   them flush first, as before.
//! - **One snapshot per read.** With one connection, nothing in the app
//!   could commit between two statements of the same store method. A reader
//!   is not excluded that way, so [`ReadGuard`] wraps each read in a deferred
//!   transaction: every statement in it sees the same committed state, and
//!   the snapshot ends when the guard drops.
//! - **No WAL, no pool.** If the database is not in WAL mode (an in-memory
//!   database, or a file system that refuses it), a reader would block the
//!   writer, so the pool is left empty and reads share the write connection
//!   as they always did. The same if a reader cannot be opened.
//! - **Timing.** Each connection is behind its own [`TimedMutex`], named, so
//!   a slow wait or hold reports which connection it was.
//! - **All readers busy.** A read then waits for whichever reader is
//!   released first ([`ReadPool::checkout`]), not for one picked in advance.
//!   If that wait is slow, the report reads as a wait for any reader and
//!   names the store methods that held the readers when it began.

use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};

use super::timed_lock::{PoolWait, TimedGuard, TimedMutex};
use super::StoreError;

/// Read-only connections per store. Readers are the main thread's
/// synchronous commands, the async runtime's commands and pollers, and MCP
/// read tools on the blocking pool; four covers them without each holding a
/// page cache for nothing.
pub(super) const READERS: usize = 4;

/// How long a statement retries a locked database before failing. SQLite
/// only returns busy across connections in WAL mode at a checkpoint or WAL
/// reset, or when another process writes; rusqlite's default is also 5s.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The WAL file is truncated back to this size when a checkpoint lets it
/// restart. Without a limit it stays at its high-water mark (117 MB on one
/// machine) for as long as the app runs.
const JOURNAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;

/// Pages of WAL that trigger the automatic (passive) checkpoint at commit.
/// SQLite's default, stated so it is not lost. A passive checkpoint never
/// waits for readers; it copies what no reader still needs and leaves the
/// rest for the next one, which is fine while read transactions stay short.
const WAL_AUTOCHECKPOINT_PAGES: i64 = 1000;

/// Settings for the write connection, before the schema is touched. Whether
/// the database ended up in WAL mode, which readers need.
///
/// `synchronous=NORMAL` is the setting SQLite recommends for WAL: a commit
/// no longer waits on an fsync (the WAL is synced at checkpoints), which
/// shortens every write's hold on the connection under heavy I/O. It cannot
/// corrupt the database; a power loss (not an app crash) can lose the last
/// commits before a checkpoint.
pub(super) fn configure_writer(conn: &Connection) -> Result<bool, StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    conn.execute_batch(&format!(
        "PRAGMA synchronous=NORMAL;
         PRAGMA journal_size_limit={JOURNAL_SIZE_LIMIT_BYTES};
         PRAGMA wal_autocheckpoint={WAL_AUTOCHECKPOINT_PAGES};"
    ))?;
    Ok(mode.eq_ignore_ascii_case("wal"))
}

/// A read-only connection to the database at `path`.
fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // Read-only already; this makes a write that slipped onto a reader fail
    // with a clear error even if the open flags change.
    conn.execute_batch("PRAGMA query_only=ON;")?;
    Ok(conn)
}

/// The store's read-only connections. Empty means reads use the write
/// connection.
pub(super) struct ReadPool {
    readers: Vec<TimedMutex<Connection>>,
    /// Where the next checkout starts looking, so readers share the load.
    next: AtomicUsize,
    /// How many checkouts are waiting for a reader to be released. Held
    /// while a waiter looks for a free reader and while a release wakes
    /// one, so a release between the look and the wait is not missed.
    waiting: Mutex<usize>,
    /// Signalled, under `waiting`, each time a reader is released while a
    /// checkout waits.
    released: Condvar,
}

/// A reader checked out of a [`ReadPool`]. Dropping it releases the
/// connection and wakes a checkout waiting for one.
pub(crate) struct PooledReader<'a> {
    /// `Some` until dropped; taken so the connection is free before a
    /// waiter is woken.
    conn: Option<TimedGuard<'a, Connection>>,
    pool: &'a ReadPool,
}

impl PooledReader<'_> {
    /// The slow-lock report this checkout would log if released now, were
    /// a wait or hold of `threshold` slow.
    #[cfg(test)]
    fn report_if_slower_than(&self, threshold: Duration) -> Option<String> {
        self.conn
            .as_ref()
            .expect("the reader is held until drop")
            .report_if_slower_than(threshold)
    }
}

impl Deref for PooledReader<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("the reader is held until drop")
    }
}

impl Drop for PooledReader<'_> {
    fn drop(&mut self) {
        drop(self.conn.take());
        let waiting = self
            .pool
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *waiting > 0 {
            self.pool.released.notify_one();
        }
    }
}

impl ReadPool {
    /// No readers: every read takes the write connection.
    pub(super) fn empty() -> Self {
        Self {
            readers: Vec::new(),
            next: AtomicUsize::new(0),
            waiting: Mutex::new(0),
            released: Condvar::new(),
        }
    }

    /// `count` readers on the database at `path`, which must already be in
    /// WAL mode with its schema in place. If one cannot be opened the pool is
    /// left empty and the error logged: reads then share the write
    /// connection, slower under contention but correct.
    pub(super) fn open(path: &Path, count: usize) -> Self {
        let mut readers = Vec::with_capacity(count);
        for index in 0..count {
            match open_reader(path) {
                Ok(conn) => {
                    readers.push(TimedMutex::new(conn, format!("store reader {}", index + 1)))
                }
                Err(e) => {
                    log::error!(
                        "Couldn't open a read-only store connection; reads share the write \
                         connection: {e}"
                    );
                    return Self::empty();
                }
            }
        }
        Self {
            readers,
            ..Self::empty()
        }
    }

    /// How many readers there are.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.readers.len()
    }

    /// A free reader if there is one, otherwise whichever is released
    /// first. `None` when the pool is empty.
    ///
    /// A poisoned reader is handed out like any other: it is still a good
    /// connection, a panic mid-read changed nothing.
    #[track_caller]
    pub(super) fn checkout(&self) -> Option<PooledReader<'_>> {
        if self.readers.is_empty() {
            return None;
        }
        let began = Instant::now();
        if let Some(conn) = self.free_reader(began, None) {
            return Some(self.pooled(conn));
        }
        // All busy: note who holds them, for the report if the wait is slow.
        let wait = PoolWait::begin("any store reader", &self.readers);
        // Every release takes `waiting` before it decides whether to
        // signal, so one that happens after the look below finds this
        // checkout counted and wakes it.
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        *waiting += 1;
        let conn = loop {
            if let Some(conn) = self.free_reader(began, Some(&wait)) {
                break conn;
            }
            waiting = self
                .released
                .wait(waiting)
                .unwrap_or_else(PoisonError::into_inner);
        };
        *waiting -= 1;
        // The reader a release signalled for may have gone to a checkout
        // that was not waiting, and this one then took another's: pass the
        // signal on rather than leave a waiter asleep beside a free reader.
        if *waiting > 0 {
            self.released.notify_one();
        }
        drop(waiting);
        Some(self.pooled(conn))
    }

    /// The first reader that is free right now, looking from the next in
    /// turn. `began` is when the checkout started, for the lock's timing,
    /// and `wait` the wait it has been in since it found them all busy.
    #[track_caller]
    fn free_reader(
        &self,
        began: Instant,
        wait: Option<&PoolWait>,
    ) -> Option<TimedGuard<'_, Connection>> {
        let count = self.readers.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed) % count;
        for offset in 0..count {
            let reader = &self.readers[(start + offset) % count];
            if let Some(conn) = reader.try_lock_waiting_since(began, wait) {
                return Some(conn);
            }
        }
        None
    }

    fn pooled<'a>(&'a self, conn: TimedGuard<'a, Connection>) -> PooledReader<'a> {
        PooledReader {
            conn: Some(conn),
            pool: self,
        }
    }
}

/// What a store read runs on: a pooled reader, or the write connection when
/// the store has no readers.
pub(super) enum ReadConn<'a> {
    Reader(PooledReader<'a>),
    Writer(TimedGuard<'a, Connection>),
}

impl Deref for ReadConn<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        match self {
            Self::Reader(conn) => conn,
            Self::Writer(conn) => conn,
        }
    }
}

/// A connection checked out for one store read, inside a read transaction
/// that ends when the guard drops (see the module docs).
pub(crate) struct ReadGuard<'a> {
    conn: ReadConn<'a>,
    /// Whether `BEGIN` ran, so drop has a transaction to end.
    snapshot: bool,
}

impl<'a> ReadGuard<'a> {
    /// Start a read on `conn`. If `BEGIN` fails (it takes no lock, so it
    /// should not), the read goes ahead without one snapshot, logged.
    pub(super) fn begin(conn: ReadConn<'a>) -> Self {
        let snapshot = match conn.execute_batch("BEGIN DEFERRED") {
            Ok(()) => true,
            Err(e) => {
                log::warn!("Couldn't start a store read transaction; reading without one: {e}");
                false
            }
        };
        Self { conn, snapshot }
    }
}

impl Deref for ReadGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl Drop for ReadGuard<'_> {
    fn drop(&mut self) {
        if !self.snapshot || self.conn.is_autocommit() {
            return;
        }
        if let Err(e) = self.conn.execute_batch("COMMIT") {
            log::warn!("Couldn't end a store read transaction; rolling it back: {e}");
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Project, Store};
    use std::sync::mpsc;
    use std::sync::Arc;

    /// Long enough for any machine to run one query, short enough that a
    /// regression fails the test instead of hanging it.
    const PATIENCE: Duration = Duration::from_secs(20);

    /// Run `f` on another thread; what it returns, or `None` if it was still
    /// waiting after [`PATIENCE`].
    fn within_patience<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(PATIENCE).ok()
    }

    fn project_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn a_read_completes_while_the_write_connection_is_held() {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = Project::new("owner/repo");
        store.create_project(&project).unwrap();

        let held = store.hold_connection();
        let reader = Arc::clone(&store);
        let id = project.id.clone();
        let read = within_patience(move || {
            (
                reader.list_projects().unwrap().len(),
                reader.get_project(&id).unwrap().map(|p| p.id),
            )
        });
        drop(held);

        assert_eq!(
            read,
            Some((1, Some(project.id))),
            "reads must not wait on the write connection"
        );
    }

    #[test]
    fn a_write_completes_while_every_reader_is_mid_read() {
        let store = Arc::new(Store::in_memory().unwrap());
        store.create_project(&Project::new("owner/one")).unwrap();

        // Each reader holds an open read transaction, as a slow read would.
        let readers = store.hold_readers();
        for reader in &readers {
            reader.execute_batch("BEGIN").unwrap();
            assert_eq!(project_count(reader), 1);
        }
        let writer = Arc::clone(&store);
        let wrote = within_patience(move || writer.create_project(&Project::new("owner/two")));
        assert!(
            matches!(wrote, Some(Ok(()))),
            "writes must not wait on readers: {wrote:?}"
        );

        // The readers' snapshots predate the write; once they end, a read
        // sees it.
        for reader in &readers {
            assert_eq!(project_count(reader), 1);
            reader.execute_batch("COMMIT").unwrap();
        }
        drop(readers);
        assert_eq!(store.list_projects().unwrap().len(), 2);
    }

    #[test]
    fn a_read_right_after_a_write_sees_it() {
        let store = Store::in_memory().unwrap();
        for n in 0..20 {
            let project = Project::new(&format!("owner/repo-{n}"));
            store.create_project(&project).unwrap();
            assert!(store.get_project(&project.id).unwrap().is_some());
            let name = format!("renamed {n}");
            store.rename_project(&project.id, &name).unwrap();
            assert_eq!(store.get_project(&project.id).unwrap().unwrap().name, name);
        }
    }

    #[test]
    fn one_read_sees_one_snapshot() {
        let store = Arc::new(Store::in_memory().unwrap());
        store.create_project(&Project::new("owner/one")).unwrap();

        let read = store.read_conn();
        assert_eq!(project_count(&read), 1);
        let writer = Arc::clone(&store);
        let wrote = within_patience(move || writer.create_project(&Project::new("owner/two")));
        assert!(matches!(wrote, Some(Ok(()))), "{wrote:?}");
        assert_eq!(
            project_count(&read),
            1,
            "a commit between two statements of one read must not show"
        );
        drop(read);

        assert_eq!(project_count(&store.read_conn()), 2);
    }

    #[test]
    fn readers_refuse_writes() {
        let store = Store::in_memory().unwrap();
        let read = store.read_conn();
        let err = read
            .execute("UPDATE projects SET name = 'x' WHERE id = 'nothing'", [])
            .expect_err("a reader must not write");
        let message = err.to_string();
        assert!(
            message.contains("readonly")
                || message.contains("read-only")
                || message.contains("query_only"),
            "{message}"
        );
    }

    #[test]
    fn concurrent_readers_and_a_writer_agree() {
        let store = Arc::new(Store::in_memory().unwrap());
        let writer = Arc::clone(&store);
        let wrote = std::thread::spawn(move || {
            for n in 0..50 {
                writer
                    .create_project(&Project::new(&format!("owner/repo-{n}")))
                    .unwrap();
            }
        });
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let store = Arc::clone(&store);
                std::thread::spawn(move || {
                    let mut last = 0;
                    for _ in 0..50 {
                        let seen = store.list_projects().unwrap().len();
                        assert!(seen >= last, "a later read saw fewer rows");
                        last = seen;
                    }
                })
            })
            .collect();
        wrote.join().unwrap();
        for reader in readers {
            reader.join().unwrap();
        }
        assert_eq!(store.list_projects().unwrap().len(), 50);
    }

    #[test]
    fn without_wal_reads_share_the_write_connection() {
        // `:memory:` has no WAL, so it gets no readers.
        let store = Store::new(":memory:").unwrap();
        assert_eq!(store.readers.len(), 0);
        let project = Project::new("owner/repo");
        store.create_project(&project).unwrap();
        assert_eq!(store.list_projects().unwrap().len(), 1);
        assert!(store.get_project(&project.id).unwrap().is_some());
    }

    #[test]
    fn connections_get_their_settings() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.readers.len(), READERS);
        let pragma = |conn: &Connection, name: &str| -> String {
            conn.query_row(&format!("PRAGMA {name}"), [], |row| {
                row.get::<_, rusqlite::types::Value>(0)
            })
            .map(|value| match value {
                rusqlite::types::Value::Integer(n) => n.to_string(),
                rusqlite::types::Value::Text(text) => text,
                other => format!("{other:?}"),
            })
            .unwrap()
        };

        let writer = store.hold_connection();
        assert_eq!(pragma(&writer, "journal_mode"), "wal");
        // 1 is NORMAL.
        assert_eq!(pragma(&writer, "synchronous"), "1");
        assert_eq!(pragma(&writer, "foreign_keys"), "1");
        assert_eq!(
            pragma(&writer, "journal_size_limit"),
            JOURNAL_SIZE_LIMIT_BYTES.to_string()
        );
        assert_eq!(
            pragma(&writer, "wal_autocheckpoint"),
            WAL_AUTOCHECKPOINT_PAGES.to_string()
        );
        assert_eq!(pragma(&writer, "busy_timeout"), "5000");
        drop(writer);

        let reader = store.read_conn();
        assert_eq!(pragma(&reader, "query_only"), "1");
        assert_eq!(pragma(&reader, "busy_timeout"), "5000");
    }

    #[test]
    fn readers_are_shared_out_and_named() {
        let store = Store::in_memory().unwrap();
        let held = store.hold_readers();
        assert_eq!(held.len(), READERS);
        let mut names: Vec<_> = store.readers.readers.iter().map(|r| r.name()).collect();
        names.sort();
        assert_eq!(
            names,
            (1..=READERS)
                .map(|n| format!("store reader {n}"))
                .collect::<Vec<_>>()
        );
        // With every reader taken, the next read waits for one rather than
        // falling back to the write connection.
        let waiting = std::thread::scope(|scope| {
            let read = scope.spawn(|| store.list_projects().map(|p| p.len()));
            std::thread::sleep(Duration::from_millis(100));
            let finished_early = read.is_finished();
            drop(held);
            (finished_early, read.join().unwrap().unwrap())
        });
        assert_eq!(waiting, (false, 0));
    }

    /// With every reader busy, a read gets whichever is released first. It
    /// used to pick one reader in advance and wait for that one, however
    /// many of the others went idle meanwhile.
    #[test]
    fn a_waiting_read_takes_whichever_reader_is_released_first() {
        let store = Store::in_memory().unwrap();
        for released in 0..READERS {
            let mut held = store.hold_readers();
            let (waited, read) = std::thread::scope(|scope| {
                let read = scope.spawn(|| store.list_projects().map(|p| p.len()));
                std::thread::sleep(Duration::from_millis(100));
                let waited = !read.is_finished();
                // One reader goes idle; the rest stay busy.
                drop(held.remove(released));
                let deadline = Instant::now() + PATIENCE;
                while !read.is_finished() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let took_it = read.is_finished();
                // Lets a read that is still waiting finish, so the scope ends.
                drop(held);
                (waited, took_it.then(|| read.join().unwrap().unwrap()))
            });
            assert!(waited, "the read ran with every reader busy");
            assert_eq!(
                read,
                Some(0),
                "the read did not take the released reader (the {released}th held)"
            );
        }
    }

    /// A read that waited with every reader busy reports the wait as one
    /// for any reader, held by the call site that had them all, and not as
    /// a wait on the reader it happened to get. A read that found one free
    /// reports no such thing.
    #[test]
    fn a_slow_wait_for_a_reader_reports_who_held_them_all() {
        let store = Store::in_memory().unwrap();
        let waited_for = Duration::from_millis(150);

        let mut held = store.hold_readers();
        let report = std::thread::scope(|scope| {
            let read = scope.spawn(|| {
                let reader = store.readers.checkout().expect("the pool has readers");
                reader.report_if_slower_than(waited_for)
            });
            std::thread::sleep(waited_for * 2);
            assert!(!read.is_finished(), "the read ran with every reader busy");
            drop(held.remove(2));
            read.join().unwrap()
        })
        .expect("a wait past the threshold is reported");
        drop(held);

        // `hold_readers` checks every reader out from one line of the store.
        let (before_holders, after_holders) = report
            .split_once(" for any store reader (all held, by ")
            .unwrap_or_else(|| panic!("not a wait for any reader: {report}"));
        assert!(
            before_holders.starts_with("Slow store lock on store reader ")
                && before_holders.contains(&format!(" at {}:", file!())),
            "{report}"
        );
        let (holders, after) = after_holders
            .split_once(", when the wait began), held ")
            .unwrap_or_else(|| panic!("no holders: {report}"));
        assert!(
            holders.ends_with(|c: char| c.is_ascii_digit())
                && holders.contains("store/mod.rs:")
                && !holders.contains(", "),
            "one holding call site, in the store: {report}"
        );
        assert!(after.ends_with('s'), "{report}");

        // No wait: nothing about a pool, even with everything reported.
        let reader = store.readers.checkout().expect("the pool has readers");
        let report = reader
            .report_if_slower_than(Duration::ZERO)
            .expect("a zero threshold reports everything");
        assert!(!report.contains("any store reader"), "{report}");
        assert!(!report.contains("held by"), "{report}");
    }

    /// More waiting reads than readers: each release wakes one, and none
    /// is left asleep.
    #[test]
    fn every_waiting_read_is_served_as_readers_are_released() {
        let store = Store::in_memory().unwrap();
        let held = store.hold_readers();
        std::thread::scope(|scope| {
            let reads: Vec<_> = (0..READERS * 3)
                .map(|_| scope.spawn(|| store.list_projects().map(|p| p.len())))
                .collect();
            std::thread::sleep(Duration::from_millis(100));
            assert!(reads.iter().all(|read| !read.is_finished()));
            drop(held);
            let deadline = Instant::now() + PATIENCE;
            while reads.iter().any(|read| !read.is_finished()) {
                assert!(Instant::now() < deadline, "a waiting read was never woken");
                std::thread::sleep(Duration::from_millis(5));
            }
            for read in reads {
                assert_eq!(read.join().unwrap().unwrap(), 0);
            }
        });
    }
}
