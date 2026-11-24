//! The counters the harness reads over `/perf/metrics`. All in memory, and
//! nothing is recorded until [`activate`] is called.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::histogram::Histogram;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static STORE_LOCK_WAIT: Histogram = Histogram::new();
static STORE_LOCK_HOLD: Histogram = Histogram::new();
/// Text, which the writer flushes on a timer.
static TEXT_STAMP_TO_STORED: Histogram = Histogram::new();
/// Tool calls, which it writes at once.
static TOOL_STAMP_TO_STORED: Histogram = Histogram::new();
static TABLES: Mutex<Option<Tables>> = Mutex::new(None);

#[derive(Default)]
struct Tables {
    /// By command name, the app's own callers and the harness driver apart.
    commands: BTreeMap<String, Command>,
    driver_commands: BTreeMap<String, Command>,
    /// By `<level>.<kind>`.
    frames: BTreeMap<String, Frames>,
    /// The newest stamp counted per fake agent.
    stamps: HashMap<String, u64>,
}

#[derive(Default)]
struct Command {
    count: u64,
    errors: u64,
    wire_bytes: u64,
    total_us: u64,
    max_us: u64,
}

#[derive(Default)]
struct Frames {
    frames: u64,
    bytes: u64,
}

pub(super) fn activate() {
    ACTIVE.store(true, Relaxed);
}

fn tables<R>(f: impl FnOnce(&mut Tables) -> R) -> R {
    let mut guard = TABLES.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Tables::default))
}

pub(super) fn store_lock_waited(wait: Duration) {
    if ACTIVE.load(Relaxed) {
        STORE_LOCK_WAIT.record(wait.as_micros() as u64);
    }
}

pub(super) fn store_lock_held(hold: Duration) {
    if ACTIVE.load(Relaxed) {
        STORE_LOCK_HOLD.record(hold.as_micros() as u64);
    }
}

/// A command served over the harness port: `wire_bytes` is the response
/// body as sent, so after compression when the caller accepted it.
pub(super) fn command(name: &str, driver: bool, ok: bool, wire_bytes: u64, elapsed: Duration) {
    let us = elapsed.as_micros() as u64;
    tables(|tables| {
        let table = if driver {
            &mut tables.driver_commands
        } else {
            &mut tables.commands
        };
        let entry = table.entry(name.to_string()).or_default();
        entry.count += 1;
        entry.errors += u64::from(!ok);
        entry.wire_bytes += wire_bytes;
        entry.total_us += us;
        entry.max_us = entry.max_us.max(us);
    });
}

pub(super) fn is_active() -> bool {
    ACTIVE.load(Relaxed)
}

/// A session stream frame of `bytes` handed to a client, by level and kind.
pub(super) fn stream_frame(level: &str, kind: &str, bytes: u64) {
    tables(|tables| {
        let entry = tables.frames.entry(format!("{level}.{kind}")).or_default();
        entry.frames += 1;
        entry.bytes += bytes;
    });
}

/// A message row's content was just written. The fake agent stamps its
/// output with `pfm-<kind><agent>-<seq>-<epoch ms>`, kind `x` for text and
/// `t` for a tool call; each stamp this write stores for the first time
/// says how long ago the agent wrote that piece. Streamed text is flushed
/// on a timer (`agent/writer.rs`), so for text the figure includes up to
/// one flush interval by design.
pub(crate) fn message_stored(content: &str) {
    if !ACTIVE.load(Relaxed) {
        return;
    }
    let mut stamps = stamps_newest_first(content);
    let Some(newest) = stamps.next() else {
        return;
    };
    let (agent, seq, _) = newest;
    // A row rewritten without new output must not count again as slower.
    let last = tables(|tables| match tables.stamps.get_mut(agent) {
        Some(last) => Some(std::mem::replace(last, seq.max(*last))),
        None => tables.stamps.insert(agent.to_string(), seq),
    });
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let histogram = if agent.starts_with('t') {
        &TOOL_STAMP_TO_STORED
    } else {
        &TEXT_STAMP_TO_STORED
    };
    for (_, _, sent_ms) in std::iter::once(newest)
        .chain(stamps)
        .take_while(|(other, seq, _)| *other == agent && !last.is_some_and(|last| *seq <= last))
        .take(MAX_STAMPS_PER_WRITE)
    {
        histogram.record(now_ms.saturating_sub(sent_ms) * 1_000);
    }
}

const STAMP_PREFIX: &str = "pfm-";
/// A bound on the scan, far above what one flush interval can hold.
const MAX_STAMPS_PER_WRITE: usize = 256;

/// `(kind and agent, seq, epoch ms)` of each whole stamp, last one first.
fn stamps_newest_first(content: &str) -> impl Iterator<Item = (&str, u64, u64)> {
    content
        .rmatch_indices(STAMP_PREFIX)
        .filter_map(move |(at, _)| parse_stamp(&content[at + STAMP_PREFIX.len()..]))
}

fn parse_stamp(rest: &str) -> Option<(&str, u64, u64)> {
    let mut parts = rest.splitn(3, '-');
    let agent = parts.next()?;
    let seq = parts.next()?.parse().ok()?;
    let sent = parts.next()?;
    let digits = sent.bytes().take_while(u8::is_ascii_digit).count();
    Some((agent, seq, sent[..digits].parse().ok()?))
}

pub(super) fn reset() {
    STORE_LOCK_WAIT.reset();
    STORE_LOCK_HOLD.reset();
    TEXT_STAMP_TO_STORED.reset();
    TOOL_STAMP_TO_STORED.reset();
    tables(|tables| {
        tables.commands.clear();
        tables.driver_commands.clear();
        tables.frames.clear();
    });
}

pub(super) fn snapshot() -> Value {
    let commands = |table: &BTreeMap<String, Command>| -> Value {
        table
            .iter()
            .map(|(name, c)| {
                let stats = json!({
                    "count": c.count,
                    "errors": c.errors,
                    "wireBytes": c.wire_bytes,
                    "totalUs": c.total_us,
                    "maxUs": c.max_us,
                });
                (name.clone(), stats)
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    };
    tables(|tables| {
        let frames: serde_json::Map<_, _> = tables
            .frames
            .iter()
            .map(|(key, f)| (key.clone(), json!({ "frames": f.frames, "bytes": f.bytes })))
            .collect();
        json!({
            "pid": std::process::id(),
            "dataDir": crate::paths::data_dir(),
            "commands": commands(&tables.commands),
            "driverCommands": commands(&tables.driver_commands),
            "streamFrames": frames,
            "storeLock": {
                "wait": STORE_LOCK_WAIT.snapshot(),
                "hold": STORE_LOCK_HOLD.snapshot(),
            },
            "stampToStored": {
                "text": TEXT_STAMP_TO_STORED.snapshot(),
                "tool": TOOL_STAMP_TO_STORED.snapshot(),
            },
        })
    })
}

#[cfg(test)]
mod tests {
    use super::stamps_newest_first;

    #[test]
    fn stamps_are_read_newest_first() {
        let content = "one pfm-xs1-4-1700000000100 two pfm-xs1-5-1700000000250 three";
        assert_eq!(
            stamps_newest_first(content).collect::<Vec<_>>(),
            [("xs1", 5, 1_700_000_000_250), ("xs1", 4, 1_700_000_000_100)]
        );
        assert_eq!(
            stamps_newest_first(r#"{"title":"Run pfm-tab-12-1700000000001"}"#).next(),
            Some(("tab", 12, 1_700_000_000_001))
        );
    }

    #[test]
    fn content_with_no_whole_stamp_has_none() {
        assert_eq!(stamps_newest_first("plain text").next(), None);
        assert_eq!(stamps_newest_first("cut off pfm-s1-4").next(), None);
        assert_eq!(stamps_newest_first("not a number pfm-s1-x-12").next(), None);
    }
}
