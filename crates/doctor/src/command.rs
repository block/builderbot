//! Bounded subprocess helpers for doctor probes.
//!
//! The doctor crate runs many small local probes. None of them should be able
//! to pin a blocking worker forever, so probe call sites route through this
//! module instead of `Command::output()`.

use std::fmt;
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wait_timeout::ChildExt;

pub(crate) const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const FRESHNESS_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

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
            kill_child_process_group(&mut child);
            let _ = child.wait();
            if let Some(handle) = stdout_thread {
                let _ = join_reader(handle);
            }
            if let Some(handle) = stderr_thread {
                let _ = join_reader(handle);
            }
            return Err(CommandError::Timeout {
                command: display_command,
                timeout,
            });
        }
        Err(source) => {
            kill_child_process_group(&mut child);
            let _ = child.wait();
            if let Some(handle) = stdout_thread {
                let _ = join_reader(handle);
            }
            if let Some(handle) = stderr_thread {
                let _ = join_reader(handle);
            }
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

fn kill_child_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(-pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }

    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_runner_captures_success_output() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("printf stdout; printf stderr >&2");

        let output =
            run_command_with_timeout(command, "sh -c output", Duration::from_secs(2)).unwrap();

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "stdout");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "stderr");
    }

    #[test]
    fn command_runner_times_out() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 5");

        let err = run_command_with_timeout(command, "sh -c sleep", Duration::from_millis(100))
            .unwrap_err();

        match err {
            CommandError::Timeout { command, timeout } => {
                assert_eq!(command, "sh -c sleep");
                assert_eq!(timeout, Duration::from_millis(100));
            }
            other => panic!("expected timeout, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn command_runner_kills_process_group_descendant() {
        use nix::sys::signal;
        use nix::unistd::Pid;

        let dir = std::env::temp_dir().join(format!(
            "doctor-command-timeout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("child.pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());

        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        let err = run_command_with_timeout(command, "sh -c descendant", Duration::from_millis(250))
            .unwrap_err();
        assert!(matches!(err, CommandError::Timeout { .. }));

        let pid = std::fs::read_to_string(&pid_file)
            .expect("descendant pid should be written before timeout")
            .trim()
            .parse::<i32>()
            .unwrap();

        let exited = (0..20).any(|_| {
            let gone = signal::kill(Pid::from_raw(pid), None).is_err();
            if !gone {
                std::thread::sleep(Duration::from_millis(50));
            }
            gone
        });

        let _ = std::fs::remove_dir_all(&dir);
        assert!(exited, "descendant process {pid} survived timeout cleanup");
    }
}
