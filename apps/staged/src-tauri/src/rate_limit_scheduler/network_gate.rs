//! The shared gate that every network-paused session waits behind.
//!
//! A session paused with kind [`NETWORK_PAUSE_KIND`](super::NETWORK_PAUSE_KIND)
//! failed because the agent could not reach its model provider. That is a
//! fact about the machine, not about the session, so all such sessions share
//! one gate instead of each running its own backoff. API-limit pauses never
//! pass through it.
//!
//! | State   | Meaning                         | What the scheduler does          |
//! | ------- | ------------------------------- | -------------------------------- |
//! | Closed  | The network is presumed down    | Starts one probe when it is due  |
//! | Probing | One probe turn is running       | Starts no other network resume   |
//! | Open    | A probe produced agent output   | Resumes the rest, two per tick   |
//!
//! The gate lives in memory, owned by [`RateLimitScheduler`] behind a mutex,
//! and starts closed with a probe due at once, so a relaunch probes on its
//! first tick.
//!
//! Failures while the gate is not open are not counted: the session's
//! `attempt` does not advance and the 24 hour window is not applied, so a
//! laptop that sleeps for days gives up on nothing. Failures while it is open
//! are counted under the existing cap, and close it again.
//!
//! Every method takes `now`; the gate never reads a clock of its own.

use crate::store::{SessionStatus, Store};

use super::RateLimitScheduler;

/// The delay before the first probe after the gate closes, and the start of
/// the doubling for probes that fail.
pub(super) const BASE_PROBE_DELAY_MS: i64 = 15_000;
pub(super) const MAX_PROBE_DELAY_MS: i64 = 10 * 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
enum GateState {
    Closed,
    /// `session_id` was claimed as the probe at `since`.
    Probing {
        session_id: String,
        since: i64,
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
}

impl Default for NetworkGate {
    fn default() -> Self {
        Self {
            state: GateState::Closed,
            next_probe_at: 0,
            probe_delay_ms: BASE_PROBE_DELAY_MS,
        }
    }
}

impl NetworkGate {
    pub(super) fn turn(&self, now: i64) -> NetworkTurn {
        match self.state {
            GateState::Open => NetworkTurn::Resume,
            GateState::Probing { .. } => NetworkTurn::Hold,
            GateState::Closed if now >= self.next_probe_at => NetworkTurn::Probe,
            GateState::Closed => NetworkTurn::Hold,
        }
    }

    fn probe_started(&mut self, session_id: &str, now: i64) {
        self.state = GateState::Probing {
            session_id: session_id.to_string(),
            since: now,
        };
    }

    fn probe(&self) -> Option<(String, i64)> {
        match &self.state {
            GateState::Probing { session_id, since } => Some((session_id.clone(), *since)),
            _ => None,
        }
    }

    fn is_probing(&self, session_id: &str) -> bool {
        matches!(&self.state, GateState::Probing { session_id: probe, .. } if probe == session_id)
    }

    /// The probe's turn produced agent output: the network is back.
    fn probe_succeeded(&mut self, session_id: &str) {
        if self.is_probing(session_id) {
            self.state = GateState::Open;
            self.probe_delay_ms = BASE_PROBE_DELAY_MS;
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

    /// Record that `session_id` failed on the network, and answer whether the
    /// failure counts against that session's retry budget.
    fn record_failure(&mut self, session_id: &str, now: i64) -> bool {
        match &self.state {
            GateState::Open => {
                self.state = GateState::Closed;
                self.next_probe_at = now.saturating_add(BASE_PROBE_DELAY_MS);
                self.probe_delay_ms = BASE_PROBE_DELAY_MS.saturating_mul(2);
                true
            }
            GateState::Probing {
                session_id: probe, ..
            } if probe == session_id => {
                self.state = GateState::Closed;
                self.next_probe_at = now.saturating_add(self.probe_delay_ms);
                self.probe_delay_ms = self
                    .probe_delay_ms
                    .saturating_mul(2)
                    .min(MAX_PROBE_DELAY_MS);
                false
            }
            // A session that was already running when the network dropped,
            // failing while the gate is closed or another session probes. It
            // says nothing new, so the probe schedule stands.
            GateState::Probing { .. } | GateState::Closed => false,
        }
    }
}

/// How a running probe stands, read from the store.
#[derive(Debug, PartialEq, Eq)]
enum ProbeVerdict {
    Running,
    Succeeded,
    Abandoned,
}

/// A probe succeeds when its turn records any agent message, tool call or
/// prompt response after the claim. A turn that completed did get its prompt
/// response, whether or not the row for it is in the store yet.
fn probe_verdict(store: &Store, session_id: &str, since: i64) -> Result<ProbeVerdict, String> {
    if store
        .session_has_agent_output_since(session_id, since)
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

    /// The scheduler claimed `session_id` as the probe.
    pub(super) fn network_probe_started(&self, session_id: &str, now: i64) {
        log::info!("[network_gate] probing with session {session_id}");
        self.network_gate
            .lock()
            .unwrap()
            .probe_started(session_id, now);
    }

    /// Move the gate out of `Probing` if the store shows how the probe went.
    ///
    /// Runs at the start of each tick, and before a network failure is
    /// recorded so that a probe which produced output and then failed is
    /// counted as a failure with the gate open.
    pub(super) fn settle_network_probe(&self, store: &Store, now: i64) {
        self.settle_probe(store, None, now);
    }

    /// [`Self::settle_network_probe`], with the session whose network failure
    /// is about to be recorded. If that session is the probe, a row that no
    /// longer runs is its failure, not an abandoned probe: a probe that fails
    /// to start is parked again before the gate hears of it.
    fn settle_probe(&self, store: &Store, failing: Option<&str>, now: i64) {
        // The store is read with the gate unlocked; the gate rechecks that it
        // is still probing this session before it moves.
        let Some((session_id, since)) = self.network_gate.lock().unwrap().probe() else {
            return;
        };
        match probe_verdict(store, &session_id, since) {
            Ok(ProbeVerdict::Running) => {}
            Ok(ProbeVerdict::Succeeded) => {
                log::info!("[network_gate] probe session {session_id} got through; gate open");
                self.network_gate
                    .lock()
                    .unwrap()
                    .probe_succeeded(&session_id);
            }
            Ok(ProbeVerdict::Abandoned) if failing == Some(session_id.as_str()) => {}
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

    /// Record that `session_id` failed on the network, and answer whether the
    /// failure counts against that session's retry budget: it does only when
    /// the gate was open.
    pub(super) fn record_network_failure(&self, store: &Store, session_id: &str, now: i64) -> bool {
        self.settle_probe(store, Some(session_id), now);
        let counted = self
            .network_gate
            .lock()
            .unwrap()
            .record_failure(session_id, now);
        log::info!(
            "[network_gate] session {session_id} failed on the network ({}); gate closed",
            if counted { "counted" } else { "not counted" }
        );
        counted
    }
}

#[cfg(test)]
mod tests;
