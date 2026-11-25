//! Phase-by-phase timing of ACP session setup, with a deadline per step, so a
//! slow step does not cost a later one its time and a setup timeout says what
//! it was waiting for.
//!
//! Setup is several agent round trips (`initialize`, `session/new` or
//! `session/load`, `session/set_mode`, one `session/set_config_option` per
//! selection) and synchronous store writes, all on the session's own thread.
//! Each step gets its own deadline ([`super::ACP_SETUP_STEP_TIMEOUT`]) rather
//! than one budget being shared by all of them: under that shared budget a
//! `session/new` answered late but successfully was thrown away when the
//! budget ran out at the next step, so a start that had succeeded still
//! failed. Only a step that itself stalls times out now.
//!
//! [`SetupTimeline`] records when each step began and finished, logs both,
//! and on a step's timeout renders the stalled step, the completed ones, how
//! long setup had run, how late the timer fired after the step's deadline (a
//! direct measure of the thread not being scheduled), and the thread's
//! scheduling state at setup start and at the timeout.

use std::cell::{Cell, RefCell};
use std::fmt;
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
    /// Storing the agent's session id: the last step of a fresh session's
    /// setup, so a setup that fails leaves none stored to resume.
    SaveAgentSessionId,
    /// Storing the session's mode state.
    RecordModes,
    /// The `session/set_mode` round trip for an unattended mode.
    SetMode,
    /// One `session/set_config_option` round trip, for the selection labelled
    /// (`"model"`, `"effort"`, …).
    SetConfigOption(&'static str),
    /// Storing the session's config options, and any fallback notice.
    RecordConfigOptions,
}

impl SetupStep {
    /// Whether the step is an agent round trip rather than local work, which
    /// decides the verb its finish and its timeout are reported with.
    fn is_agent_request(self) -> bool {
        matches!(
            self,
            SetupStep::Initialize
                | SetupStep::NewSession
                | SetupStep::LoadSession
                | SetupStep::SetMode
                | SetupStep::SetConfigOption(_)
        )
    }
}

impl fmt::Display for SetupStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetupStep::Initialize => f.write_str("initialize"),
            SetupStep::RecordInitialize => f.write_str("storing initialize"),
            SetupStep::NameSession => f.write_str("storing held frames"),
            SetupStep::NewSession => f.write_str("session/new"),
            SetupStep::LoadSession => f.write_str("session/load"),
            SetupStep::SaveAgentSessionId => f.write_str("storing the agent session id"),
            SetupStep::RecordModes => f.write_str("storing session modes"),
            SetupStep::SetMode => f.write_str("session/set_mode"),
            SetupStep::SetConfigOption(label) => write!(f, "session/set_config_option ({label})"),
            SetupStep::RecordConfigOptions => f.write_str("storing config options"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StepRecord {
    step: SetupStep,
    /// Offset from setup start.
    began: Duration,
    /// `None` while the step is still running, and forever if setup was
    /// abandoned during it or it timed out.
    took: Option<Duration>,
}

/// The timeline of one setup attempt. Lives on the session thread beside the
/// setup future; steps are recorded through `&self`.
pub(super) struct SetupTimeline {
    session_id: String,
    /// What a step's timeout error starts with, e.g. "Timed out waiting for
    /// ACP protocol startup".
    headline: String,
    /// Each step's own deadline, counted from when the step begins.
    step_budget: Duration,
    started: Instant,
    started_sched: SchedState,
    steps: RefCell<Vec<StepRecord>>,
    /// Set once a step has timed out and logged it, so [`Self::failed`] does
    /// not log the same failure again.
    timed_out: Cell<bool>,
}

impl SetupTimeline {
    /// Start timing a setup whose steps each get `step_budget`, and log the
    /// thread and its scheduling state. A step's timeout error starts with
    /// `headline`.
    pub(super) fn start(session_id: &str, headline: &str, step_budget: Duration) -> Self {
        let started = Instant::now();
        let started_sched = sched_state::current();
        let thread = std::thread::current();
        log::info!(
            "ACP setup {session_id}: started on thread {} ({:?}), {}s per step; scheduling: \
             {started_sched}",
            thread.name().unwrap_or("unnamed"),
            thread.id(),
            step_budget.as_secs(),
        );
        Self {
            session_id: session_id.to_string(),
            headline: headline.to_string(),
            step_budget,
            started,
            started_sched,
            steps: RefCell::new(Vec::new()),
            timed_out: Cell::new(false),
        }
    }

    /// Run `future` as `step` under the step's own deadline, logging and
    /// recording its start and finish. A future that finishes is never
    /// discarded, however late it was polled: the deadline is consulted only
    /// while `future` is still pending. On a timeout the error is the
    /// timeout's message, already logged. A future dropped before it finishes
    /// stays recorded as in progress.
    pub(super) async fn step<F: Future>(
        &self,
        step: SetupStep,
        future: F,
    ) -> Result<F::Output, String> {
        let (index, deadline) = self.begin(step);
        match tokio::time::timeout_at(deadline, future).await {
            Ok(output) => {
                self.finish(index);
                Ok(output)
            }
            Err(_) => Err(self.step_timed_out(index, deadline)),
        }
    }

    fn begin(&self, step: SetupStep) -> (usize, Instant) {
        let now = Instant::now();
        let began = now.saturating_duration_since(self.started);
        log::info!(
            "ACP setup {}: {step} started at +{}",
            self.session_id,
            secs(began)
        );
        let mut steps = self.steps.borrow_mut();
        steps.push(StepRecord {
            step,
            began,
            took: None,
        });
        (steps.len() - 1, now + self.step_budget)
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
            record.step,
            secs(took),
            secs(now)
        );
    }

    /// Whether a step has timed out: the structured form of a timeout's
    /// error, which callers see only after the SDK has wrapped it.
    pub(super) fn timed_out(&self) -> bool {
        self.timed_out.get()
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

    /// Log a setup that failed with `error`, unless the failure is a step's
    /// timeout, which logged itself.
    pub(super) fn failed(&self, error: &str) {
        if self.timed_out.get() {
            return;
        }
        log::warn!(
            "ACP setup {}: failed after {}: {error} ({})",
            self.session_id,
            secs(self.started.elapsed()),
            stall_details(&self.steps.borrow(), self.started.elapsed())
        );
    }

    /// The step at `index` ran past `deadline`: snapshot the scheduling
    /// state, log, and return the error.
    fn step_timed_out(&self, index: usize, deadline: Instant) -> String {
        let fired = Instant::now();
        let message = describe_timeout(
            &self.headline,
            &self.steps.borrow(),
            index,
            self.step_budget,
            fired.saturating_duration_since(self.started),
            fired.saturating_duration_since(deadline),
            &self.started_sched,
            &sched_state::current(),
        );
        log::warn!("ACP setup {}: {message}", self.session_id);
        self.timed_out.set(true);
        message
    }
}

/// Run `future` as `step` of `timeline`, under the step's deadline, or
/// untimed and without a deadline when there is no timeline.
pub(super) async fn timed<F: Future>(
    timeline: Option<&SetupTimeline>,
    step: SetupStep,
    future: F,
) -> Result<F::Output, String> {
    match timeline {
        Some(timeline) => timeline.step(step, future).await,
        None => Ok(future.await),
    }
}

/// `"{headline}: session/new did not answer within 90s; completed: initialize
/// 1.30s, …; setup ran 91.3s; timer fired 0.00s late; scheduling at start: …;
/// at timeout: …"`.
#[allow(clippy::too_many_arguments)]
fn describe_timeout(
    headline: &str,
    steps: &[StepRecord],
    stalled: usize,
    step_budget: Duration,
    elapsed: Duration,
    lateness: Duration,
    start_sched: &SchedState,
    timeout_sched: &SchedState,
) -> String {
    let step = steps[stalled].step;
    let verb = if step.is_agent_request() {
        "did not answer"
    } else {
        "did not finish"
    };
    let completed = match completed_steps(steps) {
        Some(completed) => format!("completed: {completed}"),
        None => "nothing completed".to_string(),
    };
    format!(
        "{headline}: {step} {verb} within {}; {completed}; setup ran {}; timer fired {} late; \
         scheduling at start: {start_sched}; at timeout: {timeout_sched}",
        whole_secs(step_budget),
        secs(elapsed),
        secs(lateness),
    )
}

/// The step in progress at `elapsed`, then the completed ones.
fn stall_details(steps: &[StepRecord], elapsed: Duration) -> String {
    let stalled = match steps.iter().rev().find(|record| record.took.is_none()) {
        Some(record) => format!(
            "waiting for {} for {}",
            record.step,
            secs(elapsed.saturating_sub(record.began))
        ),
        None => match steps.last() {
            Some(last) => {
                let ended = last.began + last.took.unwrap_or_default();
                format!(
                    "between setup steps for {} after {}",
                    secs(elapsed.saturating_sub(ended)),
                    last.step
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
                .map(|took| format!("{} {}", record.step, secs(took)))
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

/// A deadline as written in configuration: whole seconds when it is one.
fn whole_secs(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{}s", duration.as_secs())
    } else {
        secs(duration)
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

    const HEADLINE: &str = "Timed out waiting for ACP protocol startup";

    #[test]
    fn a_timeout_names_the_stalled_step_its_deadline_completed_steps_and_lateness() {
        let steps = [
            done(SetupStep::Initialize, 0.0, 1.3),
            done(SetupStep::RecordInitialize, 1.3, 0.01),
            running(SetupStep::NewSession, 1.31),
        ];
        let message = describe_timeout(
            HEADLINE,
            &steps,
            2,
            Duration::from_secs(90),
            Duration::from_secs_f64(95.31),
            Duration::from_secs(4),
            &sched(31, false),
            &sched(4, true),
        );
        assert_eq!(
            message,
            "Timed out waiting for ACP protocol startup: session/new did not answer within \
             90s; completed: initialize 1.30s, storing initialize 0.01s; setup ran 95.3s; \
             timer fired 4.00s late; scheduling at start: pri 31/31; at timeout: pri 4/31, \
             darwin-bg process"
        );
    }

    #[test]
    fn a_config_option_timeout_names_the_option_and_counts_setup_past_one_budget() {
        let steps = [
            done(SetupStep::Initialize, 0.0, 0.4),
            done(SetupStep::NewSession, 0.4, 89.0),
            done(SetupStep::RecordModes, 89.4, 0.01),
            running(SetupStep::SetConfigOption("model"), 89.41),
        ];
        let message = describe_timeout(
            HEADLINE,
            &steps,
            3,
            Duration::from_secs(90),
            Duration::from_secs_f64(179.41),
            Duration::ZERO,
            &SchedState::default(),
            &SchedState::default(),
        );
        assert!(
            message.starts_with(
                "Timed out waiting for ACP protocol startup: session/set_config_option (model) \
                 did not answer within 90s; completed: initialize 0.40s, session/new 89.0s, \
                 storing session modes 0.01s; setup ran 179.4s; timer fired 0.00s late"
            ),
            "{message}"
        );
    }

    #[test]
    fn a_local_step_timeout_says_it_did_not_finish() {
        let message = describe_timeout(
            HEADLINE,
            &[running(SetupStep::NameSession, 0.5)],
            0,
            Duration::from_secs(90),
            Duration::from_secs_f64(90.5),
            Duration::ZERO,
            &SchedState::default(),
            &SchedState::default(),
        );
        assert!(
            message.starts_with(
                "Timed out waiting for ACP protocol startup: storing held frames did not \
                 finish within 90s; nothing completed; setup ran 90.5s"
            ),
            "{message}"
        );
    }

    #[test]
    fn a_failure_between_steps_says_which_step_finished_last() {
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
    fn a_failure_before_any_step() {
        assert_eq!(
            stall_details(&[], Duration::from_secs(90)),
            "before any setup step; nothing completed"
        );
    }

    #[test]
    fn the_timeout_text_keeps_its_headline_and_reads_as_no_transport_loss() {
        let message = describe_timeout(
            HEADLINE,
            &[running(SetupStep::Initialize, 0.0)],
            0,
            Duration::from_secs(90),
            Duration::from_secs(90),
            Duration::ZERO,
            &SchedState::default(),
            &SchedState::default(),
        );
        assert!(message.starts_with(
            "Timed out waiting for ACP protocol startup: initialize did not answer within 90s; "
        ));
        assert!(!super::super::is_transport_loss_error(&message));
    }

    #[tokio::test(start_paused = true)]
    async fn each_step_gets_its_own_deadline() {
        let timeline = SetupTimeline::start("s1", HEADLINE, Duration::from_secs(90));
        // Two steps that each take most of a budget: together they run well
        // past one budget, and neither times out.
        for step in [SetupStep::Initialize, SetupStep::NewSession] {
            timeline
                .step(step, tokio::time::sleep(Duration::from_secs(89)))
                .await
                .expect("a step inside its own deadline finishes");
        }
        let steps = timeline.steps.borrow().clone();
        assert_eq!(steps[1].began, Duration::from_secs(89));
        assert_eq!(steps[1].took, Some(Duration::from_secs(89)));
    }

    #[tokio::test]
    async fn a_finished_step_is_kept_however_late_it_was_polled() {
        let timeline = SetupTimeline::start("s1", HEADLINE, Duration::from_millis(10));
        // Synchronous work past the deadline, as a session thread that was
        // not scheduled, or blocked on the store, would see it: the answer is
        // there by the first poll, and is kept.
        let output = timeline
            .step(SetupStep::NewSession, async {
                std::thread::sleep(Duration::from_millis(50));
                "answered"
            })
            .await;
        assert_eq!(output, Ok("answered"));
        assert!(!timeline.timed_out.get());
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_step_times_out_and_stays_in_progress() {
        let timeline = SetupTimeline::start("s1", HEADLINE, Duration::from_secs(90));
        timeline
            .step(
                SetupStep::Initialize,
                tokio::time::sleep(Duration::from_millis(1300)),
            )
            .await
            .unwrap();
        let message = timeline
            .step(SetupStep::NewSession, std::future::pending::<()>())
            .await
            .expect_err("a stalled step times out");

        let steps = timeline.steps.borrow().clone();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].step, SetupStep::Initialize);
        assert_eq!(steps[0].took, Some(Duration::from_millis(1300)));
        assert_eq!(steps[1].step, SetupStep::NewSession);
        assert_eq!(steps[1].began, Duration::from_millis(1300));
        assert_eq!(steps[1].took, None);
        assert!(timeline.timed_out.get());

        assert!(
            message.starts_with(
                "Timed out waiting for ACP protocol startup: session/new did not answer within \
                 90s; completed: initialize 1.30s; setup ran 91.3s; timer fired 0.00s late; \
                 scheduling at start: "
            ),
            "{message}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_timeline_a_step_has_no_deadline() {
        let output = timed(None, SetupStep::NewSession, async {
            tokio::time::sleep(Duration::from_secs(600)).await;
            7
        })
        .await;
        assert_eq!(output, Ok(7));
    }

    #[test]
    fn deadlines_print_as_whole_seconds() {
        assert_eq!(whole_secs(Duration::from_secs(90)), "90s");
        assert_eq!(whole_secs(Duration::from_millis(1500)), "1.50s");
    }

    #[test]
    fn durations_keep_precision_below_ten_seconds() {
        assert_eq!(secs(Duration::from_millis(4)), "0.00s");
        assert_eq!(secs(Duration::from_millis(1404)), "1.40s");
        assert_eq!(secs(Duration::from_millis(88_240)), "88.2s");
    }
}
