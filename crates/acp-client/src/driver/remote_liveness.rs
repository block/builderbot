//! Asking bloxlet whether a remote agent process is still running.
//!
//! An agent can die on the workstation without its output stream ending:
//! when bloxlet restarts under it (as it does after the workstation runs out
//! of memory) it marks the agent's command failed with "bloxlet restarted
//! while command was running", while `sq blox acp` keeps the stream open and
//! retries disconnects indefinitely. Nothing then reaches the sessions on
//! it, so they would wait forever. The proxy uses [`check`] to find out when
//! the stream has just disconnected or ended, or a session's prompt has gone
//! quiet for [`STALLED_PROMPT_AFTER`]. A session reattached to a turn a
//! previous app instance started uses it to learn when that turn has ended,
//! and [`command_started_at`] to learn whether the process it reached is the
//! one that ran the turn.

use std::ffi::{OsStr, OsString};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

/// How often the proxy considers whether a check is due.
#[cfg(not(test))]
pub(super) const WATCH_TICK: Duration = Duration::from_secs(30);
#[cfg(test)]
pub(super) const WATCH_TICK: Duration = Duration::from_millis(20);

/// How long a session may have a prompt in flight with no frame from the
/// agent before the proxy checks on the agent. Tool calls and model turns
/// can legitimately run far longer; the check is what tells the two apart.
#[cfg(not(test))]
pub(super) const STALLED_PROMPT_AFTER: Duration = Duration::from_secs(5 * 60);
#[cfg(test)]
pub(super) const STALLED_PROMPT_AFTER: Duration = Duration::from_millis(200);

/// How long after the stream last disconnected the proxy keeps checking, and
/// how often. bloxlet takes a minute or two to restart and mark the agent's
/// command failed, so one check right away would miss it.
pub(super) const DISCONNECT_WATCH: Duration = Duration::from_secs(5 * 60);
#[cfg(not(test))]
pub(super) const CHECK_AFTER_DISCONNECT_EVERY: Duration = Duration::from_secs(30);
#[cfg(test)]
pub(super) const CHECK_AFTER_DISCONNECT_EVERY: Duration = Duration::from_millis(100);

/// How often a session reattached to a turn a previous app instance started
/// asks whether that turn has ended: as often as an attach reads the process
/// status (`AttachTiming::poll`), since for agents whose answer to the turn's
/// prompt can't be told apart (see `ReattachTurn::sole_turn_of_owner`) the
/// process reading idle is the only sign the turn ended.
#[cfg(not(test))]
pub(super) const REATTACHED_TURN_POLL: Duration = Duration::from_secs(5);
#[cfg(test)]
pub(super) const REATTACHED_TURN_POLL: Duration = Duration::from_millis(50);

/// How long a reattached turn bloxlet still runs may go without a frame for
/// its session before it is cancelled. bloxlet reports idle per process, so
/// busy co-tenants can hide a turn's end; and a permission request the
/// earlier connection never answered is not re-sent to a new one, so a turn
/// blocked on one would otherwise run (and hold its branch's queue) forever.
///
/// Not on a process dedicated to the turn's session whose agent runs
/// unattended (see [`Target::reattached_turn_quiet_cap`]): it has no
/// co-tenants and asks no permission, while a long, silent tool is just as
/// likely there. A dedicated Codex, Pi or Amp process keeps the cap, since
/// those agents ask.
#[cfg(not(test))]
pub(super) const REATTACHED_TURN_QUIET_CAP: Duration = Duration::from_secs(30 * 60);
#[cfg(test)]
pub(super) const REATTACHED_TURN_QUIET_CAP: Duration = Duration::from_millis(3000);

/// How long, after cancelling a quiet reattached turn, to wait for the agent
/// to answer its prompt as cancelled, which says the turn was still running.
/// claude-agent-acp forces a wedged turn to end 30 s after a cancel.
#[cfg(not(test))]
pub(super) const CANCELLED_TURN_ANSWER_WAIT: Duration = Duration::from_secs(45);
#[cfg(test)]
pub(super) const CANCELLED_TURN_ANSWER_WAIT: Duration = Duration::from_millis(3000);

/// How long one `sq blox ws commands` call may take. Checks made while a
/// session waits on the verdict use [`QUICK_CHECK_TIMEOUT`].
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const QUICK_CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// Larger than any workstation's command history the full listing needs to
/// reach back through to find a stopped agent's record.
const COMMAND_HISTORY_LIMIT: &str = "500";

/// Where to ask: the `sq` the proxy runs, the workspace it attaches to, and
/// the agent process it asks `sq` for.
pub(super) struct Target {
    program: OsString,
    workspace: String,
    requested: Requested,
}

/// The agent process a proxy's arguments ask `sq` for: what bloxlet is asked
/// about when `sq` never logged which process it reached.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Requested {
    /// `--command-id=<id>`: that process.
    Command(String),
    /// `--command=<argv>` (comma-separated), or Blox's default without one:
    /// the active ACP process running exactly that argv, as `sq` matches it.
    Argv(Vec<String>),
}

impl Target {
    /// The target for a proxy running `program args…`, if those are `blox acp
    /// <workspace> …` arguments.
    pub(super) fn of(program: &OsStr, args: &[OsString]) -> Option<Self> {
        let [blox, acp, workspace, flags @ ..] = args else {
            return None;
        };
        if blox != "blox" || acp != "acp" {
            return None;
        }
        let flag = |name: &str| {
            flags
                .iter()
                .find_map(|arg| Some(arg.to_str()?.strip_prefix(name)?.to_string()))
        };
        let requested = match flag("--command-id=") {
            Some(command_id) => Requested::Command(command_id),
            None => Requested::Argv(match flag("--command=") {
                Some(command) => command.split(',').map(str::to_string).collect(),
                None => crate::types::BLOX_DEFAULT_ACP_ARGV
                    .iter()
                    .map(|arg| arg.to_string())
                    .collect(),
            }),
        };
        Some(Self {
            program: program.to_owned(),
            workspace: workspace.to_str()?.to_string(),
            requested,
        })
    }

    /// How long a turn reattached to through this proxy may go quiet before
    /// it is cancelled ([`REATTACHED_TURN_QUIET_CAP`]), or `None` to wait on
    /// it for as long as bloxlet runs it. That is only for a process
    /// dedicated to one Staged session (an argv naming it, see
    /// [`crate::types::staged_session_agent`]) whose agent runs unattended
    /// ([`super::blox_runs_unattended`]): nothing can block it on a
    /// permission request, and no co-tenant hides its end. A process pinned
    /// by `--command-id` is not known to be one.
    pub(super) fn reattached_turn_quiet_cap(&self) -> Option<Duration> {
        let alone_and_unattended = match &self.requested {
            Requested::Command(_) => false,
            Requested::Argv(argv) => {
                crate::types::staged_session_agent(argv).is_some_and(super::blox_runs_unattended)
            }
        };
        (!alone_and_unattended).then_some(REATTACHED_TURN_QUIET_CAP)
    }

    /// The agent process the proxy asks `sq` for, for a log line.
    pub(super) fn requested(&self) -> String {
        match &self.requested {
            Requested::Command(command_id) => format!("command {command_id}"),
            Requested::Argv(argv) => format!("running `{}`", argv.join(" ")),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Liveness {
    /// Running a prompt.
    Running,
    /// Running, with no prompt in flight.
    Idle,
    /// The process has stopped, for the given reason.
    Gone(String),
}

/// Whether bloxlet still runs the agent process `command_id`. An error means
/// the check itself failed and says nothing about the process.
pub(super) async fn check(target: &Target, command_id: &str) -> Result<Liveness, String> {
    let active = list_commands(target, &["--status=active"], CHECK_TIMEOUT).await?;
    if let Some(command) = active.iter().find(|command| command.id == command_id) {
        // bloxlet marks an ACP process idle while none of its prompts is
        // pending, across every session it hosts.
        return Ok(if command.status == "idle" {
            Liveness::Idle
        } else {
            Liveness::Running
        });
    }
    // No longer active. Its record says why, when it is still listed.
    let record = list_commands(
        target,
        &[&format!("--limit={COMMAND_HISTORY_LIMIT}")],
        CHECK_TIMEOUT,
    )
    .await
    .ok()
    .and_then(|commands| {
        commands
            .into_iter()
            .find(|command| command.id == command_id)
    });
    Ok(Liveness::Gone(match record {
        Some(CommandRecord {
            status, message, ..
        }) if message.is_empty() => {
            format!("bloxlet reports the agent process (command {command_id}) as {status}")
        }
        Some(CommandRecord {
            status, message, ..
        }) => format!(
            "bloxlet reports the agent process (command {command_id}) as {status}: {message}"
        ),
        None => {
            format!("bloxlet no longer lists the agent process (command {command_id}) as running")
        }
    }))
}

/// Whether bloxlet still runs the agent process the proxy asked `sq` for,
/// found the way `sq` finds it: for when `sq` never logged which process it
/// reached. An error means the check itself failed and says nothing about
/// the process.
pub(super) async fn check_requested(target: &Target) -> Result<Liveness, String> {
    let argv = match &target.requested {
        Requested::Command(command_id) => return check(target, command_id).await,
        Requested::Argv(argv) => argv,
    };
    let stdout = run_listing(target, &["--status=active"], CHECK_TIMEOUT).await?;
    let active: Vec<blox_cli::WorkstationCommand> = serde_json::from_slice(&stdout)
        .map_err(|error| format!("unreadable `sq blox ws commands` output: {error}"))?;
    Ok(match blox_cli::find_acp_process(&active, None, argv) {
        Some(command) if command.status == blox_cli::CommandStatus::Idle => Liveness::Idle,
        Some(_) => Liveness::Running,
        None => Liveness::Gone(format!(
            "bloxlet lists no running agent process {}",
            target.requested()
        )),
    })
}

/// What bloxlet says about when a command started.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CommandStart {
    /// Active, and started at this time, in seconds since the epoch.
    At(i64),
    /// Active, but listed without a start time.
    Unknown,
    /// No longer active.
    NotActive,
}

/// When command `command_id` started. An error means the check itself
/// failed and says nothing about the command.
pub(super) async fn command_started_at(
    target: &Target,
    command_id: &str,
) -> Result<CommandStart, String> {
    let active = list_commands(target, &["--status=active"], QUICK_CHECK_TIMEOUT).await?;
    Ok(
        match active.into_iter().find(|command| command.id == command_id) {
            Some(CommandRecord {
                started_at: Some(started_at),
                ..
            }) => CommandStart::At(started_at),
            Some(_) => CommandStart::Unknown,
            None => CommandStart::NotActive,
        },
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct CommandRecord {
    pub(super) id: String,
    pub(super) status: String,
    pub(super) message: String,
    /// When the process started (or was created, when it never reported
    /// starting), in seconds since the epoch.
    pub(super) started_at: Option<i64>,
}

async fn list_commands(
    target: &Target,
    filter: &[&str],
    timeout: Duration,
) -> Result<Vec<CommandRecord>, String> {
    parse_commands(&run_listing(target, filter, timeout).await?)
}

/// The stdout of `sq blox ws commands <workspace> --json <filter…>`.
async fn run_listing(
    target: &Target,
    filter: &[&str],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(&target.program);
    cmd.args(["blox", "ws", "commands", &target.workspace, "--json"])
        .args(filter)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| "`sq blox ws commands` timed out".to_string())?
        .map_err(|error| format!("`sq blox ws commands` failed to run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`sq blox ws commands` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// Parse `sq blox ws commands --json`: protojson command summaries, so
/// camelCase keys and enum names, though older CLIs printed snake_case keys
/// and enum numbers.
pub(super) fn parse_commands(stdout: &[u8]) -> Result<Vec<CommandRecord>, String> {
    let commands: Vec<Value> = serde_json::from_slice(stdout)
        .map_err(|error| format!("unreadable `sq blox ws commands` output: {error}"))?;
    Ok(commands
        .iter()
        .filter_map(|command| {
            let id = command
                .get("commandId")
                .or_else(|| command.get("command_id"))?
                .as_str()?;
            Some(CommandRecord {
                id: id.to_string(),
                status: command.get("status").map_or_else(
                    || "unknown".to_string(),
                    |status| match status {
                        Value::String(name) => name
                            .trim_start_matches("COMMAND_STATUS_")
                            .to_ascii_lowercase(),
                        Value::Number(number) => match number.as_u64() {
                            Some(1) => "pending",
                            Some(2) => "running",
                            Some(3) => "succeeded",
                            Some(4) => "failed",
                            Some(5) => "idle",
                            _ => "unknown",
                        }
                        .to_string(),
                        _ => "unknown".to_string(),
                    },
                ),
                message: command
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                started_at: ["startedAt", "started_at", "createdAt", "created_at"]
                    .iter()
                    .find_map(|key| command.get(*key)?.as_str().and_then(parse_utc_timestamp)),
            })
        })
        .collect())
}

/// Seconds since the epoch of a protojson UTC timestamp such as
/// `2026-11-05T01:07:33Z` (fractional seconds are dropped).
pub(super) fn parse_utc_timestamp(text: &str) -> Option<i64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let time = time.split('.').next()?;
    let mut time = time.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Days from 1970-01-01 to the date (Howard Hinnant's days_from_civil).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(args: &[&str]) -> Option<Target> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        Target::of(OsStr::new("sq"), &args)
    }

    /// The process a proxy asks `sq` for, as bloxlet is asked about it when
    /// `sq` never names the one it reached.
    #[test]
    fn a_target_names_the_process_its_arguments_ask_for() {
        let pinned = target(&[
            "blox",
            "acp",
            "ws",
            "--command=goose,acp",
            "--command-id=42",
        ]);
        assert_eq!(pinned.unwrap().requested(), "command 42");
        let dedicated = target(&[
            "blox",
            "acp",
            "ws",
            "--command=claude-agent-acp,--staged-session=s1",
            "--env-vars=A=1",
        ]);
        assert_eq!(
            dedicated.unwrap().requested(),
            "running `claude-agent-acp --staged-session=s1`"
        );
        let default = target(&["blox", "acp", "ws"]).unwrap();
        assert_eq!(default.workspace, "ws");
        assert_eq!(default.requested(), "running `goose acp`");
        assert!(target(&["-c", "script", "fake-proxy"]).is_none());
    }
}
