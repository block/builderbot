//! Runtime checks of staged packages, before any live tree, shim or state changes.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use super::{node_binary, npm_entrypoint, ManagedTool, ManagedToolError};

pub(super) const VALIDATION_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: usize = 2048;

pub(super) async fn validate_staged_tool(
    node_install_dir: &Path,
    staging_dir: &Path,
    tool: &ManagedTool,
    timeout: Duration,
) -> Result<(), ManagedToolError> {
    // Probe the bundled native CLI through each bridge's runtime resolver.
    // Codex intercepts --version even after `cli`, so use its short flag.
    let args = match tool.id {
        "claude-acp" => ["--cli", "--version"],
        "codex-acp" => ["cli", "-V"],
        // New managed bridges must supply a native runtime check before they
        // can be promoted; an entrypoint file alone cannot certify an install.
        _ => {
            return Err(ManagedToolError::Incomplete(format!(
                "{} has no post-install runtime validation configured",
                tool.package
            )))
        }
    };
    let incomplete = |reason| {
        ManagedToolError::Incomplete(format!(
            "{} failed post-install validation: `{}` {reason}",
            tool.package,
            args.join(" ")
        ))
    };
    let mut command = tokio::process::Command::new(node_binary(node_install_dir));
    command
        .arg(npm_entrypoint(staging_dir, tool.package))
        .args(args)
        .current_dir(staging_dir)
        // In particular, CLAUDE_CODE_EXECUTABLE, CODEX_PATH and NODE_OPTIONS
        // must not let a personal install mask a missing managed dependency.
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    for key in ["HOME", "TMPDIR", "TMP", "TEMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }

    let child = command
        .spawn()
        .map_err(|error| incomplete(format!("could not start: {error}")))?;
    // Keep cleanup alive if the caller drops this validation future. The
    // supervisor owns the child and kills its whole group before reaping it.
    let cancel = CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    tokio::spawn(supervise_probe(child, timeout, cancel))
        .await
        .map_err(|error| incomplete(format!("could not supervise probe: {error}")))?
        .map_err(incomplete)
}

async fn supervise_probe(
    mut child: tokio::process::Child,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<(), String> {
    // Cache the group ID before wait() reaps the bridge: its native child can
    // still be alive and holding stdout/stderr open after the bridge exits.
    #[cfg(unix)]
    let pgid = child.id().expect("newly spawned probe has a PID") as libc::pid_t;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out = Vec::new();
    let mut err = Vec::new();
    // Bound the whole probe, including pipe reads: a subprocess holding a pipe
    // open must not wedge installs or Update all. Keep only a small excerpt,
    // but drain both streams concurrently so verbose failures cannot deadlock.
    let reason = tokio::select! {
        result = tokio::time::timeout(timeout, async {
            tokio::try_join!(
                child.wait(),
                capture_excerpt(&mut stdout, &mut out),
                capture_excerpt(&mut stderr, &mut err),
            )
        }) => match result {
            Ok(Ok((status, (), ()))) if status.success() => return Ok(()),
            Ok(Ok((status, (), ()))) => format!("exited with {status}"),
            Ok(Err(error)) => format!("could not complete: {error}"),
            Err(_) => format!("timed out after {} seconds", timeout.as_secs_f64()),
        },
        _ = cancel.cancelled() => "was cancelled".to_string(),
    };
    #[cfg(unix)]
    {
        // SAFETY: kill(2) takes no pointers; the negative PID targets only the
        // process group created for this probe, including the native CLI.
        if unsafe { libc::kill(-pgid, libc::SIGKILL) } == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                log::warn!("failed to kill ACP validation process group {pgid}: {error}");
            }
        }
    }
    // Reap the bridge (and retain child-only cleanup on non-Unix targets).
    let _ = child.kill().await;
    Err(format!(
        "{reason}{}{}",
        diagnostic("stderr", &err),
        diagnostic("stdout", &out),
    ))
}

async fn capture_excerpt(
    stream: &mut (impl AsyncRead + Unpin),
    excerpt: &mut Vec<u8>,
) -> std::io::Result<()> {
    let mut buffer = [0; 1024];
    loop {
        let len = stream.read(&mut buffer).await?;
        if len == 0 {
            return Ok(());
        }
        excerpt.extend_from_slice(&buffer[..len.min(OUTPUT_LIMIT - excerpt.len())]);
    }
}

fn diagnostic(label: &str, bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!("; {label}: {text}")
    }
}
