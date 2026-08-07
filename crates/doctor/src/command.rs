//! Bounded subprocess helpers for doctor probes.
//!
//! The doctor crate runs many small local probes. None of them should be able
//! to pin a blocking worker forever, so probe call sites route through this
//! module instead of `Command::output()`. On timeout or wait failure, the
//! runner kills and reaps the direct child; on Unix it first targets the
//! child's process group. Descendants that deliberately detach from that group
//! can survive best-effort cleanup and keep pipe reader threads alive until
//! their inherited stdout/stderr handles close, but the caller still returns.

use std::fmt;
use std::io::Read;
use std::process::{Child, Command, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wait_timeout::ChildExt;

pub(crate) const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const FRESHNESS_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Configure a child process so doctor probes do not flash a console window on
/// Windows. This is a no-op on other platforms.
pub(crate) fn configure_command(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    let _ = command;
}

#[derive(Debug)]
pub(crate) enum CommandError {
    Spawn {
        command: String,
        source: std::io::Error,
    },
    Wait {
        command: String,
        source: std::io::Error,
    },
    Timeout {
        command: String,
        timeout: Duration,
    },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::Spawn { command, source } => {
                write!(f, "failed to spawn `{command}`: {source}")
            }
            CommandError::Wait { command, source } => {
                write!(f, "failed to wait for `{command}`: {source}")
            }
            CommandError::Timeout { command, timeout } => {
                write!(
                    f,
                    "`{command}` timed out after {}",
                    format_duration(*timeout)
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandTimeout {
    pub label: String,
    pub command: String,
    pub timeout: Duration,
}

impl CommandTimeout {
    pub(crate) fn new(
        label: impl Into<String>,
        command: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            label: label.into(),
            command: command.into(),
            timeout,
        }
    }

    pub(crate) fn message(&self) -> String {
        format!(
            "{} timed out after {} running {}",
            self.label,
            format_duration(self.timeout),
            self.command
        )
    }

    pub(crate) fn raw_output(&self) -> String {
        format!(
            "$ {}\ntimed out after {}",
            self.command,
            format_duration(self.timeout)
        )
    }
}

pub(crate) fn format_duration(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{duration:?}")
    }
}

pub(crate) fn run_command_with_timeout(
    mut command: Command,
    display_command: impl Into<String>,
    timeout: Duration,
) -> Result<Output, CommandError> {
    let display_command = display_command.into();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    configure_command(&mut command);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command.spawn().map_err(|source| CommandError::Spawn {
        command: display_command.clone(),
        source,
    })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_thread = stdout.map(|mut stdout| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        })
    });
    let stderr_thread = stderr.map(|mut stderr| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        })
    });

    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            clean_up_after_incomplete_wait(&mut child);
            return Err(CommandError::Timeout {
                command: display_command,
                timeout,
            });
        }
        Err(source) => {
            clean_up_after_incomplete_wait(&mut child);
            return Err(CommandError::Wait {
                command: display_command,
                source,
            });
        }
    };

    let stdout = stdout_thread.map(join_reader).unwrap_or_default();
    let stderr = stderr_thread.map(join_reader).unwrap_or_default();

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn join_reader(handle: JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.join().unwrap_or_default()
}

fn clean_up_after_incomplete_wait(child: &mut Child) {
    kill_child_process_group_or_child(child);
    let _ = child.wait();
}

fn kill_child_process_group_or_child(child: &mut Child) {
    if kill_child_process_group(child) {
        return;
    }

    let _ = child.kill();
}

#[cfg(unix)]
fn kill_child_process_group(child: &Child) -> bool {
    let Ok(pid) = i32::try_from(child.id()) else {
        return false;
    };

    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(-pid),
        nix::sys::signal::Signal::SIGKILL,
    )
    .is_ok()
}

#[cfg(not(unix))]
fn kill_child_process_group(_child: &Child) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_runner_captures_large_output() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("yes | head -c 200000; printf stderr >&2");

        let output =
            run_command_with_timeout(command, "sh -c large output", Duration::from_secs(2))
                .unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 200_000);
        assert_eq!(String::from_utf8_lossy(&output.stderr), "stderr");
    }

    #[cfg(windows)]
    #[test]
    fn command_runner_does_not_create_a_console_window() {
        let script = r#"
            Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class Native { [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); }';
            if ([Native]::GetConsoleWindow() -ne [IntPtr]::Zero) { exit 1 }
        "#;
        let mut command = Command::new("powershell.exe");
        command
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(script);

        let output = run_command_with_timeout(
            command,
            "powershell.exe -NoProfile -NonInteractive -Command GetConsoleWindow",
            Duration::from_secs(10),
        )
        .unwrap();

        assert!(
            output.status.success(),
            "PowerShell reported a console window: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(unix)]
    #[test]
    fn command_runner_returns_when_escaped_descendant_keeps_pipes_open() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("perl -MPOSIX=setsid -e 'setsid(); sleep 2' & wait");
        let timeout = Duration::from_millis(250);
        let started = std::time::Instant::now();
        let err =
            run_command_with_timeout(command, "sh -c escaped descendant", timeout).unwrap_err();

        match err {
            CommandError::Timeout {
                command,
                timeout: actual,
            } => {
                assert_eq!(command, "sh -c escaped descendant");
                assert_eq!(actual, timeout);
            }
            other => panic!("expected timeout, got {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "timeout path waited for escaped descendant pipe EOF"
        );
    }
}
