//! The counters the harness reads over `/perf/metrics`. All in memory, and
//! nothing is recorded until [`activate`] is called.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
    /// The newest stamp counted per stamp kind and fake agent.
    stamps: HashMap<String, u64>,
    /// Per fake agent, the last [`SHOWABLE_KEPT`] stamps its session's row
    /// can show (text and tool calls), in the order they were stored.
    showable: HashMap<String, VecDeque<Showable>>,
}

/// A stamp a row can show, and when its row was written.
#[derive(Clone, Copy)]
struct Showable {
    seq: u64,
    stored_ms: u64,
}

/// Seconds of output at any rate the harness is run at; a row further
/// behind than this gets a floor, not a figure (see [`outdated`]).
const SHOWABLE_KEPT: usize = 256;

#[derive(Default)]
struct Command {
    count: u64,
    errors: u64,
    wire_bytes: u64,
    total_us: u64,
    max_us: u64,
}

/// Frames the hub sent, of one level and kind. `frames` and `bytes` are
/// those a sink took; `dropped` those it did not (a full queue, a client
/// that had gone), which reached nobody.
#[derive(Default)]
struct Frames {
    frames: u64,
    bytes: u64,
    dropped: u64,
}

impl Tables {
    fn count_frame(&mut self, level: &str, kind: &str, bytes: u64, delivered: bool) {
        let entry = self.frames.entry(format!("{level}.{kind}")).or_default();
        if delivered {
            entry.frames += 1;
            entry.bytes += bytes;
        } else {
            entry.dropped += 1;
        }
    }

    fn frames_json(&self) -> Value {
        self.frames
            .iter()
            .map(|(key, f)| {
                let counts = json!({ "frames": f.frames, "bytes": f.bytes, "dropped": f.dropped });
                (key.clone(), counts)
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    }
}

pub(super) fn activate() {
    ACTIVE.store(true, Relaxed);
}

fn tables<R>(f: impl FnOnce(&mut Tables) -> R) -> R {
    let mut guard = TABLES.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Tables::default))
}

pub(crate) fn store_lock_waited(wait: Duration) {
    if ACTIVE.load(Relaxed) {
        STORE_LOCK_WAIT.record(wait.as_micros() as u64);
    }
}

pub(crate) fn store_lock_held(hold: Duration) {
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

/// A session stream frame of `bytes` the hub sent a client, by level and
/// kind, and whether the client's sink took it.
pub(super) fn stream_frame(level: &str, kind: &str, bytes: u64, delivered: bool) {
    tables(|tables| tables.count_frame(level, kind, bytes, delivered));
}

/// A message row's content was just written. The fake agent stamps its
/// output with `pfm-<kind><agent>-<seq>-<epoch ms>`, kind `x` for text, `t`
/// for a tool call and `o` for a tool call's output; each stamp this write
/// stores for the first time says how long ago the agent wrote that piece.
/// Streamed text is flushed on a timer (`agent/writer.rs`), so for text the
/// figure includes up to one flush interval by design. Tool calls and their
/// output are written at once and share a figure.
///
/// A stamp is counted once per write however often the content holds it: a
/// tool call's row stores the call's whole input, and the fake agent puts a
/// Bash call's stamp in both its command and its description.
pub(crate) fn message_stored(content: &str) {
    if !ACTIVE.load(Relaxed) {
        return;
    }
    let Some((agent, newest, _)) = stamps_newest_first(content).next() else {
        return;
    };
    // A row rewritten without new output must not count again as slower.
    let last = tables(|tables| match tables.stamps.get_mut(agent) {
        Some(last) => Some(std::mem::replace(last, newest.max(*last))),
        None => tables.stamps.insert(agent.to_string(), newest),
    });
    let now_ms = now_ms();
    let histogram = if agent.starts_with('x') {
        &TEXT_STAMP_TO_STORED
    } else {
        &TOOL_STAMP_TO_STORED
    };
    let fresh = fresh_stamps(content, last);
    for &(_, sent_ms) in &fresh {
        histogram.record(now_ms.saturating_sub(sent_ms) * 1_000);
    }
    // Tool output (`o`) is never a row's subtitle.
    if fresh.is_empty() || !agent.starts_with(['x', 't']) {
        return;
    }
    tables(|tables| {
        let kept = tables.showable.entry(agent[1..].to_string()).or_default();
        for &(seq, _) in fresh.iter().rev() {
            if kept.len() == SHOWABLE_KEPT {
                kept.pop_front();
            }
            kept.push_back(Showable {
                seq,
                stored_ms: now_ms,
            });
        }
    });
}

/// `(seq, epoch ms)` of the stamps a write stores for the first time,
/// newest first: those of the newest stamp's kind and agent above `last`,
/// the newest one an earlier write of that kind and agent stored. Each
/// sequence number once, however often the content holds its stamp.
fn fresh_stamps(content: &str, last: Option<u64>) -> Vec<(u64, u64)> {
    let mut stamps = stamps_newest_first(content).peekable();
    let Some(&(agent, _, _)) = stamps.peek() else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    stamps
        .take_while(|(other, seq, _)| *other == agent && !last.is_some_and(|last| *seq <= last))
        .filter(|(_, seq, _)| seen.insert(*seq))
        .take(MAX_STAMPS_PER_WRITE)
        .map(|(_, seq, sent_ms)| (seq, sent_ms))
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// For each fake agent in `shown` (`{ "<agent>": seq }`, the stamp its
/// session's row shows, 0 for none): whether the store holds something
/// newer that the row could show, since when, and how much.
///
/// The answer per agent is `null` when nothing of it has been stored, and
/// otherwise `{ newestSeq, newerSeq, newerStoredMs, newerCount, floor }`:
/// `newerSeq` is the first stamp stored after the one shown and
/// `newerStoredMs` when its row was written (epoch ms), both `null` when
/// the row shows the newest. `newerCount` is how many showable stamps newer
/// than the one shown were stored by `at`, when the row was read (epoch ms;
/// all of them without it). It is a count of what a row can show, not a
/// difference of sequence numbers: one sequence runs through text, tool
/// calls and tool output, and tool output is never shown. `floor` says the
/// stamps between were no longer kept, so the row has been behind for at
/// least that long and by at least that many.
///
/// Read from memory, so the harness can ask on a timer during a measurement
/// without taking the store's lock. Never reset: it describes the store,
/// not a window.
pub(super) fn outdated(shown: &HashMap<String, u64>, at: Option<u64>) -> Value {
    tables(|tables| {
        shown
            .iter()
            .map(|(agent, &shown_seq)| {
                let answer = tables.showable.get(agent).map(|kept| {
                    let newest = kept.iter().map(|s| s.seq).max();
                    // Text and tool rows are written by different paths, so
                    // the order kept is not strictly the order of `seq`.
                    let newer = kept
                        .iter()
                        .filter(|s| s.seq > shown_seq)
                        .min_by_key(|s| s.seq);
                    let oldest = kept.iter().map(|s| s.seq).min();
                    let newer_count = kept
                        .iter()
                        .filter(|s| s.seq > shown_seq)
                        .filter(|s| at.map_or(true, |at| s.stored_ms <= at))
                        .count();
                    json!({
                        "newestSeq": newest,
                        "newerSeq": newer.map(|s| s.seq),
                        "newerStoredMs": newer.map(|s| s.stored_ms),
                        "newerCount": newer_count,
                        "floor": newer.is_some()
                            && kept.len() == SHOWABLE_KEPT
                            && oldest.is_some_and(|oldest| oldest > shown_seq),
                    })
                });
                (agent.clone(), answer.unwrap_or(Value::Null))
            })
            .collect::<serde_json::Map<_, _>>()
            .into()
    })
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

/// What these hooks can do, for a driver newer or older than they are:
/// 2 added [`outdated`], the `o` stamp kind and the finer histograms; 3 put
/// every route behind the run's token, counts a stamp once per write,
/// added `newerCount` to [`outdated`], and counts a stream frame when its
/// sink has taken it, with the ones not taken apart.
const HOOKS_VERSION: u32 = 3;

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
        let frames = tables.frames_json();
        json!({
            "hooks": HOOKS_VERSION,
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
    use std::collections::HashMap;

    use super::{
        activate, fresh_stamps, message_stored, outdated, stamps_newest_first, Tables,
        SHOWABLE_KEPT,
    };

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

    fn shown(agent: &str, seq: u64) -> HashMap<String, u64> {
        HashMap::from([(agent.to_string(), seq)])
    }

    #[test]
    fn a_row_is_outdated_from_the_first_showable_stamp_stored_after_the_one_it_shows() {
        activate();
        message_stored("pfm-xout1-3-1700000000100 a pfm-xout1-4-1700000000200 b");
        message_stored(r#"{"title":"Run pfm-tout1-5-1700000000300"}"#);
        // Tool output is stored and never shown as a subtitle.
        message_stored("output pfm-oout1-6-1700000000400");
        // A row rewritten with nothing new adds nothing.
        message_stored("pfm-xout1-3-1700000000100 a pfm-xout1-4-1700000000200 b");

        let behind = outdated(&shown("out1", 3), None);
        assert_eq!(behind["out1"]["newestSeq"], 5);
        assert_eq!(behind["out1"]["newerSeq"], 4);
        assert!(behind["out1"]["newerStoredMs"].as_u64().unwrap() > 1_700_000_000_200);
        assert_eq!(behind["out1"]["floor"], false);

        let current = outdated(&shown("out1", 5), None);
        assert_eq!(current["out1"]["newestSeq"], 5);
        assert!(current["out1"]["newerSeq"].is_null());
        assert!(current["out1"]["newerStoredMs"].is_null());
        assert_eq!(current["out1"]["newerCount"], 0);

        assert!(outdated(&shown("nobody", 0), None)["nobody"].is_null());
    }

    #[test]
    fn pieces_behind_are_the_showable_stamps_newer_than_the_one_shown() {
        activate();
        // One sequence through the three kinds: a tool call (25), five
        // steps of its output (26 to 30), then text (31).
        message_stored(
            r#"{"name":"Read","input":{"file_path":"/w/pfm-tout3-25-1700000000100.ts"}}"#,
        );
        for seq in 26..=30 {
            message_stored(&format!("output so far pfm-oout3-{seq}-1700000000200"));
        }
        message_stored("pfm-xout3-31-1700000000300 text");

        // By sequence numbers the row is 6 behind; it can show one thing more.
        let behind = outdated(&shown("out3", 25), None);
        assert_eq!(behind["out3"]["newestSeq"], 31);
        assert_eq!(behind["out3"]["newerSeq"], 31);
        assert_eq!(behind["out3"]["newerCount"], 1);
        assert_eq!(outdated(&shown("out3", 0), None)["out3"]["newerCount"], 2);

        // A piece stored after the row was read is not one it was behind by.
        let stored = behind["out3"]["newerStoredMs"].as_u64().unwrap();
        let before = outdated(&shown("out3", 25), Some(stored - 1));
        assert_eq!(before["out3"]["newerCount"], 0);
        let after = outdated(&shown("out3", 25), Some(stored));
        assert_eq!(after["out3"]["newerCount"], 1);
    }

    #[test]
    fn a_stamp_a_row_holds_twice_is_counted_once() {
        // A Bash call as the fake agent sends it and the writer stores it:
        // the stamp in the command and again in the description.
        let bash = r#"{"name":"Bash","input":{"command":"run pfm-tdup1-7-1700000000100","description":"Run pfm-tdup1-7-1700000000100"}}"#;
        assert_eq!(fresh_stamps(bash, None), [(7, 1_700_000_000_100)]);
        assert_eq!(fresh_stamps(bash, Some(6)), [(7, 1_700_000_000_100)]);
        assert_eq!(fresh_stamps(bash, Some(7)), []);
        // Text flushed together keeps every piece, newest first, and stops
        // at what an earlier write stored.
        let text =
            "pfm-xdup1-3-1700000000100 a\npfm-xdup1-4-1700000000200 b\npfm-xdup1-5-1700000000300 c";
        assert_eq!(
            fresh_stamps(text, Some(3)),
            [(5, 1_700_000_000_300), (4, 1_700_000_000_200)]
        );

        // And once among what a row can show.
        activate();
        message_stored(bash);
        let kept = outdated(&shown("dup1", 0), None);
        assert_eq!(kept["dup1"]["newestSeq"], 7);
        assert_eq!(kept["dup1"]["newerCount"], 1);
    }

    #[test]
    fn a_row_further_behind_than_what_is_kept_gets_a_floor() {
        activate();
        for seq in 1..=(SHOWABLE_KEPT as u64 + 10) {
            message_stored(&format!("pfm-xout2-{seq}-1700000000000"));
        }
        let behind = outdated(&shown("out2", 2), None);
        assert_eq!(behind["out2"]["newestSeq"], SHOWABLE_KEPT as u64 + 10);
        assert_eq!(behind["out2"]["newerSeq"], 11);
        assert_eq!(behind["out2"]["floor"], true);
        // A least value: only what is kept can be counted.
        assert_eq!(behind["out2"]["newerCount"], SHOWABLE_KEPT);
        assert_eq!(outdated(&shown("out2", 11), None)["out2"]["floor"], false);
    }

    #[test]
    fn a_frame_counts_as_sent_only_when_its_sink_took_it() {
        let mut tables = Tables::default();
        tables.count_frame("full", "delta", 100, true);
        tables.count_frame("full", "delta", 40, true);
        tables.count_frame("full", "delta", 900, false);
        tables.count_frame("summary", "snapshot", 7, false);
        let frames = tables.frames_json();
        assert_eq!(frames["full.delta"]["frames"], 2);
        assert_eq!(frames["full.delta"]["bytes"], 140);
        assert_eq!(frames["full.delta"]["dropped"], 1);
        assert_eq!(frames["summary.snapshot"]["frames"], 0);
        assert_eq!(frames["summary.snapshot"]["bytes"], 0);
        assert_eq!(frames["summary.snapshot"]["dropped"], 1);
    }

    #[test]
    fn content_with_no_whole_stamp_has_none() {
        assert_eq!(stamps_newest_first("plain text").next(), None);
        assert_eq!(stamps_newest_first("cut off pfm-s1-4").next(), None);
        assert_eq!(stamps_newest_first("not a number pfm-s1-x-12").next(), None);
    }
}
