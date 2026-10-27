//! Interactive doctor logins: one piped-stdin CLI login per check, its
//! streamed output, cancellation, and the code hand-off.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use super::{
    doctor_env_vars, ExecuteFixOptions, FixCancelHandle, FixCancellation, FixStdin, FixStdinWriter,
    FixType,
};

#[cfg(test)]
#[path = "tests/login.rs"]
mod tests;

/// One `doctor-login-output` event: a line of a running login's output, or
/// its end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorLoginOutput {
    pub check_id: String,
    /// The run this event belongs to — see [`ActiveLogin::run_id`]. A client
    /// follows one run and drops events carrying any other id: a check id
    /// alone cannot tell an earlier run's late `done` from the current run's.
    pub run_id: String,
    pub line: Option<String>,
    /// Position of `line` in the run's output, counted from zero; on the final
    /// event, the number of lines the run emitted. A client that attached late
    /// compares it against [`DoctorLoginStatus::next_seq`] to tell a line its
    /// snapshot already covered from one that arrived after the snapshot.
    pub seq: u64,
    pub done: bool,
    /// The fix's failure, when it failed. `None` on a cancelled run: doctor's
    /// runner reports a cancellation as an `Err`, but a client should render
    /// "cancelled", not a failure — that is what `cancelled` is for.
    pub error: Option<String>,
    /// The run ended because [`cancel_doctor_login`] was called on it.
    pub cancelled: bool,
}

/// Answer to [`doctor_login_status`]: whether a login is running for the check,
/// and what it has printed so far.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorLoginStatus {
    pub running: bool,
    /// The running login's run id, which its events carry and which
    /// [`send_doctor_login_code`] and [`cancel_doctor_login`] take. `None`
    /// when nothing is running.
    pub run_id: Option<String>,
    /// The last [`LOGIN_OUTPUT_TAIL_LINES`] lines the run emitted, oldest
    /// first. Empty when nothing is running.
    pub output: Vec<String>,
    /// The `seq` the run's next line will carry. `output` covers the `seq`s
    /// from `next_seq - output.len()` up to but excluding `next_seq`, which is
    /// how a client merges this snapshot with lines it received live. Counted
    /// per run: a new run for the same check starts again from zero.
    pub next_seq: u64,
}

impl DoctorLoginStatus {
    fn idle() -> Self {
        Self {
            running: false,
            run_id: None,
            output: Vec::new(),
            next_seq: 0,
        }
    }
}

/// What [`start_doctor_login`] did, and the run id of the login it either
/// began or found. Serialized as `{ "outcome": "started" | "alreadyRunning",
/// "runId": … }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LoginStart {
    Started {
        run_id: String,
    },
    /// A login for the check was already running. The caller can attach to it:
    /// [`doctor_login_status`] has its output so far, the `doctor-login-output`
    /// stream carries the rest, and [`send_doctor_login_code`] reaches it —
    /// all under this run id.
    AlreadyRunning {
        run_id: String,
    },
}

/// Lines of a login's output retained for a client that attaches after they
/// were streamed — a web client refreshed mid-login, a second client, a
/// reloaded webview. The same count the frontend keeps (`MAX_OUTPUT_LINES` in
/// `agentLogin.svelte.ts`), so a replay restores exactly what a client that
/// watched from the start is showing.
const LOGIN_OUTPUT_TAIL_LINES: usize = 40;

/// Bounded, oldest-first tail of a login's output, with a count of every line
/// that went through it so each line's event can carry its position.
#[derive(Debug, Default)]
struct LoginOutputTail {
    lines: VecDeque<String>,
    /// Lines pushed so far — the `seq` the next one gets.
    next_seq: u64,
}

impl LoginOutputTail {
    /// Record `line`, dropping the oldest once the tail is full, and return the
    /// `seq` the line's event carries.
    fn push(&mut self, line: &str) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.lines.len() == LOGIN_OUTPUT_TAIL_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line.to_string());
        seq
    }

    /// The snapshot a late client replays, for the running login `run_id`.
    fn status(&self, run_id: &str) -> DoctorLoginStatus {
        DoctorLoginStatus {
            running: true,
            run_id: Some(run_id.to_string()),
            output: self.lines.iter().cloned().collect(),
            next_seq: self.next_seq,
        }
    }
}

/// A login fix in flight, keyed by check id in [`ACTIVE_LOGINS`].
struct ActiveLogin {
    /// Identity of this run, minted by [`claim_login`] and unique across every
    /// login Staged has ever run — a UUID, so it stays unique across a backend
    /// restart that a web client's record may outlive.
    ///
    /// Events, the status snapshot and the start answer all carry it, and a
    /// code or a cancel names it. The check id is the slot, not the run: the
    /// slot is released *before* a run's `done` is emitted (see
    /// [`run_login_fix`]), so a fresh start for the same check can land between
    /// the two, and that run's client would otherwise take the earlier run's
    /// `done` as its own end — and a code typed for the earlier run would be
    /// delivered to the new one.
    run_id: String,
    /// The write end of the fix's stdin. Held here for the whole run — this
    /// entry is its only long-lived owner — because dropping the last
    /// `FixStdinWriter` is what closes the pipe: the clone
    /// [`send_doctor_login_code`] takes lives for one write. Without this one
    /// the CLI would read EOF at spawn, before the user had a code to give it.
    ///
    /// The flip side: a fix that reads stdin *to EOF* never gets it while its
    /// slot is held, which is until the run ends — so such a fix would sit
    /// until doctor's `FixTimeout`. None of the login commands does that (each
    /// reads one line), and closing stdin is not how a login is ended anyway:
    /// the Claude CLI ignores EOF and keeps waiting on its browser callback.
    /// Ending a run early is `cancel`'s job.
    writer: FixStdinWriter,
    /// Stops the run through doctor's runner, which owns the child and kills
    /// its whole process tree. See [`cancel_doctor_login`].
    cancel: FixCancelHandle,
    /// The lines streamed so far, shared with the run's `on_line` callback,
    /// which appends to it before emitting each event.
    output: Arc<Mutex<LoginOutputTail>>,
}

/// Login fixes currently running, by check id. This is intentionally only a
/// lifetime map for active subprocesses, not a cache of authentication state;
/// doctor remains the source of truth for whether login is available.
static ACTIVE_LOGINS: OnceLock<Mutex<HashMap<String, ActiveLogin>>> = OnceLock::new();

fn active_logins() -> &'static Mutex<HashMap<String, ActiveLogin>> {
    ACTIVE_LOGINS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A claimed login slot in [`ACTIVE_LOGINS`], released when dropped.
///
/// The release is structural rather than a statement at the end of
/// [`run_login_fix`]: a panic in `doctor_env_vars().await` or in the emit
/// closure, or the awaiting future being dropped, would otherwise leave the
/// entry in place — and with it [`claim_login`] refusing the check until Staged
/// restarts.
///
/// It releases only its own run: the entry is removed when it still carries
/// this slot's `run_id`, never a later run's that took the slot after this one
/// was already gone.
#[derive(Debug)]
struct LoginSlot {
    check_id: String,
    run_id: String,
}

impl Drop for LoginSlot {
    fn drop(&mut self) {
        let mut logins = active_logins().lock().unwrap_or_else(|e| e.into_inner());
        if logins
            .get(&self.check_id)
            .is_some_and(|login| login.run_id == self.run_id)
        {
            logins.remove(&self.check_id);
        }
    }
}

/// Everything a claimed login needs to run: its run id, the pipe and token the
/// fix is given, the handle and tail the entry keeps, and the slot whose drop
/// releases the entry.
#[derive(Debug)]
pub(super) struct ClaimedLogin {
    run_id: String,
    stdin: FixStdin,
    cancellation: FixCancellation,
    cancel: FixCancelHandle,
    output: Arc<Mutex<LoginOutputTail>>,
    slot: LoginSlot,
}

/// What [`claim_login`] found.
#[derive(Debug)]
pub(super) enum LoginClaim {
    /// The slot was free and is now this caller's.
    Claimed(ClaimedLogin),
    /// A login for the check is already running, under this run id.
    AlreadyRunning { run_id: String },
}

/// Why a command aimed at one run of a check's login could not reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StaleLoginRun {
    /// Nothing is running for the check: the run has ended.
    Ended,
    /// The run has ended and a newer one has since taken the check's slot.
    Superseded,
}

impl StaleLoginRun {
    /// The refusal a client shows for a code typed into a login that is gone.
    fn code_refusal(self, check_id: &str) -> String {
        match self {
            Self::Ended => format!(
                "The login this code was typed for has ended; no login is running for {check_id}"
            ),
            Self::Superseded => format!(
                "The login this code was typed for has ended; a newer login is running for \
                 {check_id} and the code was not delivered to it"
            ),
        }
    }
}

/// The entry for `check_id` in `logins`, provided it is the run `run_id` the
/// caller is attached to. The map is keyed by check id, so this is where a
/// command aimed at a finished run is told apart from one aimed at the run
/// that replaced it.
fn find_login_run<'a>(
    logins: &'a HashMap<String, ActiveLogin>,
    check_id: &str,
    run_id: &str,
) -> Result<&'a ActiveLogin, StaleLoginRun> {
    match logins.get(check_id) {
        None => Err(StaleLoginRun::Ended),
        Some(login) if login.run_id != run_id => Err(StaleLoginRun::Superseded),
        Some(login) => Ok(login),
    }
}

/// A fresh run id — see [`ActiveLogin::run_id`].
fn new_login_run_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Reserve the login slot for `check_id` under a fresh run id, returning what
/// the run needs — or the id of the login already running for the check. The
/// entry parked in [`ACTIVE_LOGINS`] is also the "a login is running" flag:
/// doctor's `FixStdin` is single-use, so a second concurrent login for one
/// check is refused here rather than spawning a CLI nothing can type into.
pub(super) fn claim_login(check_id: &str) -> Result<LoginClaim, String> {
    doctor::agents::lookup_fix_command(check_id, &FixType::Auth)
        .ok_or_else(|| format!("No login fix available for {check_id}"))?;
    let run_id = new_login_run_id();
    let (writer, stdin) = FixStdin::pipe();
    // A fresh token per run: a cancelled one stays cancelled and would refuse
    // the retry before it spawned.
    let (cancel, cancellation) = FixCancellation::token();
    let output = Arc::new(Mutex::new(LoginOutputTail::default()));
    let mut logins = active_logins().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(running) = logins.get(check_id) {
        return Ok(LoginClaim::AlreadyRunning {
            run_id: running.run_id.clone(),
        });
    }
    logins.insert(
        check_id.to_string(),
        ActiveLogin {
            run_id: run_id.clone(),
            writer,
            cancel: cancel.clone(),
            output: output.clone(),
        },
    );
    Ok(LoginClaim::Claimed(ClaimedLogin {
        run_id: run_id.clone(),
        stdin,
        cancellation,
        cancel,
        output,
        slot: LoginSlot {
            check_id: check_id.to_string(),
            run_id,
        },
    }))
}

/// The final `doctor-login-output` event for a run.
///
/// A cancelled run comes back from doctor's runner as an `Err`; `cancelled` is
/// what lets a client tell it from a failure, and the runner's message is kept
/// out of `error` so no client renders it as one. A cancel that landed after
/// the fix had already finished changes nothing — the fix's own result stands,
/// as the runner documents — so `cancelled` is only reported on a run that
/// actually ended early.
fn login_done_event(
    check_id: String,
    run_id: String,
    result: &Result<(), String>,
    cancel_requested: bool,
    lines_emitted: u64,
) -> DoctorLoginOutput {
    let cancelled = result.is_err() && cancel_requested;
    DoctorLoginOutput {
        check_id,
        run_id,
        line: None,
        seq: lines_emitted,
        done: true,
        error: if cancelled {
            None
        } else {
            result.as_ref().err().cloned()
        },
        cancelled,
    }
}

/// Run a claimed login fix to completion on a piped stdin, streaming every
/// output line to the frontend as a `doctor-login-output` event and releasing
/// the slot afterwards.
///
/// The final `done` event carries the outcome *and* the outcome is returned, so
/// this serves both entry points: [`start_doctor_login`], which spawns it and
/// watches the stream, and [`run_doctor_fix`], which awaits it.
pub(super) async fn run_login_fix(
    app_handle: tauri::AppHandle,
    check_id: String,
    login: ClaimedLogin,
) -> Result<(), String> {
    let ClaimedLogin {
        run_id,
        stdin,
        cancellation,
        cancel,
        output,
        slot,
    } = login;
    let env_vars = doctor_env_vars().await;
    let event_check_id = check_id.clone();
    let event_run_id = run_id.clone();
    let event_app = app_handle.clone();
    let tail = output.clone();
    let result = doctor::execute_fix_streaming_with_env_options(
        check_id.clone(),
        FixType::Auth,
        ExecuteFixOptions::default()
            .with_env_snapshot(env_vars)
            .with_stdin(stdin)
            .with_cancellation(cancellation),
        move |line| {
            // Recorded before it is emitted, so a `doctor_login_status`
            // snapshot taken between the two already covers the line whose
            // event is about to follow it — the client's `seq` comparison then
            // drops the event rather than showing the line twice.
            let seq = tail.lock().unwrap_or_else(|e| e.into_inner()).push(line);
            crate::web_server::emit_to_all(
                &event_app,
                "doctor-login-output",
                DoctorLoginOutput {
                    check_id: event_check_id.clone(),
                    run_id: event_run_id.clone(),
                    line: Some(line.to_string()),
                    seq,
                    done: false,
                    error: None,
                    cancelled: false,
                },
            );
        },
    )
    .await;

    let lines_emitted = output.lock().unwrap_or_else(|e| e.into_inner()).next_seq;
    let done = login_done_event(
        check_id.clone(),
        run_id,
        &result,
        cancel.is_cancelled(),
        lines_emitted,
    );
    if done.cancelled {
        if let Err(message) = &result {
            log::info!("[doctor login {check_id}] cancelled: {message}");
        }
    }
    // Released *before* the `done` goes out. A client attaches by registering
    // its listener and then asking `doctor_login_status`, so this order gives
    // it a guarantee: a snapshot that says `running` was taken before the
    // `done` was emitted, and the `done` is still ahead of the listener. The
    // other order would let a snapshot report a run whose `done` had already
    // passed, leaving that client waiting for an end it can never see.
    //
    // The price of this order is that a fresh start for the same check can be
    // claimed between the release and the emit, and its client then sees this
    // run's `done`. That is what `run_id` on the event is for: the client
    // follows the run id its start returned and drops this one.
    drop(slot);
    crate::web_server::emit_to_all(&app_handle, "doctor-login-output", done);
    result
}

/// Start an interactive login fix and stream its output to the frontend.
///
/// Returns as soon as the fix is claimed and spawned, with the run id its
/// events will carry; the caller learns the outcome from the `done` event,
/// which lets it feed a code through [`send_doctor_login_code`] while the fix
/// is still running. A login already running for the check is reported as
/// [`LoginStart::AlreadyRunning`] with *its* run id rather than as an error:
/// it is the same subprocess the caller wanted, and it can attach to it (see
/// [`doctor_login_status`]).
#[tauri::command]
pub async fn start_doctor_login(
    app_handle: tauri::AppHandle,
    check_id: String,
) -> Result<LoginStart, String> {
    let login = match claim_login(&check_id)? {
        LoginClaim::Claimed(login) => login,
        LoginClaim::AlreadyRunning { run_id } => return Ok(LoginStart::AlreadyRunning { run_id }),
    };
    let run_id = login.run_id.clone();
    tokio::spawn(async move {
        // A failure is reported to the frontend by the final `done` event; this
        // handle has no caller to return it to.
        let _ = run_login_fix(app_handle, check_id, login).await;
    });
    Ok(LoginStart::Started { run_id })
}

/// Ask run `run_id` of `check_id`'s login to stop, reporting whether it was
/// found. Idempotent, and harmless when nothing is running — or when the run
/// has ended and a newer one holds the check's slot, which is left alone: it
/// is someone else's login, and `false` tells the caller its own is gone.
///
/// The stop goes through doctor's cancellation token, which makes the runner —
/// the owner of the child — kill the fix's process tree, and the run then ends
/// with a `done` event carrying `cancelled: true`. It is deliberately *not*
/// done by dropping the stdin writer: the Claude CLI ignores EOF on stdin once
/// it has printed its URL and keeps waiting on its browser callback (the
/// stdin-vs-TTY experiments ran it with `< /dev/null` and killed it 25s later,
/// still waiting), so a closed pipe would leave the slot held until the fix
/// timeout with nothing to show for it.
#[tauri::command]
pub async fn cancel_doctor_login(check_id: String, run_id: String) -> bool {
    let cancel = find_login_run(
        &active_logins().lock().unwrap_or_else(|e| e.into_inner()),
        &check_id,
        &run_id,
    )
    .map(|login| login.cancel.clone());
    match cancel {
        Ok(cancel) => {
            cancel.cancel();
            true
        }
        Err(_) => false,
    }
}

/// Whether a login is running for `check_id`, under which run id, and what it
/// has printed so far — for a client that lost its own record of the login (a
/// web refresh, a second client, a reloaded webview) and needs the sign-in URL
/// and code entry back.
///
/// Register the `doctor-login-output` listener *before* calling this, then
/// follow the run id it names and merge that run's lines by `seq`: lines below
/// [`DoctorLoginStatus::next_seq`] are in the snapshot, lines at or above it
/// arrived after it.
#[tauri::command]
pub async fn doctor_login_status(check_id: String) -> DoctorLoginStatus {
    // The map lock and the tail lock are never held together: the `on_line`
    // callback takes only the tail's, the slot release only the map's.
    let running = active_logins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&check_id)
        .map(|login| (login.run_id.clone(), login.output.clone()));
    match running {
        Some((run_id, output)) => output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status(&run_id),
        None => DoctorLoginStatus::idle(),
    }
}

/// Deliver a line — in practice the authentication code the agent CLI asked
/// for — to run `run_id` of a login started by [`start_doctor_login`] or
/// [`run_doctor_fix`]. A code typed for a run that has since ended is refused,
/// with the refusal saying so, rather than delivered to whatever login now
/// holds the check's slot: that one printed a different URL, and a code for
/// the old one would only be rejected by it — or, worse, accepted.
///
/// `async` deliberately: `send_line` writes into the fix's stdin pipe inline
/// and can block if the fix isn't reading, and under Tauri 2 a non-`async`
/// command body runs on the main thread — the worst possible place to discover
/// a full pipe. The write itself goes to a blocking thread for the same reason.
#[tauri::command]
pub async fn send_doctor_login_code(
    check_id: String,
    run_id: String,
    code: String,
) -> Result<(), String> {
    let writer = find_login_run(
        &active_logins().lock().unwrap_or_else(|e| e.into_inner()),
        &check_id,
        &run_id,
    )
    .map(|login| login.writer.clone())
    .map_err(|stale| stale.code_refusal(&check_id))?;
    tokio::task::spawn_blocking(move || writer.send_line(code))
        .await
        .map_err(|e| format!("Failed to deliver the login code for {check_id}: {e}"))?
}
