//! Phase-by-phase timing of ACP session setup, so a setup timeout says what
//! it was waiting for.
//!
//! Setup is one budget ([`super::ACP_SETUP_TIMEOUT`]) over several agent
//! round trips and synchronous store writes, all on the session's own
//! thread. When the budget runs out, the error alone used to say nothing
//! about which step was slow, whether the agent or the store was, or whether
//! the thread had even been running. [`SetupTimeline`] records when each step
//! began and finished, logs both, and on a timeout renders the stalled step,
//! the completed ones, how late the timer fired after its deadline (a direct
//! measure of the thread not being scheduled), and the thread's scheduling
//! state at setup start and at the timeout.

use std::cell::RefCell;
use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

use super::sched_state::{self, SchedState};

/// One measured step of session setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SetupStep {
    /// The `initialize` round trip.
    Initialize,
    /// Storing the `initialize` answer.
    RecordInitialize,
    /// Naming the session, which stores the frames held until then.
    NameSession,
    /// The `session/new` round trip (with any authentication retry).
    NewSession,
    /// The `session/load` round trip (with any authentication retry).
    LoadSession,
    /// Storing the agent's session id.
    SaveAgentSessionId,
    /// Storing the session's mode state.
    RecordModes,
    /// The `session/set_mode` round trip for an unattended mode.
    SetMode,
    /// `session/set_config_option` round trips, and storing their result.
    ConfigOptions,
}

impl SetupStep {
    fn label(self) -> &'static str {
        match self {
            SetupStep::Initialize => "initialize",
            SetupStep::RecordInitialize => "storing initialize",
            SetupStep::NameSession => "storing held frames",
            SetupStep::NewSession => "session/new",
            SetupStep::LoadSession => "session/load",
            SetupStep::SaveAgentSessionId => "storing the agent session id",
            SetupStep::RecordModes => "storing session modes",
            SetupStep::SetMode => "session/set_mode",
            SetupStep::ConfigOptions => "session config options",
        }
    }

    /// Whether the step is an agent round trip rather than local work, which
    /// decides the verb its finish is logged with.
    fn is_agent_request(self) -> bool {
        matches!(
            self,
            SetupStep::Initialize
                | SetupStep::NewSession
                | SetupStep::LoadSession
                | SetupStep::SetMode
                | SetupStep::ConfigOptions
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StepRecord {
    step: SetupStep,
    /// Offset from setup start.
    began: Duration,
    /// `None` while the step is still running, and forever if setup was
    /// abandoned during it.
    took: Option<Duration>,
}

/// The timeline of one setup attempt. Lives on the session thread beside the
/// setup future; steps are recorded through `&self`.
pub(super) struct SetupTimeline {
    session_id: String,
    budget: Duration,
    started: Instant,
    deadline: Instant,
    started_sched: SchedState,
    steps: RefCell<Vec<StepRecord>>,
}

impl SetupTimeline {
    /// Start the clock for a setup with `budget`, and log the thread and its
    /// scheduling state.
    pub(super) fn start(session_id: &str, budget: Duration) -> Self {
        let started = Instant::now();
        let started_sched = sched_state::current();
        let thread = std::thread::current();
        log::info!(
            "ACP setup {session_id}: started on thread {} ({:?}), budget {}s; scheduling: {started_sched}",
            thread.name().unwrap_or("unnamed"),
            thread.id(),
            budget.as_secs(),
        );
        Self {
            session_id: session_id.to_string(),
            budget,
            started,
            deadline: started + budget,
            started_sched,
            steps: RefCell::new(Vec::new()),
        }
    }

    /// When the setup budget runs out.
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Run `future` as `step`, logging and recording its start and finish. A
    /// future dropped before it finishes stays recorded as in progress.
    pub(super) async fn step<F: Future>(&self, step: SetupStep, future: F) -> F::Output {
        let index = self.begin(step);
        let output = future.await;
        self.finish(index);
        output
    }

    fn begin(&self, step: SetupStep) -> usize {
        let began = self.started.elapsed();
        log::info!(
            "ACP setup {}: {} started at +{}",
            self.session_id,
            step.label(),
            secs(began)
        );
        let mut steps = self.steps.borrow_mut();
        steps.push(StepRecord {
            step,
            began,
            took: None,
        });
        steps.len() - 1
    }

    fn finish(&self, index: usize) {
        let now = self.started.elapsed();
        let mut steps = self.steps.borrow_mut();
        let record = &mut steps[index];
        let took = now.saturating_sub(record.began);
        record.took = Some(took);
        let verb = if record.step.is_agent_request() {
            "answered in"
        } else {
            "took"
        };
        log::info!(
            "ACP setup {}: {} {verb} {} (at +{})",
            self.session_id,
            record.step.label(),
            secs(took),
            secs(now)
        );
    }

    /// Log a completed setup with each step's time.
    pub(super) fn completed(&self) {
        log::info!(
            "ACP setup {}: completed in {} ({})",
            self.session_id,
            secs(self.started.elapsed()),
            completed_steps(&self.steps.borrow()).unwrap_or_else(|| "no steps".to_string())
        );
    }

    /// Log a setup that failed with `error` before the budget ran out.
    pub(super) fn failed(&self, error: &str) {
        log::warn!(
            "ACP setup {}: failed after {}: {error} ({})",
            self.session_id,
            secs(self.started.elapsed()),
            stall_details(&self.steps.borrow(), self.started.elapsed())
        );
    }

    /// The setup timer fired: snapshot the scheduling state, log, and return
    /// the error, `headline` followed by what setup was doing.
    pub(super) fn timed_out(&self, headline: &str) -> String {
        let fired = Instant::now();
        let message = describe_timeout(
            headline,
            &self.steps.borrow(),
            fired.saturating_duration_since(self.started),
            fired.saturating_duration_since(self.deadline),
            &self.started_sched,
            &sched_state::current(),
        );
        log::warn!(
            "ACP setup {} (budget {}s): {message}",
            self.session_id,
            self.budget.as_secs()
        );
        message
    }
}

/// Run `future` as `step` of `timeline`, or untimed without one.
pub(super) async fn timed<F: Future>(
    timeline: Option<&SetupTimeline>,
    step: SetupStep,
    future: F,
) -> F::Output {
    match timeline {
        Some(timeline) => timeline.step(step, future).await,
        None => future.await,
    }
}

/// `"{headline}: waiting for session/new for 88.2s; completed: initialize
/// 1.30s, …; timer fired 4.0s late; scheduling at start: …; at timeout: …"`.
fn describe_timeout(
    headline: &str,
    steps: &[StepRecord],
    elapsed: Duration,
    lateness: Duration,
    start_sched: &SchedState,
    timeout_sched: &SchedState,
) -> String {
    format!(
        "{headline}: {}; timer fired {} late; scheduling at start: {start_sched}; at timeout: {timeout_sched}",
        stall_details(steps, elapsed),
        secs(lateness),
    )
}

/// The step in progress at `elapsed`, then the completed ones.
fn stall_details(steps: &[StepRecord], elapsed: Duration) -> String {
    let stalled = match steps.iter().rev().find(|record| record.took.is_none()) {
        Some(record) => format!(
            "waiting for {} for {}",
            record.step.label(),
            secs(elapsed.saturating_sub(record.began))
        ),
        None => match steps.last() {
            Some(last) => {
                let ended = last.began + last.took.unwrap_or_default();
                format!(
                    "between setup steps for {} after {}",
                    secs(elapsed.saturating_sub(ended)),
                    last.step.label()
                )
            }
            None => "before any setup step".to_string(),
        },
    };
    match completed_steps(steps) {
        Some(completed) => format!("{stalled}; completed: {completed}"),
        None => format!("{stalled}; nothing completed"),
    }
}

fn completed_steps(steps: &[StepRecord]) -> Option<String> {
    let completed: Vec<String> = steps
        .iter()
        .filter_map(|record| {
            record
                .took
                .map(|took| format!("{} {}", record.step.label(), secs(took)))
        })
        .collect();
    (!completed.is_empty()).then(|| completed.join(", "))
}

/// Seconds, with two decimals below ten seconds so sub-second store calls
/// and small timer lateness stay readable.
fn secs(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs < 10.0 {
        format!("{secs:.2}s")
    } else {
        format!("{secs:.1}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(step: SetupStep, began: f64, took: f64) -> StepRecord {
        StepRecord {
            step,
            began: Duration::from_secs_f64(began),
            took: Some(Duration::from_secs_f64(took)),
        }
    }

    fn running(step: SetupStep, began: f64) -> StepRecord {
        StepRecord {
            step,
            began: Duration::from_secs_f64(began),
            took: None,
        }
    }

    fn sched(priority: i32, background: bool) -> SchedState {
        SchedState {
            process_background: Some(background),
            thread_priority: Some(priority),
            thread_base_priority: Some(31),
            ..SchedState::default()
        }
    }

    #[test]
    fn a_timeout_names_the_stalled_step_completed_steps_and_lateness() {
        let steps = [
            done(SetupStep::Initialize, 0.0, 1.3),
            done(SetupStep::RecordInitialize, 1.3, 0.01),
            running(SetupStep::NewSession, 1.31),
        ];
        let message = describe_timeout(
            "Timed out waiting for ACP protocol startup after 90s",
            &steps,
            Duration::from_secs_f64(94.0),
            Duration::from_secs(4),
            &sched(31, false),
            &sched(4, true),
        );
        assert_eq!(
            message,
            "Timed out waiting for ACP protocol startup after 90s: waiting for session/new \
             for 92.7s; completed: initialize 1.30s, storing initialize 0.01s; timer fired \
             4.00s late; scheduling at start: pri 31/31; at timeout: pri 4/31, darwin-bg process"
        );
    }

    #[test]
    fn a_timeout_between_steps_says_which_step_finished_last() {
        let steps = [
            done(SetupStep::Initialize, 0.0, 0.5),
            done(SetupStep::SaveAgentSessionId, 1.0, 88.0),
        ];
        assert_eq!(
            stall_details(&steps, Duration::from_secs(90)),
            "between setup steps for 1.00s after storing the agent session id; completed: \
             initialize 0.50s, storing the agent session id 88.0s"
        );
    }

    #[test]
    fn a_timeout_before_any_step() {
        assert_eq!(
            stall_details(&[], Duration::from_secs(90)),
            "before any setup step; nothing completed"
        );
    }

    #[test]
    fn the_timeout_text_keeps_its_headline_and_reads_as_no_transport_loss() {
        let message = describe_timeout(
            "Timed out waiting for ACP protocol startup after 90s",
            &[running(SetupStep::Initialize, 0.0)],
            Duration::from_secs(90),
            Duration::ZERO,
            &SchedState::default(),
            &SchedState::default(),
        );
        assert!(message.starts_with("Timed out waiting for ACP protocol startup after 90s: "));
        assert!(!super::super::is_transport_loss_error(&message));
    }

    #[tokio::test(start_paused = true)]
    async fn steps_are_recorded_and_an_abandoned_step_stays_in_progress() {
        let timeline = SetupTimeline::start("s1", Duration::from_secs(90));
        timeline
            .step(
                SetupStep::Initialize,
                tokio::time::sleep(Duration::from_millis(1300)),
            )
            .await;
        let stalled = timeline.step(SetupStep::NewSession, std::future::pending::<()>());
        let result = tokio::time::timeout_at(timeline.deadline(), stalled).await;
        assert!(result.is_err());

        let steps = timeline.steps.borrow().clone();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].step, SetupStep::Initialize);
        assert_eq!(steps[0].took, Some(Duration::from_millis(1300)));
        assert_eq!(steps[1].step, SetupStep::NewSession);
        assert_eq!(steps[1].began, Duration::from_millis(1300));
        assert_eq!(steps[1].took, None);

        let message = timeline.timed_out("Timed out");
        assert!(
            message.starts_with(
                "Timed out: waiting for session/new for 88.7s; completed: initialize 1.30s; \
                 timer fired 0.00s late; scheduling at start: "
            ),
            "{message}"
        );
    }

    #[test]
    fn durations_keep_precision_below_ten_seconds() {
        assert_eq!(secs(Duration::from_millis(4)), "0.00s");
        assert_eq!(secs(Duration::from_millis(1404)), "1.40s");
        assert_eq!(secs(Duration::from_millis(88_240)), "88.2s");
    }
}
