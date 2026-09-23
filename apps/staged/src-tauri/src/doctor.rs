//! Tauri command wrappers for the doctor health-check system.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

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
    /// The last [`LOGIN_OUTPUT_TAIL_LINES`] lines the run emitted, oldest
    /// first. Empty when nothing is running.
    pub output: Vec<String>,
    /// The `seq` the run's next line will carry. `output` covers the `seq`s
    /// from `next_seq - output.len()` up to but excluding `next_seq`, which is
    /// how a client merges this snapshot with lines it received live.
    pub next_seq: u64,
}

impl DoctorLoginStatus {
    fn idle() -> Self {
        Self {
            running: false,
            output: Vec::new(),
            next_seq: 0,
        }
    }
}

/// What [`start_doctor_login`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LoginStart {
    Started,
    /// A login for the check was already running. The caller can attach to it:
    /// [`doctor_login_status`] has its output so far, the `doctor-login-output`
    /// stream carries the rest, and [`send_doctor_login_code`] reaches it.
    AlreadyRunning,
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

    /// The snapshot a late client replays, for a login that is running.
    fn status(&self) -> DoctorLoginStatus {
        DoctorLoginStatus {
            running: true,
            output: self.lines.iter().cloned().collect(),
            next_seq: self.next_seq,
        }
    }
}

/// A login fix in flight, keyed by check id in [`ACTIVE_LOGINS`].
struct ActiveLogin {
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
#[derive(Debug)]
struct LoginSlot {
    check_id: String,
}

impl Drop for LoginSlot {
    fn drop(&mut self) {
        active_logins()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.check_id);
    }
}

/// Everything a claimed login needs to run: the pipe and token the fix is
/// given, the handle and tail the entry keeps, and the slot whose drop
/// releases the entry.
#[derive(Debug)]
struct ClaimedLogin {
    stdin: FixStdin,
    cancellation: FixCancellation,
    cancel: FixCancelHandle,
    output: Arc<Mutex<LoginOutputTail>>,
    slot: LoginSlot,
}

/// Environment snapshot for doctor checks and fixes. Shaped through
/// `apply_managed_tools_env` so checks resolve binaries from the same PATH
/// the agent spawn path uses — a bridge Staged manages must never be
/// reported missing (or prompt an install) just because the user has no
/// global copy. The managed npm env is overlaid on top, so checks probe npm
/// state (`npm prefix -g`, version lookups) with the same private-prefix view
/// the fixes install into — a check never contradicts the fix that just ran.
async fn doctor_env_vars() -> Vec<(String, String)> {
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
        // npm-backed checks and fixes route through Block's Artifactory
        // square-npm proxy (registry.npmjs.org is blocked on managed
        // devices); `no-block-npm-registry` builds fall back to npm's
        // default public registry.
        npm_registry: crate::managed_acp_tools::npm_registry().map(str::to_string),
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
        npm_registry: crate::managed_acp_tools::npm_registry().map(str::to_string),
        ..Default::default()
    }
    .with_env_snapshot(env_vars)
}

/// Run all health checks and return the report.
///
/// This is the cheap, no-network path: it resolves binaries and reports
/// install/auth status but does not probe registries for version freshness.
/// The frontend calls this first for an instant paint, then follows up with
/// [`run_doctor_freshness`] to fill in version/update information.
#[tauri::command]
pub async fn run_doctor() -> DoctorReport {
    run_doctor_report(false).await
}

/// Run all health checks with version freshness enabled.
///
/// This is the slower second pass: it probes each readout's installed version
/// and looks up the latest version from the relevant registry (npm, brew,
/// crates.io, GitHub releases), populating `installedVersion`, `latestVersion`,
/// `updateAvailable`, and the source-aware `updateCommand`/`updateFixType` on
/// each readout. Hits the network, so it must never block first paint.
#[tauri::command]
pub async fn run_doctor_freshness() -> DoctorReport {
    run_doctor_report(true).await
}

/// Run the doctor crate's checks plus Staged-local ones (currently the
/// managed Node.js runtime check) over one shared env snapshot. Bundled
/// readouts are labeled by the doctor crate itself via
/// `RunChecksOptions::bundled_tools_dir`.
async fn run_doctor_report(check_freshness: bool) -> DoctorReport {
    let env_vars = doctor_env_vars().await;
    let (mut report, node_runtime) = tokio::join!(
        doctor::run_checks_with_options(run_checks_options(
            check_freshness,
            env_vars.clone(),
            crate::acp_tools::primary_tools_dir(),
        )),
        run_node_runtime_check(),
    );
    if let Some(check) = node_runtime {
        report.checks.push(check);
    }
    report
}

/// Reserve the login slot for `check_id`, returning what the run needs — or
/// `Ok(None)` when a login for the check is already running. The entry parked
/// in [`ACTIVE_LOGINS`] is also the "a login is running" flag: doctor's
/// `FixStdin` is single-use, so a second concurrent login for one check is
/// refused here rather than spawning a CLI nothing can type into.
fn claim_login(check_id: &str) -> Result<Option<ClaimedLogin>, String> {
    doctor::agents::lookup_fix_command(check_id, &FixType::Auth)
        .ok_or_else(|| format!("No login fix available for {check_id}"))?;
    let (writer, stdin) = FixStdin::pipe();
    // A fresh token per run: a cancelled one stays cancelled and would refuse
    // the retry before it spawned.
    let (cancel, cancellation) = FixCancellation::token();
    let output = Arc::new(Mutex::new(LoginOutputTail::default()));
    let mut logins = active_logins().lock().unwrap_or_else(|e| e.into_inner());
    if logins.contains_key(check_id) {
        return Ok(None);
    }
    logins.insert(
        check_id.to_string(),
        ActiveLogin {
            writer,
            cancel: cancel.clone(),
            output: output.clone(),
        },
    );
    Ok(Some(ClaimedLogin {
        stdin,
        cancellation,
        cancel,
        output,
        slot: LoginSlot {
            check_id: check_id.to_string(),
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
    result: &Result<(), String>,
    cancel_requested: bool,
    lines_emitted: u64,
) -> DoctorLoginOutput {
    let cancelled = result.is_err() && cancel_requested;
    DoctorLoginOutput {
        check_id,
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
        stdin,
        cancellation,
        cancel,
        output,
        slot,
    } = login;
    let env_vars = doctor_env_vars().await;
    let event_check_id = check_id.clone();
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
    drop(slot);
    crate::web_server::emit_to_all(&app_handle, "doctor-login-output", done);
    result
}

/// Start an interactive login fix and stream its output to the frontend.
///
/// Returns as soon as the fix is claimed and spawned; the caller learns the
/// outcome from the `done` event, which lets it feed a code through
/// [`send_doctor_login_code`] while the fix is still running. A login already
/// running for the check is reported as [`LoginStart::AlreadyRunning`] rather
/// than an error: it is the same subprocess the caller wanted, and it can
/// attach to it (see [`doctor_login_status`]).
#[tauri::command]
pub async fn start_doctor_login(
    app_handle: tauri::AppHandle,
    check_id: String,
) -> Result<LoginStart, String> {
    let Some(login) = claim_login(&check_id)? else {
        return Ok(LoginStart::AlreadyRunning);
    };
    tokio::spawn(async move {
        // A failure is reported to the frontend by the final `done` event; this
        // handle has no caller to return it to.
        let _ = run_login_fix(app_handle, check_id, login).await;
    });
    Ok(LoginStart::Started)
}

/// Ask the login running for `check_id` to stop, reporting whether there was
/// one. Idempotent, and harmless when nothing is running.
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
pub async fn cancel_doctor_login(check_id: String) -> bool {
    let cancel = active_logins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&check_id)
        .map(|login| login.cancel.clone());
    match cancel {
        Some(cancel) => {
            cancel.cancel();
            true
        }
        None => false,
    }
}

/// Whether a login is running for `check_id`, and what it has printed so far —
/// for a client that lost its own record of the login (a web refresh, a second
/// client, a reloaded webview) and needs the sign-in URL and code entry back.
///
/// Register the `doctor-login-output` listener *before* calling this, then
/// merge by `seq`: lines below [`DoctorLoginStatus::next_seq`] are in the
/// snapshot, lines at or above it arrived after it.
#[tauri::command]
pub async fn doctor_login_status(check_id: String) -> DoctorLoginStatus {
    // The map lock and the tail lock are never held together: the `on_line`
    // callback takes only the tail's, the slot release only the map's.
    let output = active_logins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&check_id)
        .map(|login| login.output.clone());
    match output {
        Some(output) => output.lock().unwrap_or_else(|e| e.into_inner()).status(),
        None => DoctorLoginStatus::idle(),
    }
}

/// Deliver a line — in practice the authentication code the agent CLI asked
/// for — to a login started by [`start_doctor_login`] or [`run_doctor_fix`].
///
/// `async` deliberately: `send_line` writes into the fix's stdin pipe inline
/// and can block if the fix isn't reading, and under Tauri 2 a non-`async`
/// command body runs on the main thread — the worst possible place to discover
/// a full pipe. The write itself goes to a blocking thread for the same reason.
#[tauri::command]
pub async fn send_doctor_login_code(check_id: String, code: String) -> Result<(), String> {
    let writer = active_logins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&check_id)
        .map(|login| login.writer.clone())
        .ok_or_else(|| format!("No active login for {check_id}"))?;
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
        let login = claim_login(&check_id)?
            .ok_or_else(|| format!("A login is already running for {check_id}"))?;
        return run_login_fix(app_handle, check_id, login).await;
    }
    if matches!(fix_type, FixType::Command | FixType::Bridge) {
        if let Some(tool_id) = managed_tool_for_check(&check_id) {
            return install_managed_tool_logged(tool_id, &check_id).await;
        }
    }
    let env_vars = doctor_env_vars().await;
    if doctor::agents::lookup_fix_command(&check_id, &fix_type)
        .as_deref()
        .is_some_and(crate::managed_acp_tools::is_npm_backed_command)
    {
        ensure_managed_node_runtime_for_fix().await?;
    }
    doctor::execute_fix_with_env_options(check_id, fix_type, execute_fix_options(None, env_vars))
        .await
}

/// The managed ACP bridge behind a doctor check id, when this build manages
/// it. `None` routes the check to the doctor crate's regular fix commands —
/// which is also the correct fallback whenever bridge management is off (dev
/// override, `no-managed-acp-tools`, unsupported target).
fn managed_tool_for_check(check_id: &str) -> Option<&'static str> {
    let tool_id = match check_id {
        "ai-agent-claude" => "claude-acp",
        "ai-agent-codex" => "codex-acp",
        _ => return None,
    };
    crate::managed_acp_tools::managed_tool(tool_id).map(|tool| tool.id)
}

/// Install (or float-upgrade) a managed bridge for a doctor fix/update.
/// Progress goes to the log — doctor fixes have no streamed-output channel,
/// only a button spinner.
async fn install_managed_tool_logged(tool_id: &str, check_id: &str) -> Result<(), String> {
    let log_prefix = format!("[doctor fix {check_id}]");
    crate::managed_acp_tools::install_managed_tool(tool_id, &|line| {
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
    // Updates for the managed ACP bridges are the floating installer itself
    // (`<pkg>@latest` onto the managed runtime) — no shell command runs, so
    // the frontend-supplied command needs no validation here. Readouts
    // resolved from the managed shim dir derive no update command at all
    // (they are labeled bundled), so this arm only fires for a bridge copy
    // that resolved elsewhere (e.g. a user install found on PATH before the
    // first reconcile lands) — and the managed install is the correct
    // upgrade for that state too.
    if let Some(tool_id) = managed_tool_for_check(&check_id) {
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
    if crate::managed_acp_tools::is_npm_backed_command(&expected) {
        ensure_managed_node_runtime_for_fix().await?;
    }
    // Run the backend-derived `expected`, not the frontend-supplied `command`.
    // They are equal past the guard above, but executing `expected` makes the
    // command that runs provably the one the backend derived — no dependence on
    // the equality check surviving future edits.
    doctor::execute_fix_with_env_options(
        check_id,
        fix_type,
        execute_fix_options(Some(expected), env_vars),
    )
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
// Managed Node.js runtime check
// =============================================================================

const NODE_RUNTIME_CHECK_ID: &str = "node-runtime";
const NODE_RUNTIME_CHECK_LABEL: &str = "Node.js Runtime";

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
    build_node_runtime_check(state, &install_dir, &npm_tools)
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

fn build_node_runtime_check(
    state: ManagedNodeRuntimeState,
    install_dir: &Path,
    npm_tools: &[String],
) -> Option<DoctorCheck> {
    let version = &crate::managed_node::node_runtime_lock().version;
    let (status, message) = match state {
        ManagedNodeRuntimeState::Ready => (
            CheckStatus::Pass,
            format!("Staged-managed Node.js {version} is installed"),
        ),
        ManagedNodeRuntimeState::Broken => (
            CheckStatus::Warn,
            format!("Staged-managed Node.js {version} is damaged; run the fix to reinstall it"),
        ),
        ManagedNodeRuntimeState::Missing if npm_tools.is_empty() => return None,
        ManagedNodeRuntimeState::Missing => (
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

    #[test]
    fn ready_runtime_passes_without_a_fix() {
        let check = build_node_runtime_check(
            ManagedNodeRuntimeState::Ready,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &["copilot".to_string()],
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
        )
        .expect("needed-but-missing runtime is reported");
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.message.contains("is not installed"));
        assert_eq!(check.fix_type, Some(FixType::Command));
        let output = check.raw_output.as_deref().expect("raw output");
        assert!(output.contains("- amp-acp"));
        assert!(output.contains("- copilot"));

        // Nothing installed that needs it: stay silent.
        assert!(build_node_runtime_check(
            ManagedNodeRuntimeState::Missing,
            Path::new("/data/packages/node/v9.9.9/plat"),
            &[],
        )
        .is_none());
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

    /// Checks and fixes must agree on the registry: both option builders take
    /// it from the same `managed_acp_tools::npm_registry()` gate.
    #[test]
    fn doctor_options_route_npm_through_the_managed_registry() {
        let expected = crate::managed_acp_tools::npm_registry().map(str::to_string);
        assert_eq!(
            run_checks_options(false, Vec::new(), None).npm_registry,
            expected
        );
        assert_eq!(execute_fix_options(None, Vec::new()).npm_registry, expected);
        if !cfg!(feature = "no-block-npm-registry") {
            assert!(expected.is_some());
        }
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
            tail.status(),
            DoctorLoginStatus {
                running: true,
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
        let status = tail.status();
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

        let done = login_done_event("ai-agent-claude".into(), &cancelled_by_runner, true, 3);
        assert_eq!(
            done,
            DoctorLoginOutput {
                check_id: "ai-agent-claude".into(),
                line: None,
                seq: 3,
                done: true,
                error: None,
                cancelled: true,
            }
        );

        let failed = Err("Fix timed out after 10m without finishing: …".to_string());
        let done = login_done_event("ai-agent-claude".into(), &failed, false, 3);
        assert!(!done.cancelled);
        assert_eq!(
            done.error.as_deref(),
            Some("Fix timed out after 10m without finishing: …")
        );

        let done = login_done_event("ai-agent-claude".into(), &Ok(()), true, 0);
        assert!(done.done && !done.cancelled && done.error.is_none());
    }

    /// The slot lifecycle end to end: a claim holds the check, a second claim
    /// is refused rather than an error, the status and cancel commands find
    /// the run through the entry, dropping the claim releases the slot, and
    /// the next claim gets a token the earlier cancel does not refuse.
    ///
    /// Uses `ai-agent-codex` because `ACTIVE_LOGINS` is process-global and the
    /// other login test in this module claims `ai-agent-claude`.
    #[tokio::test]
    async fn login_slot_is_released_on_drop_and_the_cancel_reaches_the_run() {
        let check_id = "ai-agent-codex";
        let login = claim_login(check_id)
            .expect("codex has a login fix")
            .expect("the first claim takes the slot");
        assert!(
            claim_login(check_id)
                .expect("still a valid check")
                .is_none(),
            "a second claim while the slot is held is `None`, not an error"
        );

        assert_eq!(
            doctor_login_status(check_id.into()).await,
            DoctorLoginStatus {
                running: true,
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

        assert!(!login.cancel.is_cancelled());
        assert!(cancel_doctor_login(check_id.into()).await);
        assert!(
            login.cancel.is_cancelled(),
            "the command reaches the token the run was given"
        );
        assert!(
            cancel_doctor_login(check_id.into()).await,
            "repeating it is a no-op that still reports the running login"
        );

        drop(login);
        assert_eq!(
            doctor_login_status(check_id.into()).await,
            DoctorLoginStatus::idle()
        );
        assert!(
            !cancel_doctor_login(check_id.into()).await,
            "nothing running: nothing to cancel"
        );

        let again = claim_login(check_id)
            .expect("still a valid check")
            .expect("a released slot can be claimed again");
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
