//! The wake and network-change triggers, driven tick by tick with an
//! injected clock and a fake fingerprint.

use std::collections::BTreeSet;
use std::io;
use std::net::IpAddr;
use std::sync::Mutex;

use super::*;

/// The scheduler's tick interval.
const TICK_MS: i64 = 15_000;
/// A gap between ticks that only a suspended process produces.
const SUSPENDED_MS: i64 = WAKE_GAP_MS + 1;

#[derive(Default)]
struct FakeNetworkState {
    /// `None` makes the read fail.
    addresses: Option<BTreeSet<IpAddr>>,
    reads: usize,
}

/// A fingerprint the test sets by hand, which counts how often it is read.
#[derive(Clone)]
struct FakeNetwork(Arc<Mutex<FakeNetworkState>>);

impl FakeNetwork {
    fn with_addresses(addresses: &[&str]) -> Self {
        let network = Self(Arc::default());
        network.set_addresses(addresses);
        network
    }

    fn set_addresses(&self, addresses: &[&str]) {
        self.0.lock().unwrap().addresses = Some(
            addresses
                .iter()
                .map(|address| address.parse().unwrap())
                .collect(),
        );
    }

    fn make_unreadable(&self) {
        self.0.lock().unwrap().addresses = None;
    }

    fn reads(&self) -> usize {
        self.0.lock().unwrap().reads
    }

    fn scheduler(&self) -> RateLimitScheduler {
        RateLimitScheduler::with_network_fingerprint(Box::new(self.clone()))
    }
}

impl NetworkFingerprint for FakeNetwork {
    fn read(&self) -> io::Result<BTreeSet<IpAddr>> {
        let mut state = self.0.lock().unwrap();
        state.reads += 1;
        state
            .addresses
            .clone()
            .ok_or_else(|| io::Error::other("interfaces unavailable"))
    }
}

const WIFI: &[&str] = &["192.0.2.10"];
const OTHER_WIFI: &[&str] = &["198.51.100.7"];

/// Tick every 15 seconds from `from`, failing each probe as it starts, until
/// four have failed. The fourth fails 105 seconds in and leaves the next
/// probe two minutes away. Returns the time of the last tick.
fn fail_four_probes(scheduler: &RateLimitScheduler, store: &Arc<Store>, from: i64) -> i64 {
    let mut at = from;
    let mut failures = 0;
    loop {
        for session_id in tick_with_triggers(scheduler, store, at) {
            fail_on_network(store, scheduler, &session_id, at);
            failures += 1;
        }
        if failures == 4 {
            assert_eq!(at, from + 7 * TICK_MS);
            return at;
        }
        at += TICK_MS;
    }
}

#[test]
fn a_long_gap_resets_the_probe_delay() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = FakeNetwork::with_addresses(WIFI).scheduler();
    let now = crate::store::now_timestamp();
    network_paused_sessions(&store, &scheduler, 2, now);
    let last = fail_four_probes(&scheduler, &store, now);

    // The next probe is two minutes off, so an ordinary tick starts nothing.
    let before_sleep = last + TICK_MS;
    assert!(tick_with_triggers(&scheduler, &store, before_sleep).is_empty());

    // The machine sleeps. The tick that sees the gap starts nothing, and the
    // one after it probes, long before the two minutes are up.
    let woke = before_sleep + SUSPENDED_MS;
    assert!(tick_with_triggers(&scheduler, &store, woke).is_empty());
    let probed = woke + TICK_MS;
    let probe = tick_with_triggers(&scheduler, &store, probed);
    assert_eq!(probe.len(), 1);

    // The doubling starts over too: a failure waits the base delay.
    fail_on_network(&store, &scheduler, &probe[0], probed);
    assert!(tick_with_triggers(&scheduler, &store, probed + BASE_PROBE_DELAY_MS - 1).is_empty());
    assert_eq!(
        tick_with_triggers(&scheduler, &store, probed + BASE_PROBE_DELAY_MS).len(),
        1
    );
}

#[test]
fn no_probe_starts_on_the_tick_after_a_long_gap() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = FakeNetwork::with_addresses(WIFI).scheduler();
    let now = crate::store::now_timestamp();
    assert!(tick_with_triggers(&scheduler, &store, now).is_empty());
    // A probe is due at once: the gate is fresh.
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    let woke = now + SUSPENDED_MS;
    assert!(tick_with_triggers(&scheduler, &store, woke).is_empty());
    assert_eq!(
        tick_with_triggers(&scheduler, &store, woke + TICK_MS),
        sessions
    );
}

#[test]
fn a_gap_of_exactly_the_threshold_is_normal() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = FakeNetwork::with_addresses(WIFI).scheduler();
    let now = crate::store::now_timestamp();
    assert!(tick_with_triggers(&scheduler, &store, now).is_empty());
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    assert_eq!(
        tick_with_triggers(&scheduler, &store, now + WAKE_GAP_MS),
        sessions
    );
}

#[test]
fn a_run_of_long_gaps_never_starts_a_probe() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = FakeNetwork::with_addresses(WIFI).scheduler();
    let now = crate::store::now_timestamp();
    assert!(tick_with_triggers(&scheduler, &store, now).is_empty());
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    // Maintenance wakes during a night of sleep: one tick each, far apart.
    let mut at = now;
    for _ in 0..20 {
        at += 20 * 60_000;
        assert!(tick_with_triggers(&scheduler, &store, at).is_empty());
    }
    assert_eq!(recovery(&store, &sessions[0]).attempt, 0);

    assert_eq!(
        tick_with_triggers(&scheduler, &store, at + TICK_MS),
        sessions
    );
}

#[test]
fn a_fingerprint_change_resets_the_delay() {
    let store = Arc::new(Store::in_memory().unwrap());
    let network = FakeNetwork::with_addresses(WIFI);
    let scheduler = network.scheduler();
    let now = crate::store::now_timestamp();
    network_paused_sessions(&store, &scheduler, 2, now);
    let last = fail_four_probes(&scheduler, &store, now);

    // The same network: the two minute wait stands.
    assert!(tick_with_triggers(&scheduler, &store, last + TICK_MS).is_empty());

    network.set_addresses(OTHER_WIFI);
    let probed = last + 2 * TICK_MS;
    let probe = tick_with_triggers(&scheduler, &store, probed);
    assert_eq!(probe.len(), 1);

    fail_on_network(&store, &scheduler, &probe[0], probed);
    assert!(tick_with_triggers(&scheduler, &store, probed + BASE_PROBE_DELAY_MS - 1).is_empty());
    assert_eq!(
        tick_with_triggers(&scheduler, &store, probed + BASE_PROBE_DELAY_MS).len(),
        1
    );
}

/// Tick every 15 seconds from `from` until a probe starts, and return it
/// with the time it started.
fn next_probe(scheduler: &RateLimitScheduler, store: &Arc<Store>, from: i64) -> (String, i64) {
    let mut at = from;
    loop {
        let mut started = tick_with_triggers(scheduler, store, at);
        if let Some(session_id) = started.pop() {
            assert!(started.is_empty());
            return (session_id, at);
        }
        at += TICK_MS;
        assert!(at < from + 2 * MAX_PROBE_DELAY_MS, "no probe started");
    }
}

/// No address at all is not a reason to stop probing: the provider may be
/// on this machine. The doubling schedule runs as it does with a network.
#[test]
fn an_empty_fingerprint_does_not_stop_the_doubling_probe_schedule() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = FakeNetwork::with_addresses(&[]).scheduler();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    let mut at = now;
    let mut waits = Vec::new();
    for _ in 0..4 {
        let (probe, started_at) = next_probe(&scheduler, &store, at);
        assert_eq!(probe, sessions[0]);
        waits.push(started_at - at);
        fail_on_network(&store, &scheduler, &probe, started_at);
        at = started_at;
    }
    assert_eq!(waits, [0, 15_000, 30_000, 60_000]);
    assert_eq!(recovery(&store, &sessions[0]).attempt, 0);
}

/// Losing every address is a change to an empty fingerprint, which resets
/// nothing, and a probe that comes due meanwhile still starts.
#[test]
fn losing_every_address_neither_resets_the_delay_nor_blocks_a_due_probe() {
    let store = Arc::new(Store::in_memory().unwrap());
    let network = FakeNetwork::with_addresses(WIFI);
    let scheduler = network.scheduler();
    let now = crate::store::now_timestamp();
    network_paused_sessions(&store, &scheduler, 2, now);
    let last = fail_four_probes(&scheduler, &store, now);

    // The next probe is two minutes off. The addresses go: no early probe.
    network.set_addresses(&[]);
    let due = last + 120_000;
    let mut at = last + TICK_MS;
    while at < due {
        assert!(tick_with_triggers(&scheduler, &store, at).is_empty());
        at += TICK_MS;
    }
    // Due, with still no address: it starts all the same.
    assert_eq!(tick_with_triggers(&scheduler, &store, due).len(), 1);
}

/// An address appearing after there was none is a change that does reset.
#[test]
fn an_address_appearing_resets_the_delay() {
    let store = Arc::new(Store::in_memory().unwrap());
    let network = FakeNetwork::with_addresses(&[]);
    let scheduler = network.scheduler();
    let now = crate::store::now_timestamp();
    network_paused_sessions(&store, &scheduler, 2, now);
    let last = fail_four_probes(&scheduler, &store, now);
    assert!(tick_with_triggers(&scheduler, &store, last + TICK_MS).is_empty());

    network.set_addresses(WIFI);
    assert_eq!(
        tick_with_triggers(&scheduler, &store, last + 2 * TICK_MS).len(),
        1
    );
}

/// A wake with no network has nothing new to probe: the schedule stands.
#[test]
fn a_wake_with_no_network_leaves_the_probe_schedule_alone() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = FakeNetwork::with_addresses(&[]).scheduler();
    let now = crate::store::now_timestamp();
    network_paused_sessions(&store, &scheduler, 2, now);
    let last = fail_four_probes(&scheduler, &store, now);
    let due = last + 120_000;

    let woke = last + TICK_MS + SUSPENDED_MS;
    let mut at = woke;
    while at < due {
        assert!(
            tick_with_triggers(&scheduler, &store, at).is_empty(),
            "{} ms before the probe is due",
            due - at
        );
        at += TICK_MS;
    }
    assert_eq!(tick_with_triggers(&scheduler, &store, at).len(), 1);
}

/// A read that fails says nothing about the network, so it does not hold a
/// wake back the way an empty fingerprint does.
#[test]
fn an_unreadable_fingerprint_does_not_hold_back_a_wake() {
    let store = Arc::new(Store::in_memory().unwrap());
    let network = FakeNetwork::with_addresses(&[]);
    let scheduler = network.scheduler();
    let now = crate::store::now_timestamp();
    network_paused_sessions(&store, &scheduler, 2, now);
    let last = fail_four_probes(&scheduler, &store, now);

    network.make_unreadable();
    let woke = last + TICK_MS + SUSPENDED_MS;
    assert!(tick_with_triggers(&scheduler, &store, woke).is_empty());
    assert_eq!(
        tick_with_triggers(&scheduler, &store, woke + TICK_MS).len(),
        1
    );
}

/// The whole path for a lone session behind a dead proxy: the machine is
/// awake with an address, every probe fails, and after the twentieth the
/// session is left paused for a manual resume.
#[test]
fn a_lone_session_gives_up_after_twenty_failed_probes_with_a_network() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = FakeNetwork::with_addresses(WIFI).scheduler();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    let mut at = now;
    for failure in 1..=MAX_CONSECUTIVE_PROBE_FAILURES {
        let (probe, started_at) = next_probe(&scheduler, &store, at);
        fail_on_network(&store, &scheduler, &probe, started_at);
        at = started_at;

        let state = recovery(&store, &sessions[0]);
        assert_eq!(state.attempt, 0);
        if failure < MAX_CONSECUTIVE_PROBE_FAILURES {
            assert_eq!(state.gave_up_at, None, "failure {failure}");
            assert_eq!(state.reason.as_deref(), Some("network_wait"));
        } else {
            assert_eq!(state.gave_up_at, Some(at));
            assert_eq!(state.next_retry_at, None);
            assert_eq!(state.reason.as_deref(), Some("probe_limit_exceeded"));
        }
    }

    // Still paused, and never probed with again.
    let row = store.get_session(&sessions[0]).unwrap().unwrap();
    assert_eq!(row.status, SessionStatus::Paused);
    let end = at + 2 * MAX_PROBE_DELAY_MS;
    while at < end {
        at += TICK_MS;
        assert!(tick_with_triggers(&scheduler, &store, at).is_empty());
    }
}

/// With no network the same run of failures gives up on nothing.
#[test]
fn failed_probes_with_no_network_never_give_up() {
    let store = Arc::new(Store::in_memory().unwrap());
    let scheduler = FakeNetwork::with_addresses(&[]).scheduler();
    let now = crate::store::now_timestamp();
    let sessions = network_paused_sessions(&store, &scheduler, 1, now);

    let mut at = now;
    for _ in 0..MAX_CONSECUTIVE_PROBE_FAILURES + 5 {
        let (probe, started_at) = next_probe(&scheduler, &store, at);
        fail_on_network(&store, &scheduler, &probe, started_at);
        at = started_at;
    }
    let state = recovery(&store, &sessions[0]);
    assert_eq!(state.gave_up_at, None);
    assert_eq!(state.reason.as_deref(), Some("network_wait"));
}

#[test]
fn the_fingerprint_is_read_only_while_closed_with_network_rows() {
    let store = Arc::new(Store::in_memory().unwrap());
    let network = FakeNetwork::with_addresses(WIFI);
    let scheduler = network.scheduler();
    // The store stamps the probe's reply with the wall clock, so the ticks
    // run in the past: the reply then lands after the probe's claim.
    let now = crate::store::now_timestamp() - 10 * TICK_MS;

    // Closed, but nothing is paused on the network.
    assert!(tick_with_triggers(&scheduler, &store, now).is_empty());
    assert_eq!(network.reads(), 0);

    // Closed with a row waiting: read, and the row probes.
    let sessions = network_paused_sessions(&store, &scheduler, 2, now);
    assert_eq!(
        tick_with_triggers(&scheduler, &store, now + TICK_MS),
        [sessions[0].clone()]
    );
    assert_eq!(network.reads(), 1);

    // Probing: not read.
    assert!(tick_with_triggers(&scheduler, &store, now + 2 * TICK_MS).is_empty());
    assert_eq!(network.reads(), 1);

    // Open: not read.
    agent_replies(&store, &sessions[0]);
    assert_eq!(
        tick_with_triggers(&scheduler, &store, now + 3 * TICK_MS),
        [sessions[1].clone()]
    );
    assert_eq!(network.reads(), 1);
}

#[test]
fn the_real_fingerprint_leaves_out_loopback_and_link_local_addresses() {
    let Ok(addresses) = InterfaceAddresses.read() else {
        return;
    };
    for address in addresses {
        assert!(!address.is_loopback(), "{address}");
        let link_local = match address {
            IpAddr::V4(v4) => v4.is_link_local(),
            IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
        };
        assert!(!link_local, "{address}");
    }
}
