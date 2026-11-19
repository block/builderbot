//! The shared gate that every network-paused session waits behind.
//!
//! A session paused with kind [`NETWORK_PAUSE_KIND`](super::NETWORK_PAUSE_KIND)
//! failed because the agent could not reach its model provider. That is a
//! fact about the machine, not about the session, so all such sessions share
//! one gate instead of each running its own backoff. API-limit pauses never
//! pass through it.
//!
//! | State   | Meaning                          | What the scheduler does         |
//! | ------- | -------------------------------- | ------------------------------- |
//! | Closed  | The network is presumed down     | Starts one probe when it is due |
//! | Probing | One probe turn is running        | Starts no other network resume  |
//! | Open    | A probe reached its provider     | Resumes the rest, two per tick  |
//!
//! The gate lives in memory, owned by [`RateLimitScheduler`] behind a mutex,
//! and starts closed with a probe due at once, so a relaunch probes on its
//! first tick.
//!
//! A probe succeeds when its turn reaches the model provider (see
//! [`probe_verdict`]). A probe still running after [`PROBE_TIMEOUT_MS`] with
//! no such sign is given up on as a probe: the gate closes and the schedule
//! moves on as after a failed probe, while that session keeps running. A
//! user's manual resume of a network-paused session is watched as a probe
//! too when the gate is closed, so its success opens the gate for the rest.
//!
//! Failures while the gate is not open are not counted: the session's
//! `attempt` does not advance and the 24 hour window is not applied, so a
//! laptop that sleeps for days gives up on nothing. Failures while it is open
//! are counted under the existing cap, and close it again.
//!
//! One bound applies to uncounted failures. A session whose own probes fail
//! [`MAX_CONSECUTIVE_PROBE_FAILURES`] times in a row, each while the machine
//! stayed awake and held a network address, is given up on and left paused
//! for a manual resume: with a network evidently present, its failure is not
//! one that waiting fixes (a dead local proxy, a captive portal).
//!
//! A probe that fails only after the gate stopped waiting for it is still
//! that session's failed probe. The gate remembers each timed-out probe until
//! the gate opens or the session probes again, charges its late network
//! failure to the same bound under the same conditions, and holds the next
//! probe back by the probe delay, so a session that always fails late cannot
//! be probed again and again without limit.
//!
//! Two things make a closed gate probe sooner than its doubling delay, both
//! derived inside the scheduler tick with no operating-system callbacks:
//!
//! - **Wake.** A gap of more than [`WAKE_GAP_MS`] between two ticks means the
//!   process was suspended. The probe delay resets, so a probe is due at once.
//! - **Network change.** While the gate is closed, each tick reads the
//!   machine's [`NetworkFingerprint`]. A change resets the probe delay.
//!
//! An empty fingerprint means the machine has no network, so neither trigger
//! fires while it is empty. It never blocks a probe: the doubling schedule
//! keeps running, because the provider may be local (a model server or a
//! gateway on this machine) and reachable with no network at all.
//!
//! A probe starts only on a tick whose preceding gap was normal. A short
//! maintenance wake during sleep cannot produce two ticks one interval apart,
//! so it never starts a probe; after the lid opens, the second tick does.
//!
//! Every method takes `now`; the gate never reads a clock of its own.

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;

use crate::store::{CompletionReason, Session, SessionStatus, Store};

use super::{is_network_pause, PauseAccounting, RateLimitScheduler};

mod fingerprint;

pub(crate) use fingerprint::{InterfaceAddresses, NetworkFingerprint};

/// The delay before the first probe after the gate closes, and the start of
/// the doubling for probes that fail.
pub(super) const BASE_PROBE_DELAY_MS: i64 = 15_000;
pub(super) const MAX_PROBE_DELAY_MS: i64 = 10 * 60_000;
/// A gap between two ticks longer than this means the process was suspended:
/// three of the scheduler's 15 second ticks.
pub(super) const WAKE_GAP_MS: i64 = 45_000;
/// How long a probe may run without reaching its provider before the gate
/// stops waiting for it, so one hung turn cannot hold the other rows.
///
/// It is not known to be longer than an agent's own request timeout: that
/// was never measured, and an agent that retries a request that timed out
/// takes longer still. So a probe may fail on the network after this timeout
/// has passed. Such a late failure is charged to the session as a failed
/// probe (see [`NetworkGate::record_failure`]).
pub(super) const PROBE_TIMEOUT_MS: i64 = 10 * 60_000;
/// How many times in a row one session's own probes may fail, each while the
/// machine stayed awake and held a network address, before the session is
/// given up on and left paused for a manual resume.
pub(super) const MAX_CONSECUTIVE_PROBE_FAILURES: u32 = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
enum GateState {
    Closed,
    /// `session_id` was claimed as the probe at `since`.
    Probing {
        session_id: String,
        since: i64,
        /// The process was suspended while the probe ran, so its failure
        /// says nothing about a network the machine was awake to use.
        slept: bool,
    },
    Open,
}

/// What the gate lets a due network-paused session do on this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NetworkTurn {
    /// Start nothing.
    Hold,
    /// Resume this session as the probe.
    Probe,
    /// Resume normally: the gate is open.
    Resume,
}

#[derive(Debug)]
pub(super) struct NetworkGate {
    state: GateState,
    /// When a closed gate may start its next probe.
    next_probe_at: i64,
    /// The wait that the next failed probe adds before the one after it.
    probe_delay_ms: i64,
    /// The wall-clock time of the last tick.
    last_tick_at: Option<i64>,
    /// Whether the gap before the last tick was normal. A probe starts only
    /// when it was; the first tick of a process has no gap and counts.
    settled: bool,
    /// The fingerprint last read while the gate was closed. `None` until the
    /// first read, after the gate opens, and when a read fails.
    addresses: Option<BTreeSet<IpAddr>>,
    /// Per session, how many of its own probes failed in a row while the
    /// machine was awake with a network address. Cleared when the gate opens.
    probe_failures: HashMap<String, u32>,
    /// The sessions whose probe timed out and whose turn has not been heard
    /// of since, each with whether the process was suspended during that
    /// turn. Cleared when the gate opens; a session leaves when it fails on
    /// the network or probes again.
    timed_out_probes: HashMap<String, bool>,
}

impl Default for NetworkGate {
    fn default() -> Self {
        Self {
            state: GateState::Closed,
            next_probe_at: 0,
            probe_delay_ms: BASE_PROBE_DELAY_MS,
            last_tick_at: None,
            settled: true,
            addresses: None,
            probe_failures: HashMap::new(),
            timed_out_probes: HashMap::new(),
        }
    }
}

impl NetworkGate {
    pub(super) fn turn(&self, now: i64) -> NetworkTurn {
        match self.state {
            GateState::Open => NetworkTurn::Resume,
            GateState::Probing { .. } => NetworkTurn::Hold,
            GateState::Closed if now >= self.next_probe_at && self.settled => NetworkTurn::Probe,
            GateState::Closed => NetworkTurn::Hold,
        }
    }

    /// Record a scheduler tick, and answer whether the gap before it shows
    /// that the process was suspended. The caller decides whether that
    /// resets the probe delay (see [`Self::apply_triggers`]).
    fn observe_tick(&mut self, now: i64) -> bool {
        let woke = self
            .last_tick_at
            .is_some_and(|last| now.saturating_sub(last) > WAKE_GAP_MS);
        self.last_tick_at = Some(now);
        self.settled = !woke;
        if woke {
            if let GateState::Probing { slept, .. } = &mut self.state {
                *slept = true;
            }
            for slept in self.timed_out_probes.values_mut() {
                *slept = true;
            }
        }
        woke
    }

    /// Something suggests the network may be back. A closed gate may probe
    /// at once, and the next failed probe waits the base delay again. A
    /// running probe is left to finish.
    fn reset_probe_delay(&mut self, now: i64) {
        self.probe_delay_ms = BASE_PROBE_DELAY_MS;
        if self.state == GateState::Closed {
            self.next_probe_at = now;
        }
    }

    /// Act on this tick's triggers: a wake, or a changed fingerprint. Either
    /// resets the probe delay, unless the fingerprint is empty: a machine
    /// with no network has nothing new to probe, so the doubling schedule
    /// stands. Answers whether the delay was reset.
    fn apply_triggers(&mut self, woke: bool, changed: bool, now: i64) -> bool {
        let fire = (woke || changed) && !self.network_absent();
        if fire {
            self.reset_probe_delay(now);
        }
        fire
    }

    fn is_closed(&self) -> bool {
        self.state == GateState::Closed
    }

    /// The last fingerprint read held no address: there is no network.
    fn network_absent(&self) -> bool {
        self.addresses.as_ref().is_some_and(BTreeSet::is_empty)
    }

    /// The last fingerprint read held an address. `false` when it held none
    /// and when nothing is known.
    fn network_present(&self) -> bool {
        self.addresses
            .as_ref()
            .is_some_and(|addresses| !addresses.is_empty())
    }

    /// Record the fingerprint read on this tick, and answer whether it
    /// differs from the one before. The first read is the baseline, not a
    /// change.
    fn observe_fingerprint(&mut self, addresses: BTreeSet<IpAddr>) -> bool {
        if !self.is_closed() {
            return false;
        }
        let changed = self
            .addresses
            .as_ref()
            .is_some_and(|previous| *previous != addresses);
        self.addresses = Some(addresses);
        changed
    }

    /// The fingerprint could not be read. Nothing is known about the
    /// network, so an earlier "no network" answer no longer holds back the
    /// triggers.
    fn fingerprint_unreadable(&mut self) {
        self.addresses = None;
    }

    fn probe_started(&mut self, session_id: &str, now: i64) {
        // A session that probes again was parked since its probe timed out,
        // so that turn is over.
        self.timed_out_probes.remove(session_id);
        self.state = GateState::Probing {
            session_id: session_id.to_string(),
            since: now,
            slept: false,
        };
    }

    /// The user resumed a network-paused session by hand. A closed gate
    /// watches that turn as its probe, so its answer opens the gate for the
    /// rest; a gate already probing or open has its answer coming.
    fn adopt_probe(&mut self, session_id: &str, now: i64) -> bool {
        let adopt = self.is_closed();
        if adopt {
            self.probe_started(session_id, now);
        }
        adopt
    }

    fn probe(&self) -> Option<(String, i64)> {
        match &self.state {
            GateState::Probing {
                session_id, since, ..
            } => Some((session_id.clone(), *since)),
            _ => None,
        }
    }

    fn is_probing(&self, session_id: &str) -> bool {
        matches!(&self.state, GateState::Probing { session_id: probe, .. } if probe == session_id)
    }

    /// The probe's turn reached its provider: the network is back.
    fn probe_succeeded(&mut self, session_id: &str) {
        if self.is_probing(session_id) {
            self.state = GateState::Open;
            self.probe_delay_ms = BASE_PROBE_DELAY_MS;
            self.probe_failures.clear();
            self.timed_out_probes.clear();
            // The fingerprint is not read while the gate is open, so the
            // next closed period starts from a fresh baseline.
            self.addresses = None;
        }
    }

    /// The probe ended without saying anything about the network: it was
    /// stopped, deleted, or failed for another reason. Another session may
    /// probe at once, and the delay is left alone.
    fn probe_abandoned(&mut self, session_id: &str, now: i64) {
        if self.is_probing(session_id) {
            self.state = GateState::Closed;
            self.next_probe_at = now;
        }
    }

    /// The probe has run for [`PROBE_TIMEOUT_MS`] without reaching its
    /// provider. The gate stops waiting for it: the schedule moves on as
    /// after a failed probe, and the session, still running, is left alone.
    /// It is remembered, so that a network failure it reports later is
    /// charged as the failed probe it is.
    fn probe_timed_out(&mut self, session_id: &str, now: i64) {
        if let GateState::Probing {
            session_id: probe,
            slept,
            ..
        } = &self.state
        {
            if probe == session_id {
                self.timed_out_probes.insert(session_id.to_string(), *slept);
                self.close_after_failed_probe(now);
            }
        }
    }

    /// Close the gate after a probe that did not get through: the next one
    /// waits the current delay, and the one after it twice that.
    fn close_after_failed_probe(&mut self, now: i64) {
        self.state = GateState::Closed;
        self.next_probe_at = now.saturating_add(self.probe_delay_ms);
        self.probe_delay_ms = self
            .probe_delay_ms
            .saturating_mul(2)
            .min(MAX_PROBE_DELAY_MS);
    }

    /// Record that `session_id` failed on the network, and answer how the
    /// failure is charged to that session's retry budget.
    ///
    /// A session whose probe timed out earlier fails here as that probe,
    /// late. Unless the gate has opened since, the failure is charged to the
    /// session's run of failed probes under the same conditions as a probe
    /// that failed in time. A closed gate also waits at least the probe
    /// delay from now: the session's row is due at once, and with the timeout
    /// long past it would otherwise probe again on the next tick. The delay
    /// is not doubled a second time, because the timeout already doubled it
    /// for this probe.
    fn record_failure(&mut self, session_id: &str, now: i64) -> PauseAccounting {
        if let Some(slept) = self.timed_out_probes.remove(session_id) {
            if self.state != GateState::Open {
                if self.is_closed() {
                    self.next_probe_at = self
                        .next_probe_at
                        .max(now.saturating_add(self.probe_delay_ms));
                }
                let charged = !slept && self.network_present();
                return if charged && self.probe_failures_exhausted(session_id) {
                    PauseAccounting::ProbesExhausted
                } else {
                    PauseAccounting::Waiting
                };
            }
        }
        match &self.state {
            GateState::Open => {
                self.state = GateState::Closed;
                self.next_probe_at = now.saturating_add(BASE_PROBE_DELAY_MS);
                self.probe_delay_ms = BASE_PROBE_DELAY_MS.saturating_mul(2);
                PauseAccounting::Counted
            }
            GateState::Probing {
                session_id: probe,
                slept,
                ..
            } if probe == session_id => {
                let awake = !*slept;
                self.close_after_failed_probe(now);
                if awake && self.network_present() && self.probe_failures_exhausted(session_id) {
                    PauseAccounting::ProbesExhausted
                } else {
                    PauseAccounting::Waiting
                }
            }
            // A session that was already running when the network dropped,
            // failing while the gate is closed or another session probes. It
            // says nothing new, so the probe schedule stands.
            GateState::Probing { .. } | GateState::Closed => PauseAccounting::Waiting,
        }
    }

    /// Count one more failed probe of `session_id` made with the machine
    /// awake and a network present, and answer whether that was its last.
    fn probe_failures_exhausted(&mut self, session_id: &str) -> bool {
        let failures = self
            .probe_failures
            .entry(session_id.to_string())
            .or_insert(0);
        *failures += 1;
        let exhausted = *failures >= MAX_CONSECUTIVE_PROBE_FAILURES;
        if exhausted {
            self.probe_failures.remove(session_id);
        }
        exhausted
    }
}

/// How a running probe stands, read from the store.
#[derive(Debug, PartialEq, Eq)]
enum ProbeVerdict {
    Running,
    Succeeded,
    Abandoned,
}

/// A probe succeeds when its turn shows, after the claim, that a request
/// reached the provider: a tool call or its result, a result the agent did
/// not mark as an error, or a prompt response (see
/// [`Store::session_reached_provider_since`]). Message text does not count:
/// a failed turn records its own error as text. A turn that completed did
/// get its prompt response, whether or not the row for it is in the store
/// yet.
fn probe_verdict(store: &Store, session_id: &str, since: i64) -> Result<ProbeVerdict, String> {
    if store
        .session_reached_provider_since(session_id, since)
        .map_err(|e| e.to_string())?
    {
        return Ok(ProbeVerdict::Succeeded);
    }
    let status = store
        .get_session(session_id)
        .map_err(|e| e.to_string())?
        .map(|session| session.status);
    Ok(match status {
        Some(SessionStatus::Running) => ProbeVerdict::Running,
        Some(SessionStatus::Completed) => ProbeVerdict::Succeeded,
        // Stopped, deleted, ended on another error, or parked by something
        // other than this process's pause path (which reports a network
        // failure to the gate before it writes the pause).
        _ => ProbeVerdict::Abandoned,
    })
}

impl RateLimitScheduler {
    /// What the gate lets a due network-paused session do on this tick.
    pub(super) fn network_turn(&self, now: i64) -> NetworkTurn {
        self.network_gate.lock().unwrap().turn(now)
    }

    /// Apply the wake and network-change triggers for the tick at `now`.
    ///
    /// Runs once per tick, before any row is admitted. The fingerprint is
    /// read only while the gate is closed and `network_rows_waiting`, so a
    /// machine with nothing paused on the network never lists its interfaces.
    pub(super) fn observe_network_tick(&self, now: i64, network_rows_waiting: bool) {
        let woke = {
            let mut gate = self.network_gate.lock().unwrap();
            let woke = gate.observe_tick(now);
            if !gate.is_closed() || !network_rows_waiting {
                // No fingerprint is read, so nothing says the network is
                // absent: a wake resets the delay.
                if woke && gate.apply_triggers(true, false, now) {
                    log::info!("[network_gate] woke from a suspend; probe delay reset");
                }
                return;
            }
            woke
        };
        // The interfaces are read with the gate unlocked; the gate rechecks
        // that it is still closed before it takes the answer.
        let addresses = self.network_fingerprint.read();
        let mut gate = self.network_gate.lock().unwrap();
        let changed = match addresses {
            Ok(addresses) => gate.observe_fingerprint(addresses),
            Err(e) => {
                log::warn!("[network_gate] failed to read network interfaces: {e}");
                gate.fingerprint_unreadable();
                false
            }
        };
        if gate.apply_triggers(woke, changed, now) {
            log::info!(
                "[network_gate] {}; probe delay reset",
                if woke {
                    "woke from a suspend"
                } else {
                    "network interfaces changed"
                }
            );
        } else if woke || changed {
            log::info!(
                "[network_gate] no network interface has an address; probe schedule unchanged"
            );
        }
    }

    /// The scheduler claimed `session_id` as the probe.
    pub(super) fn network_probe_started(&self, session_id: &str, now: i64) {
        log::info!("[network_gate] probing with session {session_id}");
        self.network_gate
            .lock()
            .unwrap()
            .probe_started(session_id, now);
    }

    /// The user resumed `session` by hand and the claim won. If it was paused
    /// on the network and the gate is closed, the gate watches the turn as
    /// its probe: without this a resume that gets through tells the gate
    /// nothing, and the other paused sessions wait out the probe delay.
    pub(crate) fn manual_resume_started(&self, session: &Session, now: i64) {
        let network_paused = session.status == SessionStatus::Paused
            && session.completion_reason == Some(CompletionReason::RateLimited)
            && is_network_pause(session.rate_limit_kind.as_deref());
        if network_paused
            && self
                .network_gate
                .lock()
                .unwrap()
                .adopt_probe(&session.id, now)
        {
            log::info!(
                "[network_gate] watching manually resumed session {} as the probe",
                session.id
            );
        }
    }

    /// The probe `session_id` ended without saying anything about the
    /// network: its resume failed to start. Another session may probe at
    /// once, and the probe delay is left alone.
    pub(super) fn abandon_network_probe(&self, session_id: &str, now: i64) {
        self.network_gate
            .lock()
            .unwrap()
            .probe_abandoned(session_id, now);
    }

    /// Move the gate out of `Probing` if the store shows how the probe went,
    /// or if it has run past [`PROBE_TIMEOUT_MS`].
    ///
    /// Runs at the start of each tick, and before a network failure is
    /// recorded so that a probe which reached its provider and then failed
    /// is counted as a failure with the gate open.
    pub(super) fn settle_network_probe(&self, store: &Store, now: i64) {
        self.settle_probe(store, None, now);
    }

    /// [`Self::settle_network_probe`], with the session whose network failure
    /// is about to be recorded. If that session is the probe, its failure is
    /// what settles it: a row that no longer runs is not an abandoned probe,
    /// and one that ran long is not timed out.
    fn settle_probe(&self, store: &Store, failing: Option<&str>, now: i64) {
        // The store is read with the gate unlocked; the gate rechecks that it
        // is still probing this session before it moves.
        let Some((session_id, since)) = self.network_gate.lock().unwrap().probe() else {
            return;
        };
        match probe_verdict(store, &session_id, since) {
            Ok(ProbeVerdict::Running | ProbeVerdict::Abandoned)
                if failing == Some(session_id.as_str()) => {}
            Ok(ProbeVerdict::Running) if now.saturating_sub(since) >= PROBE_TIMEOUT_MS => {
                log::warn!(
                    "[network_gate] probe session {session_id} has not reached its provider \
                     after {} s; no longer waiting for it",
                    PROBE_TIMEOUT_MS / 1_000
                );
                self.network_gate
                    .lock()
                    .unwrap()
                    .probe_timed_out(&session_id, now);
            }
            Ok(ProbeVerdict::Running) => {}
            Ok(ProbeVerdict::Succeeded) => {
                log::info!("[network_gate] probe session {session_id} got through; gate open");
                self.network_gate
                    .lock()
                    .unwrap()
                    .probe_succeeded(&session_id);
            }
            Ok(ProbeVerdict::Abandoned) => {
                log::info!("[network_gate] probe session {session_id} ended without an answer");
                self.network_gate
                    .lock()
                    .unwrap()
                    .probe_abandoned(&session_id, now);
            }
            Err(e) => {
                log::warn!("[network_gate] failed to read probe session {session_id}: {e}");
            }
        }
    }

    /// Record that `session_id` failed on the network, and answer how the
    /// failure is charged to that session's retry budget: counted only when
    /// the gate was open, and the end of the session's automatic resumes when
    /// its own probes have failed [`MAX_CONSECUTIVE_PROBE_FAILURES`] times.
    pub(super) fn record_network_failure(
        &self,
        store: &Store,
        session_id: &str,
        now: i64,
    ) -> PauseAccounting {
        self.settle_probe(store, Some(session_id), now);
        let accounting = self
            .network_gate
            .lock()
            .unwrap()
            .record_failure(session_id, now);
        log::info!(
            "[network_gate] session {session_id} failed on the network ({accounting:?}); gate closed"
        );
        accounting
    }
}

#[cfg(test)]
mod tests;
