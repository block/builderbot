//! Tauri command wrappers for the doctor health-check system.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tauri::Manager;

use crate::managed_acp_tools::{ManagedTool, ReconcileRecord};
use crate::session_queue::sweep::{self, QueueHealth};
use crate::session_runner::SessionRegistry;
use crate::store::Store;

pub use doctor::types::{AuthStatus, InstallSource};
pub use doctor::{
    AgentVersionInfo, CheckStatus, DoctorCheck, DoctorReport, ExecuteFixOptions, FixCancelHandle,
    FixCancellation, FixStdin, FixStdinWriter, FixType, RunChecksOptions,
};

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
struct ClaimedLogin {
    run_id: String,
    stdin: FixStdin,
    cancellation: FixCancellation,
    cancel: FixCancelHandle,
    output: Arc<Mutex<LoginOutputTail>>,
    slot: LoginSlot,
}

/// What [`claim_login`] found.
#[derive(Debug)]
enum LoginClaim {
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

/// Environment snapshot for doctor checks and fixes — and for the managed
/// bridge installs themselves (see
/// [`crate::managed_acp_tools::install_managed_tool`]), so the registry npm
/// resolves is the same for a check, its fix, and the reconciler. Shaped
/// through `apply_managed_tools_env` so checks resolve binaries from the same
/// PATH the agent spawn path uses — a bridge Staged manages must never be
/// reported missing (or prompt an install) just because the user has no
/// global copy. The managed npm env is overlaid on top, so checks probe npm
/// state (`npm prefix -g`, version lookups) with the same private-prefix view
/// the fixes install into — a check never contradicts the fix that just ran.
pub(crate) async fn doctor_env_vars() -> Vec<(String, String)> {
    let mut env_vars = crate::shell_env::home_env_vars_with_extended_path(
        crate::session_runner::shell_env_cache().as_ref(),
    )
    .await;
    crate::acp_tools::apply_managed_tools_env(&mut env_vars);
    crate::managed_acp_tools::apply_managed_npm_env(
        &mut env_vars,
        &crate::managed_acp_tools::managed_npm_env(),
    );
    env_vars
}

fn run_checks_options(
    check_freshness: bool,
    env_vars: Vec<(String, String)>,
    bundled_dir: Option<PathBuf>,
) -> RunChecksOptions {
    RunChecksOptions {
        check_freshness,
        offline: false,
        // npm-backed checks and fixes pass a `--registry` only when the user
        // set `STAGED_NPM_REGISTRY`; otherwise npm resolves its own config
        // (see the registry policy note in `managed_acp_tools`).
        npm_registry: crate::managed_acp_tools::npm_registry_override(),
        env: None,
        // Doctor labels binaries resolved from this dir as bundled (install
        // source + readout flag) and suppresses registry update fixes for
        // them — the startup reconciler floats the managed shims to @latest,
        // so Staged owns their updates.
        bundled_tools_dir: bundled_dir,
    }
    .with_env_snapshot(env_vars)
}

fn execute_fix_options(
    command_override: Option<String>,
    env_vars: Vec<(String, String)>,
) -> ExecuteFixOptions {
    execute_fix_options_through(
        command_override,
        crate::managed_acp_tools::npm_registry_override(),
        env_vars,
    )
}

/// [`execute_fix_options`] with the registry chosen by the caller — the
/// gateway-blocked retry passes the compiled-in fallback here.
fn execute_fix_options_through(
    command_override: Option<String>,
    npm_registry: Option<String>,
    env_vars: Vec<(String, String)>,
) -> ExecuteFixOptions {
    // Everything else stays at doctor's defaults: the fixes that reach this
    // builder are the non-interactive ones (installs and updates), so nothing
    // here feeds a prompt and the child keeps inheriting stdin rather than
    // getting a piped one; the standard fix timeout is far above any install
    // this runs. Interactive logins do *not* come through here — every
    // `FixType::Auth` run goes through [`run_login_fix`], which pipes stdin so
    // the code the CLI asks for can actually be delivered. Spelled with
    // `..Default::default()` so a new doctor option doesn't break this
    // workspace-excluded crate, which `cargo check` under `crates/` never
    // compiles but `staged-ci.yml` does.
    ExecuteFixOptions {
        command_override,
        npm_registry,
        ..Default::default()
    }
    .with_env_snapshot(env_vars)
}

/// Run a doctor fix through the crate's executor with the registry precedence
/// the managed installer applies: npm's own config (or the explicit
/// `STAGED_NPM_REGISTRY`) first, and — only for an npm-backed command that
/// failed as a gateway block of `registry.npmjs.org`, with no override set
/// and a fallback compiled in — one retry with `--registry <fallback>`. The
/// executor surfaces the failed command's stderr as its error string, which
/// is exactly what the classifier reads; when the retry fails too, the
/// surfaced error keeps the original diagnosis in front of the retry's.
async fn execute_fix_with_gateway_fallback(
    check_id: String,
    fix_type: FixType,
    command_override: Option<String>,
    npm_backed: bool,
    env_vars: Vec<(String, String)>,
) -> Result<(), String> {
    let first = doctor::execute_fix_with_env_options(
        check_id.clone(),
        fix_type.clone(),
        execute_fix_options(command_override.clone(), env_vars.clone()),
    )
    .await;
    let Err(error) = first else {
        return Ok(());
    };
    if !npm_backed {
        return Err(error);
    }
    let Some(fallback) = crate::managed_acp_tools::gateway_fallback_registry(
        crate::managed_acp_tools::npm_registry_override().as_deref(),
        crate::managed_acp_tools::fallback_npm_registry(),
        &error,
    ) else {
        return Err(error);
    };
    log::info!(
        "[doctor fix {check_id}] npm could not reach registry.npmjs.org (network gateway block page); retrying through {fallback}"
    );
    doctor::execute_fix_with_env_options(
        check_id,
        fix_type,
        execute_fix_options_through(command_override, Some(fallback.to_string()), env_vars),
    )
    .await
    .map_err(|retry_error| {
        crate::managed_acp_tools::gateway_retry_failed_message(fallback, &retry_error)
    })
}

/// Run all health checks and return the report.
///
/// This is the cheap, no-network path: it resolves binaries and reports
/// install/auth status but does not probe registries for version freshness.
/// The frontend calls this first for an instant paint, then follows up with
/// [`run_doctor_freshness`] to fill in version/update information.
///
/// Takes the app handle rather than `State` borrows so the command keeps its
/// plain `DoctorReport` return: the session-queue check reads store and
/// registry state synchronously before the first await.
#[tauri::command]
pub async fn run_doctor(app_handle: tauri::AppHandle) -> DoctorReport {
    let queue = session_queue_doctor_check(&app_handle);
    run_doctor_report(false, queue).await
}

/// Run all health checks with version freshness enabled.
///
/// This is the slower second pass: it probes each readout's installed version
/// and looks up the latest version from the relevant registry (npm, brew,
/// crates.io, GitHub releases), populating `installedVersion`, `latestVersion`,
/// `updateAvailable`, and the source-aware `updateCommand`/`updateFixType` on
/// each readout. Hits the network, so it must never block first paint.
#[tauri::command]
pub async fn run_doctor_freshness(app_handle: tauri::AppHandle) -> DoctorReport {
    let queue = session_queue_doctor_check(&app_handle);
    run_doctor_report(true, queue).await
}

/// Run the doctor crate's checks plus Staged-local ones over one shared env
/// snapshot: the managed Node.js runtime check, the session-queue check the
/// caller already took (`queue_check`; both passes carry it, since the
/// frontend merges the freshness report into the first by check id), and the
/// managed-bridge version readouts layered onto the Claude/Codex rows (see
/// [`apply_managed_bridge_readouts`]). Bundled readouts are labeled by the
/// doctor crate itself via `RunChecksOptions::bundled_tools_dir`.
///
/// The managed versions are gathered concurrently with the crate's run: the
/// installed version is a local `package.json` read on both passes, and the
/// registry lookup for the latest version runs only when `check_freshness`
/// is set, so the first paint never waits on the network.
async fn run_doctor_report(
    check_freshness: bool,
    queue_check: Option<DoctorCheck>,
) -> DoctorReport {
    let env_vars = doctor_env_vars().await;
    let (mut report, node_runtime, managed_versions) = tokio::join!(
        doctor::run_checks_with_options(run_checks_options(
            check_freshness,
            env_vars.clone(),
            crate::acp_tools::primary_tools_dir(),
        )),
        run_node_runtime_check(),
        managed_bridge_versions(check_freshness, &env_vars),
    );
    match crate::managed_acp_tools::managed_shim_bin_dir() {
        Some(shim_bin_dir) => {
            let diagnostics =
                managed_install_diagnostics(&report, &managed_versions, &shim_bin_dir, &env_vars)
                    .await;
            apply_managed_bridge_readouts(
                &mut report,
                &managed_versions,
                &shim_bin_dir,
                check_freshness,
                &diagnostics,
            );
        }
        None => {
            if let Some(reason) = crate::managed_acp_tools::managed_tools_disabled_reason() {
                note_managed_installs_disabled(&mut report, &reason);
            }
        }
    }
    if let Some(check) = node_runtime {
        report.checks.push(check);
    }
    report.checks.extend(queue_check);
    report
}

/// Reserve the login slot for `check_id` under a fresh run id, returning what
/// the run needs — or the id of the login already running for the check. The
/// entry parked in [`ACTIVE_LOGINS`] is also the "a login is running" flag:
/// doctor's `FixStdin` is single-use, so a second concurrent login for one
/// check is refused here rather than spawning a CLI nothing can type into.
fn claim_login(check_id: &str) -> Result<LoginClaim, String> {
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
async fn run_login_fix(
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

#[tauri::command]
pub async fn run_doctor_fix(
    app_handle: tauri::AppHandle,
    check_id: String,
    fix_type: FixType,
) -> Result<(), String> {
    if check_id == NODE_RUNTIME_CHECK_ID {
        return ensure_managed_node_runtime_for_fix().await;
    }
    // An auth fix is the one interactive fix: it prints a verification URL and
    // then blocks reading a code from stdin. Route it through the same piped,
    // streamed path `start_doctor_login` uses so both entry points behave the
    // same. On inherited stdin — `/dev/null` in the GUI — this printed its URL
    // to a log nobody reads and could only ever finish through the CLI's own
    // browser callback, otherwise dying at the fix timeout with no way to enter
    // the code (block/berd#99).
    if matches!(fix_type, FixType::Auth) {
        // This entry point awaits the fix to its end, so there is no run to
        // hand an "already running" answer to — the caller asked for a fix and
        // there is one it can't have.
        let login = match claim_login(&check_id)? {
            LoginClaim::Claimed(login) => login,
            LoginClaim::AlreadyRunning { .. } => {
                return Err(format!("A login is already running for {check_id}"));
            }
        };
        return run_login_fix(app_handle, check_id, login).await;
    }
    if matches!(fix_type, FixType::Command | FixType::Bridge) {
        if let Some(tool_id) = managed_tool_for_check(&check_id) {
            return install_managed_tool_logged(tool_id, &check_id).await;
        }
    }
    let env_vars = doctor_env_vars().await;
    let npm_backed = doctor::agents::lookup_fix_command(&check_id, &fix_type)
        .as_deref()
        .is_some_and(crate::managed_acp_tools::is_npm_backed_command);
    if npm_backed {
        ensure_managed_node_runtime_for_fix().await?;
    }
    execute_fix_with_gateway_fallback(check_id, fix_type, None, npm_backed, env_vars).await
}

/// The install id of the ACP bridge a doctor check fronts, for the two checks
/// whose bridge Staged can manage — whether or not this build does.
fn managed_tool_id_for_check(check_id: &str) -> Option<&'static str> {
    match check_id {
        "ai-agent-claude" => Some("claude-acp"),
        "ai-agent-codex" => Some("codex-acp"),
        _ => None,
    }
}

/// The managed ACP bridge behind a doctor check id, when this build manages
/// it. `None` routes the check to the doctor crate's regular fix commands —
/// which is also the correct fallback whenever bridge management is off (dev
/// override, `no-managed-acp-tools`, unsupported target).
fn managed_tool_for_check(check_id: &str) -> Option<&'static str> {
    crate::managed_acp_tools::managed_tool(managed_tool_id_for_check(check_id)?).map(|tool| tool.id)
}

/// The managed bridge a doctor *update* targets: on a build that manages these
/// two packages, the managed installer is the only writer for them, so *every*
/// update of the Claude and Codex checks goes to it — `UpdateBridge` from a
/// managed row's ACP readout, and `UpdateMain` from a copy that resolved
/// elsewhere (a user install found on PATH before the first reconcile lands,
/// or while it keeps failing). Letting that `UpdateMain` take the
/// command-validated path instead would run `npm install -g <pkg>@latest`
/// under the fix env's `NPM_CONFIG_PREFIX`, landing a third, unowned copy in
/// `~/.staged/packages/npm-prefix` that nothing prunes, updates or runs — and
/// leaving the user's copy stale anyway.
///
/// There is still exactly one install per row, because the two update fix
/// types never both carry an action on the same check:
/// [`apply_managed_bridge_readout`] gives the managed row its `bridge` action
/// while `main` stays bundled and update-free, and an unmanaged row has only
/// the `main` action that
/// [`redirect_unmanaged_update_to_managed_install`] rewrites — the crate emits
/// no `bridge` readout for the single-binary Claude and Codex checks.
///
/// Non-update fix types, and every check when management is off (dev override,
/// `no-managed-acp-tools`, unsupported target), fall through to the
/// command-validated path.
fn managed_update_target(check_id: &str, fix_type: &FixType) -> Option<&'static str> {
    if !matches!(fix_type, FixType::UpdateMain | FixType::UpdateBridge) {
        return None;
    }
    managed_tool_for_check(check_id)
}

/// Install (or float-upgrade) a managed bridge for a doctor fix/update.
/// Progress goes to the log — doctor fixes have no streamed-output channel,
/// only a button spinner.
async fn install_managed_tool_logged(tool_id: &str, check_id: &str) -> Result<(), String> {
    let log_prefix = format!("[doctor fix {check_id}]");
    let env_vars = doctor_env_vars().await;
    crate::managed_acp_tools::install_managed_tool(tool_id, &env_vars, &|line| {
        log::info!("{log_prefix} {line}");
    })
    .await
    .map_err(|error| error.to_string())
}

/// Install (or repair) the managed Node.js runtime ahead of a fix that needs
/// it. Progress goes to the log — doctor fixes have no streamed-output
/// channel, only a button spinner.
async fn ensure_managed_node_runtime_for_fix() -> Result<(), String> {
    crate::managed_node::ensure_managed_node_runtime()
        .await
        .map_err(|error| error.to_string())
}

/// Run a source-aware update for a single readout (main CLI or ACP bridge).
///
/// Unlike [`run_doctor_fix`], update commands (`UpdateMain`/`UpdateBridge`) are
/// derived per-readout at freshness time rather than living in the static check
/// table, so the executor needs the command passed in as an override.
///
/// **Trust boundary:** we do not execute the frontend-supplied `command`
/// blindly. We re-run freshness, re-derive the expected `updateCommand` for
/// `(check_id, fix_type)` backend-side, and only proceed if the two match. This
/// keeps `run_doctor_update` from becoming an arbitrary-shell-exec hole — the
/// `command` argument is effectively a confirmation of what the UI displayed,
/// validated against the authoritative backend derivation.
#[tauri::command]
pub async fn run_doctor_update(
    check_id: String,
    fix_type: FixType,
    command: String,
) -> Result<(), String> {
    // On a build that manages these two bridges, every update of their checks
    // is the floating installer itself: `<pkg>@latest` staged, verified and
    // swapped into `~/.staged/packages` on the managed runtime. That covers
    // both the managed row's ACP readout (see `apply_managed_bridge_readout`)
    // and a copy that resolved elsewhere, whose source-aware `UpdateMain` is
    // rewritten into a managed-install description by
    // `redirect_unmanaged_update_to_managed_install` — its own `npm install -g`
    // would write into the private prefix and orphan a copy there. No shell
    // command runs, so the frontend-supplied `command` — the description the
    // confirmation showed — needs no validation; what is validated is the
    // target: a check whose bridge this build manages, and an update fix type.
    if let Some(tool_id) = managed_update_target(&check_id, &fix_type) {
        return install_managed_tool_logged(tool_id, &check_id).await;
    }
    let env_vars = doctor_env_vars().await;
    let expected = expected_update_command(
        &check_id,
        &fix_type,
        env_vars.clone(),
        crate::acp_tools::primary_tools_dir(),
    )
    .await?;
    if expected != command {
        return Err(format!(
            "Update command mismatch for {check_id}: refusing to run a command \
             that does not match the backend-derived update command."
        ));
    }
    // npm-backed updates run the managed npm into the private prefix, so the
    // managed runtime must exist before the command does.
    let npm_backed = crate::managed_acp_tools::is_npm_backed_command(&expected);
    if npm_backed {
        ensure_managed_node_runtime_for_fix().await?;
    }
    // Run the backend-derived `expected`, not the frontend-supplied `command`.
    // They are equal past the guard above, but executing `expected` makes the
    // command that runs provably the one the backend derived — no dependence on
    // the equality check surviving future edits.
    execute_fix_with_gateway_fallback(check_id, fix_type, Some(expected), npm_backed, env_vars)
        .await
}

/// Re-run freshness and return the authoritative update command for the given
/// check + slot, or an error if no actionable update is derivable. Passes the
/// bundled dir through so bundled readouts derive no update command here,
/// exactly as in the report the UI rendered.
async fn expected_update_command(
    check_id: &str,
    fix_type: &FixType,
    env_vars: Vec<(String, String)>,
    bundled_dir: Option<PathBuf>,
) -> Result<String, String> {
    let report =
        doctor::run_checks_with_options(run_checks_options(true, env_vars, bundled_dir)).await;

    let check = report
        .checks
        .iter()
        .find(|c| c.id == check_id)
        .ok_or_else(|| format!("No such check: {check_id}"))?;

    // The fix type selects which readout's update applies.
    let readout = match fix_type {
        FixType::UpdateMain => check.main.as_ref(),
        FixType::UpdateBridge => check.bridge.as_ref(),
        _ => return Err(format!("{fix_type:?} is not an update fix type")),
    };

    readout
        .and_then(|r| r.update_command.clone())
        .ok_or_else(|| format!("No actionable update available for {check_id}"))
}

// =============================================================================
// Managed bridge readouts — the versions of the ACP bridges Staged installs
// =============================================================================

/// What Staged knows about one managed bridge's versions at report time.
///
/// Gathered outside the doctor crate: the crate labels the managed shims
/// bundled and, on that policy, probes only the vendored agent CLI's version
/// (`main`) and offers no update. Staged updates these packages itself at
/// launch and daily, so its rows additionally show — and can update — the ACP
/// package, as a second readout on the same executable.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedBridgeVersions {
    tool: ManagedTool,
    /// The ACP package version installed in `~/.staged/packages`, from the
    /// live package's `package.json` (`state.json` as fallback).
    installed: Option<String>,
    /// The newest published version of the *same* package. `None` outside the
    /// freshness pass and whenever the lookup failed — unknown, not current.
    latest: Option<String>,
}

/// The versions of every bridge this build manages, gathered concurrently.
/// Empty when nothing is managed (dev override, `no-managed-acp-tools`,
/// unsupported target), which is also what keeps those builds from ever
/// growing a managed update action. The registry is asked only when
/// `check_freshness` is set.
async fn managed_bridge_versions(
    check_freshness: bool,
    env_vars: &[(String, String)],
) -> Vec<ManagedBridgeVersions> {
    let tools = crate::managed_acp_tools::managed_tools();
    if tools.is_empty() {
        return Vec::new();
    }
    let Some(packages_root) = crate::paths::packages_dir() else {
        return Vec::new();
    };
    let handles: Vec<_> = tools
        .into_iter()
        .map(|tool| {
            let env_vars = env_vars.to_vec();
            let packages_root = packages_root.clone();
            tokio::spawn(async move {
                let installed =
                    crate::managed_acp_tools::installed_tool_version(&packages_root, &tool);
                let latest = if check_freshness {
                    crate::managed_acp_tools::latest_published_version(&tool, &env_vars).await
                } else {
                    None
                };
                ManagedBridgeVersions {
                    tool,
                    installed,
                    latest,
                }
            })
        })
        .collect();
    let mut versions = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(version) = handle.await {
            versions.push(version);
        }
    }
    versions
}

/// What Doctor says about the managed installer on a Claude/Codex row whose
/// executable is *not* the managed shim: the last reconcile's outcome from
/// `state.json` — the only record a failed launch install leaves behind — and
/// the npm registry a managed install resolves on this machine. Gathered once
/// per report; nothing here is persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ManagedInstallDiagnostics {
    /// `state.json`'s `lastReconcile`, `None` before the first reconcile has
    /// finished (or with no `state.json` at all).
    last_reconcile: Option<ReconcileRecord>,
    /// The registry label for the row's raw output: the `STAGED_NPM_REGISTRY`
    /// override, what `npm config get registry` answered under the doctor
    /// env snapshot, or "npm default" when neither could be asked.
    npm_registry: String,
    /// Wall-clock milliseconds the reconcile age is rendered against.
    now_ms: u64,
}

impl ManagedInstallDiagnostics {
    /// The last reconcile's error for `tool_id`, without the `<id>: ` prefix
    /// the reconciler records it under.
    fn reconcile_error_for(&self, tool_id: &str) -> Option<&str> {
        reconcile_error_for(self.last_reconcile.as_ref()?, tool_id)
    }

    /// One raw-output line on the last managed install run.
    fn last_reconcile_line(&self) -> String {
        match &self.last_reconcile {
            None => "last managed install: none recorded yet".to_string(),
            Some(record) => format!(
                "last managed install: {} {}",
                if record.ok { "succeeded" } else { "failed" },
                format_age(record.at_ms, self.now_ms)
            ),
        }
    }
}

/// The reconciler records each failure as `<tool id>: <error>`; the error on
/// its own, for the tool asked about.
fn reconcile_error_for<'a>(record: &'a ReconcileRecord, tool_id: &str) -> Option<&'a str> {
    record
        .errors
        .iter()
        .find_map(|error| error.strip_prefix(tool_id)?.strip_prefix(": "))
}

/// `just now`, `N minutes ago`, `N hours ago`, `N days ago` — enough to tell a
/// stale record from the launch reconcile that just ran.
fn format_age(at_ms: u64, now_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(at_ms) / 1000;
    let ago = |count: u64, unit: &str| {
        let plural = if count == 1 { "" } else { "s" };
        format!("{count} {unit}{plural} ago")
    };
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => ago(seconds / 60, "minute"),
        3600..=86_399 => ago(seconds / 3600, "hour"),
        _ => ago(seconds / 86_400, "day"),
    }
}

/// Gather [`ManagedInstallDiagnostics`] for a report. The registry probe
/// spawns `npm`, so it runs only when a Claude/Codex row will actually show
/// it: one that resolved to a copy outside the managed shim dir.
async fn managed_install_diagnostics(
    report: &DoctorReport,
    versions: &[ManagedBridgeVersions],
    shim_bin_dir: &Path,
    env_vars: &[(String, String)],
) -> ManagedInstallDiagnostics {
    let unmanaged_row_present = report.checks.iter().any(|check| {
        let Some(tool_id) = managed_tool_id_for_check(&check.id) else {
            return false;
        };
        let Some(tool) = versions.iter().find(|v| v.tool.id == tool_id) else {
            return false;
        };
        check
            .path
            .as_deref()
            .is_some_and(|path| !is_managed_shim_path(path, shim_bin_dir, tool.tool.binary))
    });
    let npm_registry = if unmanaged_row_present {
        detect_npm_registry_label(env_vars).await
    } else {
        String::new()
    };
    ManagedInstallDiagnostics {
        last_reconcile: last_reconcile_record(),
        npm_registry,
        now_ms: crate::managed_acp_tools::now_ms(),
    }
}

/// `state.json`'s last reconcile record, if any.
fn last_reconcile_record() -> Option<ReconcileRecord> {
    let packages_root = crate::paths::packages_dir()?;
    crate::managed_acp_tools::read_state(&packages_root).last_reconcile
}

/// The npm registry a managed install resolves, labeled with where that
/// answer came from — so a row can say which mirror (or the public registry)
/// the failed install actually asked.
async fn detect_npm_registry_label(env_vars: &[(String, String)]) -> String {
    if let Some(registry) = crate::managed_acp_tools::npm_registry_override() {
        return format!("{registry} (STAGED_NPM_REGISTRY)");
    }
    match crate::managed_acp_tools::configured_npm_registry(env_vars).await {
        Some(registry) => format!("{registry} (per npm config)"),
        None => "npm default (per npm config; no npm on PATH to ask)".to_string(),
    }
}

/// Layer the managed ACP readout onto every check whose executable is one of
/// the managed shims in `shim_bin_dir` (see [`apply_managed_bridge_readout`]).
/// Every other Claude/Codex row is a copy Staged does not manage: it is
/// flagged as such, with why the managed copy is missing and the managed
/// install as its fix (see [`flag_missing_managed_copy`]), and its update is
/// pointed at the managed installer too (see
/// [`redirect_unmanaged_update_to_managed_install`]) — on a managing build it
/// is the only writer for these two packages.
fn apply_managed_bridge_readouts(
    report: &mut DoctorReport,
    versions: &[ManagedBridgeVersions],
    shim_bin_dir: &Path,
    check_freshness: bool,
    diagnostics: &ManagedInstallDiagnostics,
) {
    for check in &mut report.checks {
        let Some(tool_id) = managed_tool_id_for_check(&check.id) else {
            continue;
        };
        if let Some(versions) = versions.iter().find(|v| v.tool.id == tool_id) {
            if !apply_managed_bridge_readout(check, versions, shim_bin_dir, check_freshness) {
                flag_missing_managed_copy(check, versions, shim_bin_dir, diagnostics);
                redirect_unmanaged_update_to_managed_install(check, versions, shim_bin_dir);
            }
        }
    }
}

/// Note on the Claude/Codex rows that this build does not manage bridge
/// installs, and why — the `STAGED_ACP_TOOLS_DIR` dev override, the
/// `no-managed-acp-tools` feature, an unsupported target. A copy resolved from
/// Homebrew (or the override dir) is then by design, and the row must not read
/// like a failed install. Status is kept; only the message and raw output
/// gain the note.
fn note_managed_installs_disabled(report: &mut DoctorReport, reason: &str) {
    for check in &mut report.checks {
        if managed_tool_id_for_check(&check.id).is_none() {
            continue;
        }
        let note = format!("managed installs disabled: {reason}");
        check.message = format!("{} ({note})", check.message);
        append_raw_output(check, &note);
    }
}

/// Add a block to a check's raw output, below whatever the crate recorded.
fn append_raw_output(check: &mut DoctorCheck, block: &str) {
    check.raw_output = Some(match check.raw_output.take() {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{}\n\n{block}", existing.trim_end())
        }
        _ => block.to_string(),
    });
}

/// Flag a Claude/Codex row whose executable is a copy Staged does not manage
/// — Homebrew, the user's npm prefix, nvm — and report whether it did.
///
/// On a managing build the managed shim dir is first on the probe PATH, so a
/// copy from anywhere else can only win while the managed copy is *absent*:
/// the launch reconcile has not finished yet, or it failed (no network to the
/// registry, a gateway block page, a 404 from a mirror, a validation failure).
/// Left as the crate built it, the row is a green "Installed" pointing at
/// `/opt/homebrew/bin`, indistinguishable from a healthy setup and with no
/// way to get the managed copy. So:
///
/// - `status` drops to `Warn` (a row already warning or failing keeps its
///   status), and `message` says which copy is in use, that the managed copy
///   is missing, and — when the last reconcile recorded an error for this
///   tool — that error, which names the registry or URL it used.
/// - `raw_output` gains the expected shim path, the last reconcile's outcome
///   and age, the error with a classified hint on what to change
///   (`install_failure_hint`), and the npm registry Doctor detected.
/// - The row's fix becomes the managed install (`Bridge`, which
///   `run_doctor_fix` routes to `install_managed_tool_logged` for these two
///   checks), described by [`managed_install_description`] — the same text
///   [`redirect_unmanaged_update_to_managed_install`] gives the row's update
///   when a newer release is known, so the two buttons never disagree. The
///   fix is offered whether or not an update is available: the point is to
///   get the managed copy, not to chase a version.
///
/// `path`, `main` and the version fields stay the crate's: the row still shows
/// the copy that is actually on PATH and how old it is.
fn flag_missing_managed_copy(
    check: &mut DoctorCheck,
    versions: &ManagedBridgeVersions,
    shim_bin_dir: &Path,
    diagnostics: &ManagedInstallDiagnostics,
) -> bool {
    let Some(path) = check.path.clone() else {
        return false;
    };
    let tool = &versions.tool;
    if is_managed_shim_path(&path, shim_bin_dir, tool.binary) {
        return false;
    }
    let reconcile_error = diagnostics.reconcile_error_for(tool.id);
    let error_suffix = reconcile_error
        .map(|error| format!(": {error}"))
        .unwrap_or_default();
    if check.status == CheckStatus::Pass {
        check.status = CheckStatus::Warn;
        check.message = format!("Using {path} — Staged's managed copy is missing{error_suffix}");
    } else {
        check.message = format!(
            "{}; using {path} — Staged's managed copy is missing{error_suffix}",
            check.message
        );
    }

    let mut detail = vec![
        format!(
            "Staged-managed copy: missing (expected {})",
            shim_bin_dir.join(tool.binary).display()
        ),
        diagnostics.last_reconcile_line(),
    ];
    if let Some(error) = reconcile_error {
        detail.push(format!("{}: {error}", tool.id));
        if let Some(hint) = crate::managed_acp_tools::install_failure_hint(error, tool.package) {
            detail.push(format!("hint: {hint}"));
        }
    }
    detail.push(format!("npm registry: {}", diagnostics.npm_registry));
    append_raw_output(check, &detail.join("\n"));

    let latest = versions.latest.as_deref().or_else(|| {
        check
            .main
            .as_ref()
            .and_then(|main| main.latest_version.as_deref())
    });
    check.fix_type = Some(FixType::Bridge);
    check.fix_command = Some(managed_install_description(tool, latest, &path));
    true
}

/// Give a managed Claude/Codex row the two readouts its one executable
/// stands for, and report whether it did.
///
/// - `main` is left as the doctor crate built it: the vendored agent CLI's
///   version, probed through the bridge passthrough, with the crate's bundled
///   suppression (no update) intact — the agent is updated as part of the
///   package, never on its own.
/// - `bridge` is added: the ACP package's installed version, and on the
///   freshness pass the latest published version of that same package,
///   `update_available` from comparing the two, and — only when newer — the
///   one actionable update, typed `UpdateBridge`, which `run_doctor_update`
///   routes to the managed installer. The command text describes updating
///   Staged's managed copy; nothing runs it as a shell command.
/// - `bridge_path` is set to the same executable so the row's path gate
///   renders the readout, on both the initial and the freshness report — the
///   frontend's freshness merge copies `bridge` but not `bridge_path`, so a
///   readout that appeared only on the second pass would stay hidden.
/// - The flat version fields mirror the ACP readout, as the crate does for
///   any check with a bridge readout.
///
/// Nothing is touched unless the check's executable is the managed shim for
/// this tool and the crate labeled it bundled: a copy that resolved elsewhere
/// keeps its source-aware readout, is flagged as missing the managed copy
/// (see [`flag_missing_managed_copy`]), and has its update redirected to the
/// managed install (see [`redirect_unmanaged_update_to_managed_install`]). An
/// unknown installed or latest version leaves `update_available` unknown
/// rather than false.
fn apply_managed_bridge_readout(
    check: &mut DoctorCheck,
    versions: &ManagedBridgeVersions,
    shim_bin_dir: &Path,
    check_freshness: bool,
) -> bool {
    let Some(path) = check.path.clone() else {
        return false;
    };
    if !is_managed_shim_path(&path, shim_bin_dir, versions.tool.binary) {
        return false;
    }
    if check.main.as_ref().and_then(|main| main.bundled) != Some(true) {
        return false;
    }

    let update_available = if check_freshness {
        newer_version_available(versions.installed.as_deref(), versions.latest.as_deref())
    } else {
        None
    };
    let update_command = match (&update_available, &versions.latest) {
        (Some(true), Some(latest)) => Some(managed_update_description(&versions.tool, latest)),
        _ => None,
    };
    let bridge = AgentVersionInfo {
        install_source: Some(InstallSource::Bundled),
        bundled: Some(true),
        installed_version: versions.installed.clone(),
        latest_version: versions.latest.clone(),
        update_available,
        // Only the freshness pass says anything about updates, matching the
        // crate; and what it says is "not self-updating": the update is the
        // user's to action here, unlike the crate's bundled default.
        self_updating: check_freshness.then_some(false),
        update_fix_type: update_command.as_ref().map(|_| FixType::UpdateBridge),
        update_command,
    };
    check.installed_version = bridge.installed_version.clone();
    check.latest_version = bridge.latest_version.clone();
    check.update_available = bridge.update_available;
    check.self_updating = bridge.self_updating;
    check.bridge_path = Some(path);
    check.bridge = Some(bridge);
    true
}

/// Point a non-managed copy's update at the managed installer, and report
/// whether it did.
///
/// Doctor can resolve one of these two checks to a copy of the bridge the user
/// installed themselves (Homebrew, their own npm prefix, nvm) — before the
/// launch reconcile has written the managed shim, or for as long as it keeps
/// failing. On a managing build the crate's source-aware `UpdateMain` must not
/// run: `npm install -g <pkg>@latest` under the fix env's `NPM_CONFIG_PREFIX`
/// would install a third copy into `~/.staged/packages/npm-prefix` that
/// nothing owns, and would not upgrade the user's copy the confirmation named.
///
/// So the action becomes the managed install, described as such, and only the
/// action: `path`, `install_source`, both versions and `update_available` stay
/// as the crate reported them, so the row keeps showing the copy that is
/// actually on PATH, where it is, and how old it is. The fix type is set even
/// when the crate derived no command (an `Unknown` source, say) — Staged can
/// install its own copy where the crate had nothing to offer.
///
/// The version the description names is the one the installer will fetch:
/// `versions.latest`, Staged's own `npm view <pkg> version` against the
/// configured registry, gathered alongside the managed readout. The crate's
/// `latest_version` is derived per install source — `brew info` for a Homebrew
/// copy — and can name a release the configured registry does not (yet) carry,
/// or lag one it does; it is only the fallback for when the managed probe
/// failed, so the text at least names the release the row is flagged stale
/// against.
///
/// Only ever fires on the freshness pass, since `update_available` is unset
/// until then, and never on a management-off build, where
/// [`managed_bridge_versions`] returns nothing.
fn redirect_unmanaged_update_to_managed_install(
    check: &mut DoctorCheck,
    versions: &ManagedBridgeVersions,
    shim_bin_dir: &Path,
) -> bool {
    let Some(path) = check.path.clone() else {
        return false;
    };
    if is_managed_shim_path(&path, shim_bin_dir, versions.tool.binary) {
        return false;
    }
    let Some(main) = check.main.as_mut() else {
        return false;
    };
    if main.update_available != Some(true) {
        return false;
    }
    let latest = versions
        .latest
        .as_deref()
        .or(main.latest_version.as_deref());
    main.update_command = Some(managed_install_description(&versions.tool, latest, &path));
    main.update_fix_type = Some(FixType::UpdateMain);
    true
}

/// Whether `path` is the shim Staged writes for `binary` in `shim_bin_dir` —
/// the executable of a managed install, as opposed to a same-named copy that
/// resolved from anywhere else on PATH.
fn is_managed_shim_path(path: &str, shim_bin_dir: &Path, binary: &str) -> bool {
    let path = Path::new(path);
    path.starts_with(shim_bin_dir) && path.file_name().is_some_and(|name| name == binary)
}

/// What the update confirmation shows for a managed bridge. Not a shell
/// command: `run_doctor_update` dispatches this readout to the managed
/// installer, which installs `<pkg>@latest` into `~/.staged/packages` — so
/// the text says that, rather than offering a global `npm install -g` the
/// user might run by hand and end up with a second copy.
fn managed_update_description(tool: &ManagedTool, latest: &str) -> String {
    format!(
        "update Staged's managed {} to {latest} in ~/.staged/packages",
        tool.package
    )
}

/// What the update confirmation shows when the row is a copy Staged does not
/// manage: the managed copy is absent or stale, so the installer *installs*
/// rather than updates, and the text is explicit that `replaced` — the copy on
/// PATH, the one the version beside it was read from — is not touched.
fn managed_install_description(tool: &ManagedTool, latest: Option<&str>, replaced: &str) -> String {
    let version = latest
        .map(|latest| format!(" {latest}"))
        .unwrap_or_default();
    format!(
        "install Staged's managed {}{version} in ~/.staged/packages \
         (the copy at {replaced} is left as is)",
        tool.package
    )
}

/// `Some(true)` when `latest` is a newer release than `installed`, comparing
/// the two versions of one package as semver triples. `None` — unknown, not
/// "current" — when either side is missing or does not parse.
fn newer_version_available(installed: Option<&str>, latest: Option<&str>) -> Option<bool> {
    let installed = parse_version_triple(installed?)?;
    let latest = parse_version_triple(latest?)?;
    Some(latest > installed)
}

/// `X.Y.Z[-pre][+build]`, with an optional leading `v`, as `(X, Y, Z)`.
fn parse_version_triple(version: &str) -> Option<(u64, u64, u64)> {
    let core = version
        .trim()
        .trim_start_matches('v')
        .split(['-', '+', ' '])
        .next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

// =============================================================================
// Managed Node.js runtime check
// =============================================================================

const NODE_RUNTIME_CHECK_ID: &str = "node-runtime";
const NODE_RUNTIME_CHECK_LABEL: &str = "Node.js Runtime";
const SESSION_QUEUE_CHECK_ID: &str = "session-queue";
const SESSION_QUEUE_CHECK_LABEL: &str = "Session Queue";

/// The session-queue readout: what the stranded-session sweep sees right now
/// and how often it has had to act. `None` until the store is open — the
/// needs-reset flow runs the Doctor with no store managed.
fn session_queue_doctor_check<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Option<DoctorCheck> {
    let store = crate::get_store(&app_handle.try_state::<Mutex<Option<Arc<Store>>>>()?).ok()?;
    let registry = app_handle.try_state::<Arc<SessionRegistry>>()?;
    match sweep::queue_health(&store, &registry) {
        Ok(health) => Some(build_session_queue_check(&health)),
        Err(e) => {
            log::warn!("[doctor] session-queue check failed to read the store: {e}");
            None
        }
    }
}

/// `Warn` while any row would be swept right now; `Pass` otherwise, naming
/// the lifetime sweep count when it is non-zero so a recurrence stays
/// visible after the sweep has already cleaned up.
fn build_session_queue_check(health: &QueueHealth) -> DoctorCheck {
    let stranded = health.stranded.len();
    let (status, message) = if stranded > 0 {
        (
            CheckStatus::Warn,
            format!(
                "{} {} running with nothing in this process owning {}; the stranded-session sweep ends {} within a minute",
                stranded,
                if stranded == 1 { "session is" } else { "sessions are" },
                if stranded == 1 { "it" } else { "them" },
                if stranded == 1 { "it" } else { "them" },
            ),
        )
    } else if health.swept_total > 0 {
        (
            CheckStatus::Pass,
            format!(
                "No stranded sessions; the sweep has ended {} so far",
                if health.swept_total == 1 {
                    "1 session".to_string()
                } else {
                    format!("{} sessions", health.swept_total)
                }
            ),
        )
    } else {
        (CheckStatus::Pass, "No stranded sessions".to_string())
    };

    let mut detail = vec![
        "checked: running session rows owned by this process".to_string(),
        format!("owned running rows: {}", health.owned_running),
        format!(
            "of which owned by a registered runner or a liveness hold: {}",
            health.live
        ),
        format!("stranded now: {stranded}"),
    ];
    detail.extend(health.stranded.iter().map(|row| {
        format!(
            "- session {} (branch {})",
            row.session_id,
            row.branch_id.as_deref().unwrap_or("none"),
        )
    }));
    detail.push(format!("ended by the sweep so far: {}", health.swept_total));

    DoctorCheck {
        id: SESSION_QUEUE_CHECK_ID.to_string(),
        label: SESSION_QUEUE_CHECK_LABEL.to_string(),
        status,
        message,
        fix_url: None,
        fix_command: None,
        fix_type: None,
        path: None,
        bridge_path: None,
        raw_output: Some(detail.join("\n")),
        auth_status: None,
        login_command: None,
        installed_version: None,
        latest_version: None,
        update_available: None,
        install_source: None,
        self_updating: None,
        main: None,
        bridge: None,
    }
}

/// Disk states of the Staged-managed Node.js runtime the check reports on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManagedNodeRuntimeState {
    /// The pinned version is installed and answers the readiness probe.
    Ready,
    /// The pinned install dir exists but the probe fails — a crashed install
    /// or damaged tree that a reinstall repairs.
    Broken,
    /// The pinned version is not on disk (fresh profile, or a pin bump left
    /// only a superseded version behind).
    Missing,
}

/// Report the state of the Staged-managed Node.js runtime that npm-installed
/// agent tools run on, with a native fix that (re)installs the pinned
/// version (`run_doctor_fix` routes this check id to
/// `ensure_managed_node_runtime`). Silent when there is nothing to report:
/// an unsupported target, or a runtime that was never installed and no
/// Staged-installed npm tools that would need it.
async fn run_node_runtime_check() -> Option<DoctorCheck> {
    let node_root = crate::managed_node::managed_node_root()?;
    let install_dir = crate::managed_node::pinned_install_dir(&node_root)?;
    let state = if crate::managed_node::pinned_runtime_ready(&node_root).await {
        ManagedNodeRuntimeState::Ready
    } else if install_dir.exists() {
        ManagedNodeRuntimeState::Broken
    } else {
        ManagedNodeRuntimeState::Missing
    };
    // Both install families depend on the runtime: the private-prefix npm
    // tools (copilot, amp-acp) and the managed bridge shims, whose embedded
    // node paths break silently without it.
    let mut npm_tools: Vec<String> = [
        crate::managed_acp_tools::npm_prefix_bin_dir(),
        crate::managed_acp_tools::managed_shim_bin_dir(),
    ]
    .into_iter()
    .flatten()
    .flat_map(|dir| installed_npm_tool_names(&dir))
    .collect();
    npm_tools.sort();
    npm_tools.dedup();
    build_node_runtime_check(
        state,
        &install_dir,
        &npm_tools,
        last_reconcile_record().as_ref(),
    )
}

/// Names of the bin shims npm wrote into the Staged-private prefix — the
/// tools that need the managed runtime to run at all.
fn installed_npm_tool_names(bin_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(bin_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect()
}

/// Build the runtime row. `last_reconcile` is `state.json`'s record of the
/// last managed install run: a runtime that is `Missing` after a reconcile
/// that *failed* is not "never needed", it is the install that could not
/// happen — every managed bridge install starts by ensuring the runtime — so
/// that case warns with the recorded error even when nothing is installed
/// yet, instead of staying silent. With no record (first launch still
/// reconciling, or a profile that never ran one) a missing runtime is
/// reported only when installed tools need it, as before.
fn build_node_runtime_check(
    state: ManagedNodeRuntimeState,
    install_dir: &Path,
    npm_tools: &[String],
    last_reconcile: Option<&ReconcileRecord>,
) -> Option<DoctorCheck> {
    let version = &crate::managed_node::node_runtime_lock().version;
    let failed_reconcile = last_reconcile.filter(|record| !record.ok);
    let (status, message) = match (&state, failed_reconcile) {
        (ManagedNodeRuntimeState::Ready, _) => (
            CheckStatus::Pass,
            format!("Staged-managed Node.js {version} is installed"),
        ),
        (ManagedNodeRuntimeState::Broken, _) => (
            CheckStatus::Warn,
            format!("Staged-managed Node.js {version} is damaged; run the fix to reinstall it"),
        ),
        (ManagedNodeRuntimeState::Missing, Some(record)) => (
            CheckStatus::Warn,
            format!(
                "Staged-managed Node.js {version} could not be installed: {}",
                reconcile_headline(record)
            ),
        ),
        (ManagedNodeRuntimeState::Missing, None) if npm_tools.is_empty() => return None,
        (ManagedNodeRuntimeState::Missing, None) => (
            CheckStatus::Warn,
            format!(
                "Staged-managed Node.js {version} is not installed; Staged-installed agent tools require it"
            ),
        ),
    };

    let state_label = match state {
        ManagedNodeRuntimeState::Ready => "ready",
        ManagedNodeRuntimeState::Broken => "broken",
        ManagedNodeRuntimeState::Missing => "missing",
    };
    let mut detail = vec![
        "checked: Staged-managed Node.js runtime".to_string(),
        format!("pinned version: {version}"),
        format!("install dir: {}", install_dir.display()),
        format!("state: {state_label}"),
    ];
    if npm_tools.is_empty() {
        detail.push("Staged-installed npm tools: none".to_string());
    } else {
        detail.push("Staged-installed npm tools:".to_string());
        detail.extend(npm_tools.iter().map(|name| format!("- {name}")));
    }
    if let Some(record) = failed_reconcile {
        detail.push(format!(
            "last managed install: failed {} with {} error(s):",
            format_age(record.at_ms, crate::managed_acp_tools::now_ms()),
            record.errors.len()
        ));
        detail.extend(record.errors.iter().map(|error| format!("- {error}")));
        if let Some(hint) =
            crate::managed_acp_tools::install_failure_hint(&reconcile_headline(record), "Node.js")
        {
            detail.push(format!("hint: {hint}"));
        }
    }

    let node_path = (state == ManagedNodeRuntimeState::Ready)
        .then(|| install_dir.join("bin").join("node").display().to_string());
    // Native fix: `run_doctor_fix` routes this check id to
    // `ensure_managed_node_runtime`. The command string is what the fix
    // confirmation dialog displays, not a shell command.
    let fix = (status != CheckStatus::Pass).then(|| {
        (
            FixType::Command,
            format!("download and install Node.js {version} into ~/.staged/packages"),
        )
    });
    Some(node_runtime_doctor_check(
        status,
        message,
        node_path,
        Some(detail.join("\n")),
        fix,
    ))
}

/// The one error a failed reconcile is summarised by on the runtime row: the
/// first that names the Node.js runtime download — the failure every bridge
/// shares when the runtime could not be fetched — else the first recorded,
/// in both cases without the reconciler's `<tool id>: ` prefix.
fn reconcile_headline(record: &ReconcileRecord) -> String {
    record
        .errors
        .iter()
        .find(|error| error.contains("Node.js runtime"))
        .or_else(|| record.errors.first())
        .map(|error| strip_tool_id_prefix(error).to_string())
        .unwrap_or_else(|| "no error recorded".to_string())
}

/// `<tool id>: <error>` → `<error>`, for the ids this build knows; anything
/// else is returned whole.
fn strip_tool_id_prefix(error: &str) -> &str {
    error
        .split_once(": ")
        .filter(|(id, _)| {
            crate::managed_acp_tools::MANAGED_TOOLS
                .iter()
                .any(|tool| tool.id == *id)
        })
        .map(|(_, rest)| rest)
        .unwrap_or(error)
}

fn node_runtime_doctor_check(
    status: CheckStatus,
    message: String,
    path: Option<String>,
    raw_output: Option<String>,
    fix: Option<(FixType, String)>,
) -> DoctorCheck {
    let (fix_type, fix_command) = fix.map(|(t, c)| (Some(t), Some(c))).unwrap_or((None, None));
    DoctorCheck {
        id: NODE_RUNTIME_CHECK_ID.to_string(),
        label: NODE_RUNTIME_CHECK_LABEL.to_string(),
        status,
        message,
        fix_url: None,
        fix_command,
        fix_type,
        path,
        bridge_path: None,
        raw_output,
        auth_status: None,
        login_command: None,
        installed_version: None,
        latest_version: None,
        update_available: None,
        install_source: None,
        self_updating: None,
        main: None,
        bridge: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned_version() -> String {
        crate::managed_node::node_runtime_lock().version.clone()
    }

    fn queue_health(stranded: Vec<sweep::StrandedRow>, swept_total: usize) -> QueueHealth {
        QueueHealth {
            owned_running: stranded.len() + 1,
            live: 1,
            stranded,
            swept_total,
        }
    }

    #[test]
    fn session_queue_check_passes_when_nothing_is_stranded() {
        let check = build_session_queue_check(&queue_health(Vec::new(), 0));

        assert_eq!(check.id, SESSION_QUEUE_CHECK_ID);
        assert_eq!(check.status, CheckStatus::Pass);
        assert_eq!(check.message, "No stranded sessions");
        assert!(check.fix_type.is_none());
        let detail = check.raw_output.expect("detail is attached");
        assert!(detail.contains("owned running rows: 1"), "{detail}");
        assert!(detail.contains("stranded now: 0"), "{detail}");
        assert!(detail.contains("ended by the sweep so far: 0"), "{detail}");
    }

    /// A sweep that has already acted leaves nothing stranded, so the status
    /// stays `Pass` — but the count is what makes a recurrence visible.
    #[test]
    fn session_queue_check_reports_past_sweeps_without_warning() {
        let check = build_session_queue_check(&queue_health(Vec::new(), 2));

        assert_eq!(check.status, CheckStatus::Pass);
        assert_eq!(
            check.message,
            "No stranded sessions; the sweep has ended 2 sessions so far"
        );
    }

    #[test]
    fn session_queue_check_warns_while_a_row_is_stranded() {
        let row = sweep::StrandedRow {
            session_id: "s-1".to_string(),
            branch_id: Some("b-1".to_string()),
        };
        let check = build_session_queue_check(&queue_health(vec![row], 0));

        assert_eq!(check.status, CheckStatus::Warn);
        assert_eq!(
            check.message,
            "1 session is running with nothing in this process owning it; the stranded-session sweep ends it within a minute"
        );
        let detail = check.raw_output.expect("detail is attached");
        assert!(detail.contains("- session s-1 (branch b-1)"), "{detail}");
        assert!(
            detail.contains("of which owned by a registered runner or a liveness hold: 1"),
            "{detail}"
        );
    }

    #[test]
    fn ready_runtime_passes_without_a_fix() {
        let check = build_node_runtime_check(
            ManagedNodeRuntimeState::Ready,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &["copilot".to_string()],
            None,
        )
        .expect("ready runtime is reported");

        assert_eq!(check.status, CheckStatus::Pass);
        assert_eq!(
            check.message,
            format!("Staged-managed Node.js {} is installed", pinned_version())
        );
        assert_eq!(
            check.path.as_deref(),
            Some("/data/packages/node/v9.9.9/plat/bin/node")
        );
        assert!(check.fix_type.is_none());
        assert!(check.fix_command.is_none());
        assert!(check.fix_url.is_none());
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("state: ready"));
        assert!(output.contains("- copilot"));
    }

    #[test]
    fn damaged_runtime_warns_with_a_native_reinstall_fix() {
        let check = build_node_runtime_check(
            ManagedNodeRuntimeState::Broken,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &[],
            None,
        )
        .expect("damaged runtime is reported");

        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.message.contains("is damaged"));
        assert!(check.path.is_none());
        assert_eq!(check.fix_type, Some(FixType::Command));
        let fix_command = check.fix_command.as_deref().expect("fix command");
        assert!(fix_command.contains(&pinned_version()));
        assert!(fix_command.contains("~/.staged/packages"));
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("state: broken"));
        assert!(output.contains("Staged-installed npm tools: none"));
    }

    #[test]
    fn missing_runtime_warns_only_when_installed_tools_need_it() {
        // Tools installed into the private prefix need the runtime: warn with
        // the reinstall fix.
        let check = build_node_runtime_check(
            ManagedNodeRuntimeState::Missing,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &["amp-acp".to_string(), "copilot".to_string()],
            None,
        )
        .expect("needed-but-missing runtime is reported");
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.message.contains("is not installed"));
        assert_eq!(check.fix_type, Some(FixType::Command));
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("- amp-acp"));
        assert!(output.contains("- copilot"));
        assert!(!output.contains("last managed install"), "{output}");

        // Nothing installed that needs it, and no reconcile on record (or a
        // clean one): stay silent.
        for record in [None, Some(reconcile_record(true, Vec::new()))] {
            assert!(build_node_runtime_check(
                ManagedNodeRuntimeState::Missing,
                Path::new("/data/packages/node/v9.9.9/plat"),
                &[],
                record.as_ref(),
            )
            .is_none());
        }
    }

    fn reconcile_record(ok: bool, errors: Vec<&str>) -> ReconcileRecord {
        ReconcileRecord {
            at_ms: 1_000,
            ok,
            errors: errors.into_iter().map(str::to_string).collect(),
        }
    }

    /// A runtime that is missing *because* the last reconcile failed is the
    /// install that could not happen, not one that was never needed: the row
    /// warns with the recorded error even with nothing installed yet, lists
    /// every error (and the URL they name) in its detail, and keeps the
    /// reinstall fix.
    #[test]
    fn missing_runtime_after_a_failed_reconcile_warns_with_the_recorded_errors() {
        let node_error = "claude-acp: Node.js runtime download failed: \
             https://nodejs.org/dist/v9.9.9/node-v9.9.9-darwin-arm64.tar.gz: \
             error sending request for url";
        let record = reconcile_record(
            false,
            vec![
                "codex-acp: npm install failed: npm install exited with exit status: 1",
                node_error,
            ],
        );
        let check = build_node_runtime_check(
            ManagedNodeRuntimeState::Missing,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &[],
            Some(&record),
        )
        .expect("a failed install is reported");

        assert_eq!(check.status, CheckStatus::Warn);
        assert_eq!(
            check.message,
            format!(
                "Staged-managed Node.js {} could not be installed: Node.js runtime download \
                 failed: https://nodejs.org/dist/v9.9.9/node-v9.9.9-darwin-arm64.tar.gz: \
                 error sending request for url",
                pinned_version()
            ),
            "leads with the runtime download error, minus the tool id"
        );
        assert_eq!(check.fix_type, Some(FixType::Command));
        assert!(check.fix_command.is_some());
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("state: missing"), "{output}");
        assert!(output.contains("last managed install: failed"), "{output}");
        assert!(output.contains("with 2 error(s):"), "{output}");
        assert!(output.contains(&format!("- {node_error}")), "{output}");
        assert!(
            output.contains("- codex-acp: npm install failed"),
            "{output}"
        );
        assert!(
            output.contains("hint: the configured registry/host did not respond"),
            "{output}"
        );

        // With no runtime-specific error the first recorded one is the
        // headline, still without its tool id.
        let record = reconcile_record(
            false,
            vec!["claude-acp: installed ACP bridge is incomplete: probe exited with 1"],
        );
        let check = build_node_runtime_check(
            ManagedNodeRuntimeState::Missing,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &[],
            Some(&record),
        )
        .unwrap();
        assert!(
            check.message.ends_with(
                "could not be installed: installed ACP bridge is incomplete: probe exited with 1"
            ),
            "{}",
            check.message
        );
        assert!(!check.raw_output.as_deref().unwrap().contains("hint:"));
    }

    #[test]
    fn format_age_rounds_to_the_largest_whole_unit() {
        assert_eq!(format_age(1_000, 1_000), "just now");
        assert_eq!(format_age(0, 59_000), "just now");
        assert_eq!(format_age(0, 60_000), "1 minute ago");
        assert_eq!(format_age(0, 2 * 3_600_000 + 5), "2 hours ago");
        assert_eq!(format_age(0, 3 * 86_400_000), "3 days ago");
        // A clock that went backwards is not a panic.
        assert_eq!(format_age(5_000, 1_000), "just now");
    }

    #[test]
    fn installed_npm_tool_names_lists_visible_entries_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("copilot"), "").unwrap();
        std::fs::write(dir.path().join(".copilot.tmp"), "").unwrap();

        let names = installed_npm_tool_names(dir.path());
        assert_eq!(names, vec!["copilot".to_string()]);

        // An absent dir reads as no tools, not an error.
        assert!(installed_npm_tool_names(&dir.path().join("absent")).is_empty());
    }

    /// Bundled-readout labeling lives in the doctor crate; Staged's job is
    /// only to hand the managed shim dir into the run options.
    #[test]
    fn run_checks_options_carries_bundled_tools_dir() {
        let dir = PathBuf::from("/data/packages/bin");
        let opts = run_checks_options(false, Vec::new(), Some(dir.clone()));
        assert_eq!(opts.bundled_tools_dir, Some(dir));

        let opts = run_checks_options(false, Vec::new(), None);
        assert!(opts.bundled_tools_dir.is_none());
    }

    /// Fixes and updates for the two bridge checks route to the managed
    /// installer exactly when this build manages bridges; every other check
    /// keeps the doctor crate's regular fix commands.
    #[test]
    fn managed_bridge_checks_route_to_the_managed_installer() {
        let managed = crate::managed_acp_tools::managed_tools_enabled();
        assert_eq!(
            managed_tool_for_check("ai-agent-claude"),
            managed.then_some("claude-acp")
        );
        assert_eq!(
            managed_tool_for_check("ai-agent-codex"),
            managed.then_some("codex-acp")
        );
        assert_eq!(managed_tool_for_check("ai-agent-copilot"), None);
        assert_eq!(managed_tool_for_check("ai-agent-amp"), None);
        assert_eq!(managed_tool_for_check(NODE_RUNTIME_CHECK_ID), None);
    }

    /// Both update fix types on a managed check dispatch to the managed
    /// installer — `UpdateBridge` from the managed row's ACP readout, and the
    /// `UpdateMain` a copy that resolved outside the managed dir carries, whose
    /// own `npm install -g` would orphan a copy in the private prefix. Non-update
    /// fix types take the command-validated path, and nothing is managed when
    /// bridge management is off (dev override, `no-managed-acp-tools`,
    /// unsupported target).
    #[test]
    fn both_update_fix_types_dispatch_to_the_managed_installer() {
        let managed = crate::managed_acp_tools::managed_tools_enabled();
        for fix_type in [FixType::UpdateMain, FixType::UpdateBridge] {
            assert_eq!(
                managed_update_target("ai-agent-claude", &fix_type),
                managed.then_some("claude-acp"),
                "{fix_type:?}"
            );
            assert_eq!(
                managed_update_target("ai-agent-codex", &fix_type),
                managed.then_some("codex-acp"),
                "{fix_type:?}"
            );
            // Checks whose bridge Staged never manages keep the crate's own
            // fixes for both update types.
            assert_eq!(
                managed_update_target("ai-agent-amp", &fix_type),
                None,
                "{fix_type:?}"
            );
            assert_eq!(
                managed_update_target("ai-agent-copilot", &fix_type),
                None,
                "{fix_type:?}"
            );
        }
        for fix_type in [FixType::Command, FixType::Bridge, FixType::Auth] {
            assert_eq!(
                managed_update_target("ai-agent-claude", &fix_type),
                None,
                "{fix_type:?}"
            );
        }
    }

    // -- managed bridge readouts ---------------------------------------------

    fn claude_tool() -> ManagedTool {
        ManagedTool {
            id: "claude-acp",
            binary: "claude-agent-acp",
            package: "@agentclientprotocol/claude-agent-acp",
        }
    }

    const SHIM_BIN_DIR: &str = "/data/.staged/packages/bin";
    /// The vendored Claude Code version the crate's freshness pass probes
    /// through `claude-agent-acp --cli --version` — a different identity from
    /// the ACP package's own version.
    const VENDORED_CLI_VERSION: &str = "2.1.205";

    /// A Claude row as the doctor crate reports the managed shim: bundled
    /// `main`, no `bridge`. With `freshness`, the vendored CLI's probed version
    /// and the crate's bundled suppression (`self_updating`, no update).
    fn managed_claude_check(path: &str, freshness: bool) -> DoctorCheck {
        DoctorCheck {
            id: "ai-agent-claude".to_string(),
            label: "Claude Code".to_string(),
            status: CheckStatus::Pass,
            message: "Installed".to_string(),
            fix_url: None,
            fix_command: None,
            fix_type: None,
            path: Some(path.to_string()),
            bridge_path: None,
            raw_output: None,
            auth_status: Some(AuthStatus::Authenticated),
            login_command: Some("claude-agent-acp --cli auth login".to_string()),
            installed_version: freshness.then(|| VENDORED_CLI_VERSION.to_string()),
            latest_version: None,
            update_available: None,
            install_source: Some(InstallSource::Bundled),
            self_updating: freshness.then_some(true),
            main: Some(AgentVersionInfo {
                install_source: Some(InstallSource::Bundled),
                bundled: Some(true),
                installed_version: freshness.then(|| VENDORED_CLI_VERSION.to_string()),
                self_updating: freshness.then_some(true),
                ..AgentVersionInfo::default()
            }),
            bridge: None,
        }
    }

    fn managed_shim_path() -> String {
        Path::new(SHIM_BIN_DIR)
            .join("claude-agent-acp")
            .display()
            .to_string()
    }

    /// The ACP package version a copy the user installed themselves reports.
    const USER_COPY_VERSION: &str = "0.16.2";

    /// A Claude row as the doctor crate reports a copy the *user* installed —
    /// found on PATH before the launch reconcile has written the managed shim,
    /// or for as long as that reconcile keeps failing. Labeled by source rather
    /// than bundled, with the crate's freshness fields: `latest` as the registry
    /// answered it, and `command` the source-aware update the crate derived
    /// (`None` for a source it cannot update, such as `Unknown`).
    fn user_copy_claude_check(
        path: &str,
        source: &InstallSource,
        latest: Option<&str>,
        command: Option<&str>,
    ) -> DoctorCheck {
        let main = AgentVersionInfo {
            install_source: Some(source.clone()),
            bundled: None,
            installed_version: Some(USER_COPY_VERSION.to_string()),
            latest_version: latest.map(str::to_string),
            update_available: latest.map(|latest| latest != USER_COPY_VERSION),
            self_updating: Some(false),
            update_command: command.map(str::to_string),
            update_fix_type: command.map(|_| FixType::UpdateMain),
        };
        DoctorCheck {
            installed_version: main.installed_version.clone(),
            latest_version: main.latest_version.clone(),
            update_available: main.update_available,
            install_source: Some(source.clone()),
            self_updating: Some(false),
            main: Some(main),
            ..managed_claude_check(path, true)
        }
    }

    /// Run the report-wide pass over a single check, so the tests exercise the
    /// managed-readout and redirect branches through the same dispatch the
    /// report does.
    fn apply_to(
        check: DoctorCheck,
        versions: &ManagedBridgeVersions,
        check_freshness: bool,
    ) -> DoctorCheck {
        apply_with(
            check,
            versions,
            check_freshness,
            &ManagedInstallDiagnostics::default(),
        )
    }

    fn apply_with(
        check: DoctorCheck,
        versions: &ManagedBridgeVersions,
        check_freshness: bool,
        diagnostics: &ManagedInstallDiagnostics,
    ) -> DoctorCheck {
        let mut report = DoctorReport {
            checks: vec![check],
        };
        apply_managed_bridge_readouts(
            &mut report,
            std::slice::from_ref(versions),
            Path::new(SHIM_BIN_DIR),
            check_freshness,
            diagnostics,
        );
        report.checks.pop().unwrap()
    }

    /// Undo what [`flag_missing_managed_copy`] sets on a user-copy row, so a
    /// test can assert that *nothing else* changed.
    fn without_missing_copy_flag(mut check: DoctorCheck) -> DoctorCheck {
        check.status = CheckStatus::Pass;
        check.message = "Installed".to_string();
        check.fix_type = None;
        check.fix_command = None;
        check.raw_output = None;
        check
    }

    fn versions(installed: Option<&str>, latest: Option<&str>) -> ManagedBridgeVersions {
        ManagedBridgeVersions {
            tool: claude_tool(),
            installed: installed.map(str::to_string),
            latest: latest.map(str::to_string),
        }
    }

    fn json(check: &DoctorCheck) -> serde_json::Value {
        serde_json::to_value(check).unwrap()
    }

    /// The initial (no-network) report already carries both readouts: the
    /// crate's `main`, untouched, and the ACP package's installed version
    /// under `bridge`, with `bridge_path` set so the row renders it — the
    /// same structure the freshness report has, so the frontend's merge (which
    /// copies `bridge` but not `bridge_path`) never hides a readout.
    #[test]
    fn initial_report_adds_the_acp_readout_without_update_fields() {
        let path = managed_shim_path();
        let mut check = managed_claude_check(&path, false);
        let main_before = check.main.clone();

        assert!(apply_managed_bridge_readout(
            &mut check,
            &versions(Some("0.16.2"), None),
            Path::new(SHIM_BIN_DIR),
            false,
        ));

        assert_eq!(
            check.main, main_before,
            "the vendored-agent readout is preserved"
        );
        assert_eq!(check.bridge_path.as_deref(), Some(path.as_str()));
        let bridge = check.bridge.as_ref().expect("the ACP readout is added");
        assert_eq!(bridge.install_source, Some(InstallSource::Bundled));
        assert_eq!(bridge.bundled, Some(true));
        assert_eq!(bridge.installed_version.as_deref(), Some("0.16.2"));
        assert_eq!(bridge.latest_version, None);
        assert_eq!(bridge.update_available, None);
        assert_eq!(bridge.self_updating, None);
        assert_eq!(bridge.update_command, None);
        assert_eq!(bridge.update_fix_type, None);
        // The flat fields mirror the ACP readout, as the crate does for any
        // check with a bridge.
        assert_eq!(check.installed_version.as_deref(), Some("0.16.2"));
        assert_eq!(check.update_available, None);
    }

    /// The freshness report shows the two different versions side by side and
    /// attaches exactly one update — to the ACP readout — when the registry has
    /// a newer release of the same package. The text describes updating
    /// Staged's managed copy; it is not an npm command.
    #[test]
    fn freshness_with_a_newer_release_offers_one_update_on_the_acp_readout() {
        let path = managed_shim_path();
        let mut check = managed_claude_check(&path, true);

        assert!(apply_managed_bridge_readout(
            &mut check,
            &versions(Some("0.16.2"), Some("0.17.0")),
            Path::new(SHIM_BIN_DIR),
            true,
        ));

        let main = check.main.as_ref().unwrap();
        let bridge = check.bridge.as_ref().unwrap();
        assert_eq!(
            main.installed_version.as_deref(),
            Some(VENDORED_CLI_VERSION)
        );
        assert_eq!(bridge.installed_version.as_deref(), Some("0.16.2"));
        assert_eq!(bridge.latest_version.as_deref(), Some("0.17.0"));
        assert_eq!(bridge.update_available, Some(true));
        assert_eq!(bridge.self_updating, Some(false));
        assert_eq!(bridge.update_fix_type, Some(FixType::UpdateBridge));
        let command = bridge.update_command.as_deref().expect("actionable update");
        assert!(
            command.contains("@agentclientprotocol/claude-agent-acp"),
            "{command}"
        );
        assert!(command.contains("0.17.0"), "{command}");
        assert!(command.contains("~/.staged/packages"), "{command}");
        assert!(!command.contains("npm install"), "{command}");
        // Exactly one actionable readout: the agent vendored inside the same
        // package is never offered a second install of its own.
        assert_eq!(main.update_available, None);
        assert_eq!(main.update_command, None);
        assert_eq!(main.update_fix_type, None);
        assert_eq!(main.self_updating, Some(true));
        // The flat headline follows the ACP readout.
        assert_eq!(check.installed_version.as_deref(), Some("0.16.2"));
        assert_eq!(check.latest_version.as_deref(), Some("0.17.0"));
        assert_eq!(check.update_available, Some(true));
        assert_eq!(check.self_updating, Some(false));
    }

    #[test]
    fn freshness_with_the_current_release_reports_no_update() {
        let path = managed_shim_path();
        let mut check = managed_claude_check(&path, true);

        assert!(apply_managed_bridge_readout(
            &mut check,
            &versions(Some("0.17.0"), Some("0.17.0")),
            Path::new(SHIM_BIN_DIR),
            true,
        ));

        let bridge = check.bridge.as_ref().unwrap();
        assert_eq!(bridge.installed_version.as_deref(), Some("0.17.0"));
        assert_eq!(bridge.latest_version.as_deref(), Some("0.17.0"));
        assert_eq!(bridge.update_available, Some(false));
        assert_eq!(bridge.update_command, None);
        assert_eq!(bridge.update_fix_type, None);
        assert_eq!(check.update_available, Some(false));

        // A registry that is *behind* the install (a just-published version
        // not yet on the mirror) is likewise not an update.
        let mut check = managed_claude_check(&path, true);
        apply_managed_bridge_readout(
            &mut check,
            &versions(Some("0.17.1"), Some("0.17.0")),
            Path::new(SHIM_BIN_DIR),
            true,
        );
        assert_eq!(check.bridge.as_ref().unwrap().update_available, Some(false));
    }

    /// A failed or absent registry answer — and an unreadable installed
    /// version — leave `update_available` unknown, never "up to date", while
    /// whatever *was* read is still shown.
    #[test]
    fn unknown_installed_or_latest_versions_stay_unknown() {
        let path = managed_shim_path();

        let mut check = managed_claude_check(&path, true);
        assert!(apply_managed_bridge_readout(
            &mut check,
            &versions(Some("0.16.2"), None),
            Path::new(SHIM_BIN_DIR),
            true,
        ));
        let bridge = check.bridge.as_ref().unwrap();
        assert_eq!(bridge.installed_version.as_deref(), Some("0.16.2"));
        assert_eq!(bridge.latest_version, None);
        assert_eq!(bridge.update_available, None);
        assert_eq!(bridge.update_command, None);
        assert_eq!(check.update_available, None);

        let mut check = managed_claude_check(&path, true);
        assert!(apply_managed_bridge_readout(
            &mut check,
            &versions(None, Some("0.17.0")),
            Path::new(SHIM_BIN_DIR),
            true,
        ));
        let bridge = check.bridge.as_ref().unwrap();
        assert_eq!(bridge.installed_version, None);
        assert_eq!(bridge.latest_version.as_deref(), Some("0.17.0"));
        assert_eq!(bridge.update_available, None);
        assert_eq!(bridge.update_command, None);

        // Unparseable output on either side is not compared as a string.
        let mut check = managed_claude_check(&path, true);
        apply_managed_bridge_readout(
            &mut check,
            &versions(Some("0.16.2"), Some("latest")),
            Path::new(SHIM_BIN_DIR),
            true,
        );
        assert_eq!(check.bridge.as_ref().unwrap().update_available, None);
    }

    /// A copy the user installed themselves never gets the managed readout.
    /// It is flagged — the managed copy is missing, and the managed install is
    /// its fix — but nothing else changes: its own location, source and
    /// version are the honest ones for what is on PATH, and with nothing to
    /// update no update action is invented.
    #[test]
    fn a_user_copy_without_an_update_is_flagged_and_otherwise_left_alone() {
        let installed = versions(Some("0.16.2"), Some("0.17.0"));
        let path = "/opt/homebrew/bin/claude-agent-acp";

        // The registry answered, and the user copy is current.
        let check =
            user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
        let before = json(&check);
        let after = apply_to(check, &installed, true);
        assert_eq!(after.status, CheckStatus::Warn);
        assert_eq!(
            after.message,
            "Using /opt/homebrew/bin/claude-agent-acp — Staged's managed copy is missing"
        );
        assert_eq!(after.fix_type, Some(FixType::Bridge));
        assert_eq!(
            after.fix_command.as_deref(),
            Some(
                "install Staged's managed @agentclientprotocol/claude-agent-acp 0.17.0 in \
                 ~/.staged/packages (the copy at /opt/homebrew/bin/claude-agent-acp is left as is)"
            )
        );
        let detail = after.raw_output.as_deref().expect("detail");
        assert!(
            detail.contains("Staged-managed copy: missing (expected /data/.staged/packages/bin/claude-agent-acp)"),
            "{detail}"
        );
        assert!(
            detail.contains("last managed install: none recorded yet"),
            "{detail}"
        );
        assert!(!detail.contains("hint:"), "{detail}");
        assert_eq!(json(&without_missing_copy_flag(after)), before);

        // The registry lookup failed: unknown, so no action is invented.
        let check = user_copy_claude_check(path, &InstallSource::Brew, None, None);
        let before = json(&check);
        let after = apply_to(check, &installed, true);
        assert_eq!(after.fix_type, Some(FixType::Bridge));
        assert_eq!(json(&without_missing_copy_flag(after)), before);

        // The initial (no-network) pass sets no update fields at all, so there
        // is nothing to redirect there either — but the flag and fix do not
        // wait for the network: the base report already carries them.
        let mut check = managed_claude_check(path, false);
        check.main.as_mut().unwrap().bundled = None;
        check.main.as_mut().unwrap().install_source = Some(InstallSource::Npm);
        let before = json(&check);
        let after = apply_to(check, &installed, false);
        assert_eq!(after.status, CheckStatus::Warn);
        assert_eq!(after.fix_type, Some(FixType::Bridge));
        assert_eq!(json(&without_missing_copy_flag(after)), before);
    }

    /// With a failed reconcile on record the row says why the managed copy is
    /// missing — the recorded error for *this* tool, which names the registry
    /// it used — and the detail adds the classified hint and the registry
    /// Doctor detected. The other tool's error stays off this row.
    #[test]
    fn a_user_copy_names_the_recorded_install_error_and_its_hint() {
        let installed = versions(None, None);
        let path = "/opt/homebrew/bin/claude-agent-acp";
        let claude_error = "npm install failed: npm install exited with exit status: 1 \
             (registry: npm default (per npm config)): npm error code FETCH_ERROR; \
             invalid json response body at \
             https://registry.npmjs.org/@agentclientprotocol%2fclaude-agent-acp reason: \
             Unexpected token '<'";
        let diagnostics = ManagedInstallDiagnostics {
            last_reconcile: Some(ReconcileRecord {
                at_ms: 0,
                ok: false,
                errors: vec![
                    format!("claude-acp: {claude_error}"),
                    "codex-acp: npm install failed: npm install exited with exit status: 1 \
                     (registry: npm default (per npm config)): npm error code E404"
                        .to_string(),
                ],
            }),
            npm_registry: "https://registry.npmjs.org/ (per npm config)".to_string(),
            now_ms: 5 * 60_000,
        };

        let check =
            user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
        let after = apply_with(check, &installed, true, &diagnostics);

        assert_eq!(after.status, CheckStatus::Warn);
        assert_eq!(
            after.message,
            format!("Using {path} — Staged's managed copy is missing: {claude_error}")
        );
        assert_eq!(after.fix_type, Some(FixType::Bridge));
        let detail = after.raw_output.as_deref().expect("detail");
        assert!(
            detail.contains("last managed install: failed 5 minutes ago"),
            "{detail}"
        );
        assert!(
            detail.contains(&format!("claude-acp: {claude_error}")),
            "{detail}"
        );
        assert!(!detail.contains("codex-acp"), "{detail}");
        assert!(
            detail.contains(
                "hint: this network blocks registry.npmjs.org and npm has no registry configured"
            ),
            "{detail}"
        );
        assert!(
            detail.contains("npm registry: https://registry.npmjs.org/ (per npm config)"),
            "{detail}"
        );

        // A row that already warns (installed, not signed in) keeps its own
        // message in front and its status.
        let mut check =
            user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
        check.status = CheckStatus::Warn;
        check.message = "Installed, not authenticated".to_string();
        check.raw_output =
            Some("strategy 0: PATH => /opt/homebrew/bin/claude-agent-acp".to_string());
        let after = apply_with(check, &installed, true, &diagnostics);
        assert_eq!(after.status, CheckStatus::Warn);
        assert_eq!(
            after.message,
            format!(
                "Installed, not authenticated; using {path} — Staged's managed copy is missing: {claude_error}"
            )
        );
        let detail = after.raw_output.as_deref().unwrap();
        assert!(
            detail.starts_with("strategy 0: PATH => /opt/homebrew/bin/claude-agent-acp\n\nStaged-managed copy: missing"),
            "the crate's trace stays on top: {detail}"
        );
    }

    /// When this build does not manage bridges at all, a Homebrew copy is by
    /// design: the rows say so and keep their status, and no fix is added.
    /// Rows for other agents are untouched.
    #[test]
    fn managed_installs_disabled_is_noted_on_the_bridge_rows_without_a_downgrade() {
        let path = "/opt/homebrew/bin/claude-agent-acp";
        let mut claude =
            user_copy_claude_check(path, &InstallSource::Brew, Some(USER_COPY_VERSION), None);
        claude.raw_output =
            Some("strategy 0: PATH => /opt/homebrew/bin/claude-agent-acp".to_string());
        let mut codex = user_copy_claude_check(
            "/opt/homebrew/bin/codex-acp",
            &InstallSource::Brew,
            None,
            None,
        );
        codex.id = "ai-agent-codex".to_string();
        codex.status = CheckStatus::Fail;
        codex.message = "Not installed (optional)".to_string();
        codex.path = None;
        let amp = DoctorCheck {
            id: "ai-agent-amp".to_string(),
            ..claude.clone()
        };
        let amp_before = json(&amp);
        let mut report = DoctorReport {
            checks: vec![claude, codex, amp],
        };

        note_managed_installs_disabled(&mut report, "STAGED_ACP_TOOLS_DIR is set to /dev/acp");

        let claude = &report.checks[0];
        assert_eq!(claude.status, CheckStatus::Pass);
        assert_eq!(
            claude.message,
            "Installed (managed installs disabled: STAGED_ACP_TOOLS_DIR is set to /dev/acp)"
        );
        assert!(claude.fix_type.is_none());
        assert_eq!(
            claude.raw_output.as_deref(),
            Some(
                "strategy 0: PATH => /opt/homebrew/bin/claude-agent-acp\n\n\
                 managed installs disabled: STAGED_ACP_TOOLS_DIR is set to /dev/acp"
            )
        );
        let codex = &report.checks[1];
        assert_eq!(codex.status, CheckStatus::Fail);
        assert_eq!(
            codex.message,
            "Not installed (optional) (managed installs disabled: STAGED_ACP_TOOLS_DIR is set to /dev/acp)"
        );
        assert_eq!(json(&report.checks[2]), amp_before);
    }

    /// A user copy with a newer release keeps its readout but has its action
    /// redirected: on a managing build the managed installer is the only writer
    /// for these two packages, so the crate's `npm install -g` (which would
    /// orphan a copy in the private prefix) and `brew upgrade` (which Staged
    /// does not run for a copy it never uses) are replaced by a description of
    /// the managed install. A source the crate cannot update gets the action it
    /// had none for.
    ///
    /// The version the description names is the managed probe's — what the
    /// installer will actually fetch from the configured registry — not the
    /// crate's per-source answer (`brew info` for a Homebrew copy), which the
    /// readout itself keeps showing.
    #[test]
    fn a_user_copy_with_a_newer_release_is_redirected_to_the_managed_install() {
        // The configured registry is one release ahead of what the crate's
        // source-aware lookup found for the user's copy.
        let installed = versions(Some("0.16.2"), Some("0.17.1"));
        let cases = [
            (
                "/Users/u/.nvm/versions/node/v22.14.0/bin/claude-agent-acp",
                InstallSource::Npm,
                Some("npm install -g @agentclientprotocol/claude-agent-acp@latest"),
            ),
            (
                "/opt/homebrew/bin/claude-agent-acp",
                InstallSource::Brew,
                Some("brew upgrade claude-agent-acp"),
            ),
            (
                "/usr/local/bin/claude-agent-acp",
                InstallSource::Unknown,
                None,
            ),
        ];

        for (path, source, command) in &cases {
            let check = user_copy_claude_check(path, source, Some("0.17.0"), *command);
            let before = json(&check);
            let mut after = apply_to(check, &installed, true);

            assert!(after.bridge.is_none(), "{source:?}");
            assert!(after.bridge_path.is_none(), "{source:?}");
            let main = after.main.as_ref().unwrap();
            assert_eq!(
                main.update_fix_type,
                Some(FixType::UpdateMain),
                "{source:?}"
            );
            let description = main.update_command.as_deref().expect("an action");
            assert!(
                description.contains("@agentclientprotocol/claude-agent-acp"),
                "{description}"
            );
            assert!(
                description.contains("0.17.1"),
                "names the registry release the installer fetches: {description}"
            );
            assert!(
                !description.contains("0.17.0"),
                "not the crate's per-source answer: {description}"
            );
            assert!(description.contains("~/.staged/packages"), "{description}");
            assert!(description.contains(path), "{description}");
            assert!(!description.contains("npm install"), "{description}");
            assert!(!description.contains("brew upgrade"), "{description}");

            // The row's fix is the same managed install, word for word, so the
            // Update and Fix buttons never describe two different things.
            assert_eq!(after.fix_type, Some(FixType::Bridge), "{source:?}");
            assert_eq!(
                after.fix_command.as_deref(),
                Some(description),
                "{source:?}"
            );

            // The action (and the missing-copy flag) are the *only* changes:
            // path, source, both versions and `updateAvailable` still describe
            // the copy that is on PATH — the readout's `latestVersion` stays
            // the crate's, even though the description names the registry's.
            let main = after.main.as_mut().unwrap();
            main.update_command = command.map(str::to_string);
            main.update_fix_type = command.map(|_| FixType::UpdateMain);
            assert_eq!(
                json(&without_missing_copy_flag(after)),
                before,
                "{source:?}"
            );
        }
    }

    /// When the managed probe failed — registry unreachable, garbage, timed
    /// out — the description falls back to the version the crate flagged the
    /// copy stale against, rather than naming nothing: the row is still
    /// redirected, since the installer's `@latest` needs no version up front.
    #[test]
    fn a_redirected_description_falls_back_to_the_crates_latest_when_the_probe_failed() {
        let probe_failed = versions(Some("0.16.2"), None);
        let path = "/opt/homebrew/bin/claude-agent-acp";
        let check = user_copy_claude_check(
            path,
            &InstallSource::Brew,
            Some("0.17.0"),
            Some("brew upgrade claude-agent-acp"),
        );

        let after = apply_to(check, &probe_failed, true);

        let main = after.main.as_ref().unwrap();
        assert_eq!(main.update_fix_type, Some(FixType::UpdateMain));
        assert_eq!(
            main.update_command.as_deref(),
            Some(
                "install Staged's managed @agentclientprotocol/claude-agent-acp 0.17.0 in \
                 ~/.staged/packages (the copy at /opt/homebrew/bin/claude-agent-acp is left as is)"
            )
        );
    }

    /// Only the managed shim gets the managed readout. A row whose path sits
    /// under the shim dir under another name, a row with no executable, a row
    /// the crate did not label bundled, another tool's versions, and a build
    /// with nothing managed all leave the check exactly as the crate produced
    /// it.
    #[test]
    fn unmanaged_copies_and_other_checks_are_left_alone() {
        let shim_bin_dir = Path::new(SHIM_BIN_DIR);
        let installed = versions(Some("0.16.2"), Some("0.17.0"));

        // Under the shim dir but a different binary name.
        let other = Path::new(SHIM_BIN_DIR)
            .join("codex-acp")
            .display()
            .to_string();
        let mut check = managed_claude_check(&other, true);
        let before = json(&check);
        assert!(!apply_managed_bridge_readout(
            &mut check,
            &installed,
            shim_bin_dir,
            true
        ));
        assert_eq!(json(&check), before);

        // Not installed at all.
        let mut check = managed_claude_check(&managed_shim_path(), false);
        check.path = None;
        check.main = None;
        check.status = CheckStatus::Warn;
        let before = json(&check);
        assert!(!apply_managed_bridge_readout(
            &mut check,
            &installed,
            shim_bin_dir,
            false
        ));
        assert_eq!(json(&check), before);

        // The right path, but the crate did not call it bundled (a dev override
        // dir that happens to sit at the shim path is not a managed install).
        let mut check = managed_claude_check(&managed_shim_path(), true);
        check.main.as_mut().unwrap().bundled = None;
        let before = json(&check);
        assert!(!apply_managed_bridge_readout(
            &mut check,
            &installed,
            shim_bin_dir,
            true
        ));
        assert_eq!(json(&check), before);

        // Report-wide: nothing managed (dev override, disabled feature,
        // unsupported target) means no versions and no readout anywhere.
        let mut report = DoctorReport {
            checks: vec![
                managed_claude_check(&managed_shim_path(), true),
                node_runtime_doctor_check(CheckStatus::Pass, "ok".to_string(), None, None, None),
            ],
        };
        let before: Vec<_> = report.checks.iter().map(json).collect();
        apply_managed_bridge_readouts(
            &mut report,
            &[],
            shim_bin_dir,
            true,
            &ManagedInstallDiagnostics::default(),
        );
        let after: Vec<_> = report.checks.iter().map(json).collect();
        assert_eq!(after, before);

        // Report-wide: versions for one tool reach only its own check.
        let codex_versions = ManagedBridgeVersions {
            tool: ManagedTool {
                id: "codex-acp",
                binary: "codex-acp",
                package: "@agentclientprotocol/codex-acp",
            },
            installed: Some("1.1.2".to_string()),
            latest: Some("1.2.0".to_string()),
        };
        apply_managed_bridge_readouts(
            &mut report,
            &[codex_versions],
            shim_bin_dir,
            true,
            &ManagedInstallDiagnostics::default(),
        );
        let after: Vec<_> = report.checks.iter().map(json).collect();
        assert_eq!(after, before);
        apply_managed_bridge_readouts(
            &mut report,
            &[installed],
            shim_bin_dir,
            true,
            &ManagedInstallDiagnostics::default(),
        );
        assert!(report.checks[0].bridge.is_some());
        assert!(report.checks[1].bridge.is_none());
    }

    /// The redirected action's text names the package, the version, where the
    /// managed copy goes, and the copy it leaves alone — an unknown latest
    /// version drops the version rather than printing a placeholder.
    #[test]
    fn managed_install_description_names_the_package_version_and_replaced_copy() {
        let description = managed_install_description(
            &claude_tool(),
            Some("0.17.0"),
            "/opt/homebrew/bin/claude-agent-acp",
        );
        assert_eq!(
            description,
            "install Staged's managed @agentclientprotocol/claude-agent-acp 0.17.0 in \
             ~/.staged/packages (the copy at /opt/homebrew/bin/claude-agent-acp is left as is)"
        );

        let description =
            managed_install_description(&claude_tool(), None, "/opt/homebrew/bin/claude-agent-acp");
        assert_eq!(
            description,
            "install Staged's managed @agentclientprotocol/claude-agent-acp in \
             ~/.staged/packages (the copy at /opt/homebrew/bin/claude-agent-acp is left as is)"
        );
    }

    #[test]
    fn newer_version_available_compares_semver_triples() {
        assert_eq!(
            newer_version_available(Some("0.16.2"), Some("0.17.0")),
            Some(true)
        );
        assert_eq!(
            newer_version_available(Some("0.17.0"), Some("0.17.0")),
            Some(false)
        );
        assert_eq!(
            newer_version_available(Some("1.0.0"), Some("0.99.99")),
            Some(false)
        );
        assert_eq!(
            newer_version_available(Some("v1.2.3"), Some("1.2.4-rc.1")),
            Some(true)
        );
        assert_eq!(newer_version_available(None, Some("1.0.0")), None);
        assert_eq!(newer_version_available(Some("1.0.0"), None), None);
        assert_eq!(newer_version_available(Some("1.0"), Some("1.0.1")), None);
        assert_eq!(newer_version_available(Some("1.0.0"), Some("latest")), None);
    }

    #[test]
    fn is_managed_shim_path_requires_the_shim_dir_and_binary_name() {
        let dir = Path::new(SHIM_BIN_DIR);
        assert!(is_managed_shim_path(
            "/data/.staged/packages/bin/claude-agent-acp",
            dir,
            "claude-agent-acp"
        ));
        assert!(!is_managed_shim_path(
            "/data/.staged/packages/bin/codex-acp",
            dir,
            "claude-agent-acp"
        ));
        assert!(!is_managed_shim_path(
            "/data/.staged/packages/npm-prefix/bin/claude-agent-acp",
            dir,
            "claude-agent-acp"
        ));
        assert!(!is_managed_shim_path(
            "/usr/local/bin/claude-agent-acp",
            dir,
            "claude-agent-acp"
        ));
    }

    /// Checks and fixes must agree on the registry: both option builders take
    /// it from the same `managed_acp_tools::npm_registry_override()` — the
    /// explicit `STAGED_NPM_REGISTRY`, or `None` so npm resolves its own
    /// config.
    #[test]
    fn doctor_options_route_npm_through_the_registry_override() {
        let expected = crate::managed_acp_tools::npm_registry_override();
        assert_eq!(
            run_checks_options(false, Vec::new(), None).npm_registry,
            expected
        );
        assert_eq!(execute_fix_options(None, Vec::new()).npm_registry, expected);
    }

    /// The tail numbers every line from zero and keeps only the newest
    /// `LOGIN_OUTPUT_TAIL_LINES`, and its snapshot states both — a client merges
    /// live lines against `next_seq`, so a snapshot whose `output` did not sit
    /// exactly below it would show lines twice or lose them.
    #[test]
    fn login_output_tail_numbers_lines_and_keeps_the_newest() {
        let mut tail = LoginOutputTail::default();
        assert_eq!(tail.push("Opening browser to sign in…"), 0);
        assert_eq!(tail.push("If the browser didn't open, visit: https://x"), 1);
        assert_eq!(
            tail.status("run-1"),
            DoctorLoginStatus {
                running: true,
                run_id: Some("run-1".to_string()),
                output: vec![
                    "Opening browser to sign in…".to_string(),
                    "If the browser didn't open, visit: https://x".to_string(),
                ],
                next_seq: 2,
            }
        );

        for i in 2..(LOGIN_OUTPUT_TAIL_LINES as u64 + 10) {
            assert_eq!(tail.push(&format!("line {i}")), i);
        }
        let status = tail.status("run-1");
        assert_eq!(status.output.len(), LOGIN_OUTPUT_TAIL_LINES);
        assert_eq!(status.next_seq, LOGIN_OUTPUT_TAIL_LINES as u64 + 10);
        // `output[i]` carries seq `next_seq - output.len() + i`.
        let first_seq = status.next_seq - status.output.len() as u64;
        assert_eq!(status.output.first().unwrap(), &format!("line {first_seq}"));
        assert_eq!(
            status.output.last().unwrap(),
            &format!("line {}", status.next_seq - 1)
        );
    }

    /// A cancelled run is reported as cancelled and not as a failure; a run
    /// that failed on its own keeps its error; and a cancel that only landed
    /// after the fix had finished changes nothing about its result.
    #[test]
    fn login_done_event_tells_a_cancellation_from_a_failure() {
        let cancelled_by_runner =
            Err("Fix cancelled before finishing: claude-agent-acp --cli auth login".to_string());

        let done = login_done_event(
            "ai-agent-claude".into(),
            "run-1".into(),
            &cancelled_by_runner,
            true,
            3,
        );
        assert_eq!(
            done,
            DoctorLoginOutput {
                check_id: "ai-agent-claude".into(),
                run_id: "run-1".into(),
                line: None,
                seq: 3,
                done: true,
                error: None,
                cancelled: true,
            }
        );

        let failed = Err("Fix timed out after 10m without finishing: …".to_string());
        let done = login_done_event("ai-agent-claude".into(), "run-1".into(), &failed, false, 3);
        assert!(!done.cancelled);
        assert_eq!(
            done.error.as_deref(),
            Some("Fix timed out after 10m without finishing: …")
        );

        let done = login_done_event("ai-agent-claude".into(), "run-1".into(), &Ok(()), true, 0);
        assert!(done.done && !done.cancelled && done.error.is_none());
    }

    /// The start answer is what a client keys its whole record on, so its wire
    /// shape is pinned: one tag field for the outcome and the run id beside it
    /// in camelCase, for both variants.
    #[test]
    fn login_start_serializes_as_a_tagged_object_with_the_run_id() {
        assert_eq!(
            serde_json::to_value(LoginStart::Started {
                run_id: "run-1".into()
            })
            .unwrap(),
            serde_json::json!({ "outcome": "started", "runId": "run-1" })
        );
        assert_eq!(
            serde_json::to_value(LoginStart::AlreadyRunning {
                run_id: "run-1".into()
            })
            .unwrap(),
            serde_json::json!({ "outcome": "alreadyRunning", "runId": "run-1" })
        );
        // The status and the events spell it the same way.
        let status = serde_json::to_value(LoginOutputTail::default().status("run-1")).unwrap();
        assert_eq!(status["runId"], "run-1");
        assert_eq!(status["nextSeq"], 0);
        let idle = serde_json::to_value(DoctorLoginStatus::idle()).unwrap();
        assert_eq!(idle["runId"], serde_json::Value::Null);
        let done = serde_json::to_value(login_done_event(
            "ai-agent-claude".into(),
            "run-1".into(),
            &Ok(()),
            false,
            0,
        ))
        .unwrap();
        assert_eq!(done["runId"], "run-1");
    }

    /// Build an entry the way `claim_login` does, without touching the global
    /// map — for tests of the lookup logic alone.
    fn active_login(run_id: &str) -> ActiveLogin {
        let (writer, _stdin) = FixStdin::pipe();
        let (cancel, _cancellation) = FixCancellation::token();
        ActiveLogin {
            run_id: run_id.to_string(),
            writer,
            cancel,
            output: Arc::new(Mutex::new(LoginOutputTail::default())),
        }
    }

    /// A command names a run, and reaches it only while that run holds the
    /// check's slot. Nothing running is one kind of stale, a *different* run
    /// holding the slot is another, and the refusal for a code says which —
    /// and, for the second, that the code did not go to the newer login.
    #[test]
    fn find_login_run_tells_a_finished_run_from_the_one_that_replaced_it() {
        let mut logins = HashMap::new();
        assert_eq!(
            find_login_run(&logins, "ai-agent-claude", "run-1").err(),
            Some(StaleLoginRun::Ended)
        );

        logins.insert("ai-agent-claude".to_string(), active_login("run-2"));
        let found = find_login_run(&logins, "ai-agent-claude", "run-2").expect("the live run");
        assert_eq!(found.run_id, "run-2");
        assert_eq!(
            find_login_run(&logins, "ai-agent-claude", "run-1").err(),
            Some(StaleLoginRun::Superseded)
        );
        // The slot is per check: another check's run id never matches.
        assert_eq!(
            find_login_run(&logins, "ai-agent-codex", "run-2").err(),
            Some(StaleLoginRun::Ended)
        );

        let ended = StaleLoginRun::Ended.code_refusal("ai-agent-claude");
        assert!(ended.contains("has ended"), "{ended}");
        assert!(
            ended.contains("no login is running for ai-agent-claude"),
            "{ended}"
        );
        let superseded = StaleLoginRun::Superseded.code_refusal("ai-agent-claude");
        assert!(superseded.contains("has ended"), "{superseded}");
        assert!(
            superseded.contains("a newer login is running for ai-agent-claude"),
            "{superseded}"
        );
        assert!(superseded.contains("was not delivered"), "{superseded}");
    }

    /// Every claim mints its own id, and ids are not recycled: two runs of one
    /// check, and two checks' runs, never share one.
    #[test]
    fn login_run_ids_are_unique() {
        let ids: std::collections::HashSet<String> = (0..64).map(|_| new_login_run_id()).collect();
        assert_eq!(ids.len(), 64);
        assert!(ids.iter().all(|id| !id.is_empty()));
    }

    /// A slot releases only the run it was claimed for. Its `Drop` cannot
    /// normally run while a later run holds the entry — the later claim needs
    /// the entry gone first — but the release is keyed to the run id so that
    /// stays true by construction rather than by call order.
    ///
    /// Uses `ai-agent-cursor`: `ACTIVE_LOGINS` is process-global and the other
    /// login tests in this module claim other checks.
    #[test]
    fn login_slot_releases_only_its_own_run() {
        let check_id = "ai-agent-cursor";
        let LoginClaim::Claimed(login) = claim_login(check_id).expect("cursor has a login fix")
        else {
            panic!("the slot is free");
        };
        let live_run_id = login.run_id.clone();

        // A stale slot for a run that no longer holds the entry.
        drop(LoginSlot {
            check_id: check_id.to_string(),
            run_id: "an-earlier-run".to_string(),
        });
        assert_eq!(
            active_logins()
                .lock()
                .unwrap()
                .get(check_id)
                .map(|login| login.run_id.clone()),
            Some(live_run_id),
            "a stale slot's drop leaves the live run's entry alone"
        );

        drop(login);
        assert!(
            !active_logins().lock().unwrap().contains_key(check_id),
            "the run's own slot releases it"
        );
    }

    /// The slot lifecycle end to end: a claim holds the check under a run id, a
    /// second claim reports that id rather than an error, the status names it,
    /// the cancel and code commands reach the run only under it, dropping the
    /// claim releases the slot, and the next claim gets a new id and a token the
    /// earlier cancel does not refuse.
    ///
    /// Uses `ai-agent-codex` because `ACTIVE_LOGINS` is process-global and the
    /// other login tests in this module claim other checks.
    #[tokio::test]
    async fn login_slot_is_released_on_drop_and_the_cancel_reaches_the_run() {
        let check_id = "ai-agent-codex";
        let LoginClaim::Claimed(login) = claim_login(check_id).expect("codex has a login fix")
        else {
            panic!("the first claim takes the slot");
        };
        let run_id = login.run_id.clone();
        assert_eq!(
            login.slot.run_id, run_id,
            "the slot releases the run it was claimed for"
        );
        match claim_login(check_id).expect("still a valid check") {
            LoginClaim::AlreadyRunning { run_id: running } => assert_eq!(
                running, run_id,
                "a second claim names the run that holds the slot"
            ),
            LoginClaim::Claimed(_) => panic!("the slot is held"),
        }

        assert_eq!(
            doctor_login_status(check_id.into()).await,
            DoctorLoginStatus {
                running: true,
                run_id: Some(run_id.clone()),
                output: Vec::new(),
                next_seq: 0,
            }
        );
        // What the run's `on_line` records is what a late client is shown.
        login
            .output
            .lock()
            .unwrap()
            .push("Opening browser to sign in…");
        assert_eq!(
            doctor_login_status(check_id.into()).await.output,
            vec!["Opening browser to sign in…".to_string()]
        );

        // A code for a run that isn't the one holding the slot is refused —
        // and never reaches this run's pipe. The right id queues it (the fix
        // has not spawned here, so `send_line` buffers it for the replay).
        let refused =
            send_doctor_login_code(check_id.into(), "an-earlier-run".into(), "abc".into())
                .await
                .expect_err("a code for another run is refused");
        assert!(
            refused.contains("a newer login is running for ai-agent-codex"),
            "{refused}"
        );
        send_doctor_login_code(check_id.into(), run_id.clone(), "abc".into())
            .await
            .expect("a code for the live run reaches its pipe");

        assert!(!login.cancel.is_cancelled());
        assert!(
            !cancel_doctor_login(check_id.into(), "an-earlier-run".into()).await,
            "a cancel for another run is not this run's"
        );
        assert!(!login.cancel.is_cancelled());
        assert!(cancel_doctor_login(check_id.into(), run_id.clone()).await);
        assert!(
            login.cancel.is_cancelled(),
            "the command reaches the token the run was given"
        );
        assert!(
            cancel_doctor_login(check_id.into(), run_id.clone()).await,
            "repeating it is a no-op that still reports the running login"
        );

        drop(login);
        assert_eq!(
            doctor_login_status(check_id.into()).await,
            DoctorLoginStatus::idle()
        );
        assert!(
            !cancel_doctor_login(check_id.into(), run_id.clone()).await,
            "nothing running: nothing to cancel"
        );
        let refused = send_doctor_login_code(check_id.into(), run_id.clone(), "abc".into())
            .await
            .expect_err("a code for a run that has ended is refused");
        assert!(
            refused.contains("no login is running for ai-agent-codex"),
            "{refused}"
        );

        let LoginClaim::Claimed(again) = claim_login(check_id).expect("still a valid check") else {
            panic!("a released slot can be claimed again");
        };
        assert_ne!(
            again.run_id, run_id,
            "each run gets its own id, so the earlier run's events and commands can't be taken for this one's"
        );
        assert!(
            !again.cancel.is_cancelled(),
            "each run gets a fresh token, so the earlier cancel can't refuse the retry"
        );
    }

    #[test]
    fn claim_login_refuses_a_check_with_no_login_fix() {
        let err = claim_login("ai-agent-goose").expect_err("goose has no auth command");
        assert!(
            err.contains("No login fix available for ai-agent-goose"),
            "{err}"
        );
        assert!(!active_logins()
            .lock()
            .unwrap()
            .contains_key("ai-agent-goose"));
    }
}
