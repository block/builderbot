//! Inactivity watchdog for remote note sessions.
//!
//! A remote note session talks to its agent through the `sq blox acp` proxy.
//! When the network drops, the proxy process does not always die: it can
//! stay alive and blocked, so the transport never closes, no error reaches
//! the session loop, and the row sits in `running` while the agent on the
//! workstation finishes its work and writes its note to the outbox — a note
//! nothing on this side is going to look for.
//!
//! The watchdog is that look. Once the agent has been silent (no event
//! reached the [`crate::agent::MessageWriter`]) for a configurable interval,
//! it runs the same cheap `ws exec` outbox probe the per-turn collector
//! uses. A `done` marker for this turn pulls the note into the slot and
//! resolves the watchdog, and the run treats that like a connection it lost
//! after the note was delivered: the turn completes from the outbox. Anything
//! else keeps waiting and probes again after another silent interval — a
//! long-running agent that is merely quiet is left alone.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use crate::agent::ActivityClock;
use crate::note_tool::NoteSubmissionSlot;
use crate::remote_note_outbox::{self, OutboxState};

/// How long the agent must be silent before the outbox is probed, unless
/// [`INACTIVITY_PROBE_ENV`] says otherwise. Long enough that a slow tool
/// call or a model thinking through a big diff does not trigger it; short
/// enough that a finished note behind a hung proxy is picked up within the
/// detached row's polling horizon.
pub(crate) const DEFAULT_INACTIVITY_PROBE: Duration = Duration::from_secs(10 * 60);

/// Environment variable overriding [`DEFAULT_INACTIVITY_PROBE`], in whole
/// seconds. For manual testing of the hung-proxy path.
pub(crate) const INACTIVITY_PROBE_ENV: &str = "STAGED_REMOTE_NOTE_INACTIVITY_PROBE_SECS";

/// The configured inactivity interval.
pub(crate) fn inactivity_probe_interval() -> Duration {
    inactivity_probe_interval_from(std::env::var(INACTIVITY_PROBE_ENV).ok().as_deref())
}

/// [`inactivity_probe_interval`] for a given value of the variable: a
/// positive whole number of seconds, else the default (and a warning for a
/// value that is set but unusable).
pub(crate) fn inactivity_probe_interval_from(value: Option<&str>) -> Duration {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return DEFAULT_INACTIVITY_PROBE;
    };
    match value.parse::<u64>() {
        Ok(secs) if secs > 0 => Duration::from_secs(secs),
        _ => {
            log::warn!(
                "{INACTIVITY_PROBE_ENV}={value:?} is not a positive number of seconds; using \
                 the default of {}s",
                DEFAULT_INACTIVITY_PROBE.as_secs()
            );
            DEFAULT_INACTIVITY_PROBE
        }
    }
}

/// One outbox probe: the per-turn collector against the session's
/// workstation, or a test double.
type Probe = dyn Fn() -> Result<OutboxState, String> + Send + Sync;

/// Watches a remote note session's turn for a finished note the connection
/// is no longer reporting.
pub(crate) struct RemoteNoteWatchdog {
    session_id: String,
    activity: Arc<ActivityClock>,
    interval: Duration,
    probe: Arc<Probe>,
}

impl RemoteNoteWatchdog {
    /// A watchdog over `session_id`'s turn `turn_id` on `workspace_name`,
    /// probing the outbox into `slot` with the per-turn collector's probe
    /// variant: a `done` identical to the note this run already collected
    /// (`memory`, filled by the per-turn collector) is the previous turn's
    /// and reads as `Pending`, so a repair or follow-up turn is never ended
    /// on a note that was already pulled and rejected.
    pub(crate) fn new(
        session_id: String,
        workspace_name: String,
        turn_id: String,
        slot: NoteSubmissionSlot,
        memory: remote_note_outbox::CollectedNoteMemory,
        activity: Arc<ActivityClock>,
        interval: Duration,
    ) -> Self {
        let probe_session_id = session_id.clone();
        Self::with_probe(
            session_id,
            activity,
            interval,
            Arc::new(move || {
                let exec = remote_note_outbox::workspace_exec(&workspace_name);
                remote_note_outbox::probe_turn_into_slot(
                    &exec,
                    &probe_session_id,
                    &turn_id,
                    &slot,
                    &memory,
                )
            }),
        )
    }

    pub(crate) fn with_probe(
        session_id: String,
        activity: Arc<ActivityClock>,
        interval: Duration,
        probe: Arc<Probe>,
    ) -> Self {
        Self {
            session_id,
            activity,
            interval,
            probe,
        }
    }

    /// Resolves once a probe finds this turn's finished note in the outbox
    /// (by then pulled into the slot), with how long the agent had been
    /// silent when it did. Never resolves otherwise: the caller races it
    /// against the turn, and drops it when the turn ends on its own.
    ///
    /// A probe runs when the agent has been silent for the interval *and*
    /// the previous probe is at least an interval old, so a silent agent
    /// costs one `ws exec` round trip per interval, not one per wake-up.
    pub(crate) async fn finished_note_found(&self) -> Duration {
        let mut last_probe: Option<Instant> = None;
        loop {
            let idle = self.activity.idle_for();
            let until_idle_enough = self.interval.saturating_sub(idle);
            let until_probe_due = last_probe
                .map(|at| self.interval.saturating_sub(at.elapsed()))
                .unwrap_or_default();
            let wait = until_idle_enough.max(until_probe_due);
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
                continue;
            }
            last_probe = Some(Instant::now());
            log::info!(
                "Session {}: no activity from the remote agent for {}s; checking the \
                 workstation outbox for its note",
                self.session_id,
                idle.as_secs()
            );
            let probe = Arc::clone(&self.probe);
            let found = tokio::task::spawn_blocking(move || probe())
                .await
                .unwrap_or_else(|e| Err(format!("outbox probe task failed: {e}")));
            match found {
                Ok(OutboxState::Delivered(submission)) => {
                    log::warn!(
                        "Session {}: the remote agent's note \"{}\" is finished on the \
                         workstation but the connection reported nothing for {}s; treating \
                         the connection as lost and completing from the outbox",
                        self.session_id,
                        submission.title,
                        idle.as_secs()
                    );
                    return idle;
                }
                Ok(state) => log::info!(
                    "Session {}: the workstation outbox has no finished note for this turn \
                     ({state:?}); the agent may still be working, waiting on",
                    self.session_id
                ),
                Err(e) => log::warn!(
                    "Session {}: could not probe the workstation outbox after {}s of \
                     inactivity ({e}); waiting on",
                    self.session_id,
                    idle.as_secs()
                ),
            }
        }
    }
}

/// The detail the detached-row message carries when the inactivity probe,
/// not a transport error, is what ended the turn.
pub(crate) fn inactivity_detail(idle: Duration) -> String {
    let minutes = idle.as_secs().div_ceil(60).max(1);
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    format!(
        "the agent sent nothing for {minutes} {unit} while its finished note was waiting on \
         the workstation"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note_tool::NoteSubmission;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_probe_interval_comes_from_the_environment_when_it_is_usable() {
        assert_eq!(
            inactivity_probe_interval_from(None),
            DEFAULT_INACTIVITY_PROBE
        );
        assert_eq!(
            inactivity_probe_interval_from(Some("45")),
            Duration::from_secs(45)
        );
        assert_eq!(
            inactivity_probe_interval_from(Some(" 5 ")),
            Duration::from_secs(5)
        );
        for unusable in ["", "0", "-3", "ten", "1.5"] {
            assert_eq!(
                inactivity_probe_interval_from(Some(unusable)),
                DEFAULT_INACTIVITY_PROBE,
                "{unusable:?}"
            );
        }
    }

    fn delivered() -> OutboxState {
        OutboxState::Delivered(NoteSubmission {
            title: "Plan".into(),
            body: "Body".into(),
            suggested_next_steps: Default::default(),
            source: crate::note_tool::NoteSubmissionSource::Outbox,
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_agent_is_probed_once_per_interval_until_the_note_appears() {
        let probes = Arc::new(AtomicUsize::new(0));
        let probes_for_probe = Arc::clone(&probes);
        let activity = Arc::new(ActivityClock::new());
        let watchdog = RemoteNoteWatchdog::with_probe(
            "s1".into(),
            Arc::clone(&activity),
            Duration::from_secs(60),
            Arc::new(move || {
                // Pending twice, then unreachable once, then done.
                match probes_for_probe.fetch_add(1, Ordering::SeqCst) {
                    0 | 1 => Ok(OutboxState::Pending),
                    2 => Err("ws exec: connection refused".into()),
                    _ => Ok(delivered()),
                }
            }),
        );

        let found =
            tokio::time::timeout(Duration::from_secs(60 * 10), watchdog.finished_note_found())
                .await
                .expect("the fourth probe finds the note");
        assert_eq!(probes.load(Ordering::SeqCst), 4);
        assert!(found >= Duration::from_secs(60), "{found:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_active_agent_is_never_probed() {
        let probes = Arc::new(AtomicUsize::new(0));
        let probes_for_probe = Arc::clone(&probes);
        let activity = Arc::new(ActivityClock::new());
        let watchdog = RemoteNoteWatchdog::with_probe(
            "s1".into(),
            Arc::clone(&activity),
            Duration::from_secs(60),
            Arc::new(move || {
                probes_for_probe.fetch_add(1, Ordering::SeqCst);
                Ok(delivered())
            }),
        );
        let keep_talking = async {
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_secs(30)).await;
                activity.touch();
            }
        };
        tokio::select! {
            () = keep_talking => {}
            _ = watchdog.finished_note_found() => panic!("the agent was never silent long enough"),
        }
        assert_eq!(probes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_inactivity_detail_rounds_up_to_whole_minutes() {
        assert!(inactivity_detail(Duration::from_secs(601)).contains("11 minutes"));
        assert!(inactivity_detail(Duration::from_secs(600)).contains("10 minutes"));
        assert!(inactivity_detail(Duration::from_secs(5)).contains("1 minute while"));
    }
}
