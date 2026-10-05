//! Runtime checks of staged packages, before any live tree, shim or state changes.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

use super::{node_binary, npm_entrypoint, ManagedTool, ManagedToolError};

pub(super) const VALIDATION_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: usize = 2048;

pub(super) async fn validate_staged_tool(
    node_install_dir: &Path,
    staging_dir: &Path,
    tool: &ManagedTool,
    timeout: Duration,
) -> Result<(), ManagedToolError> {
    // Codex keeps its entrypoint floor check until it has a verified runtime
    // probe of its own. Claude's bridge --version would miss the native CLI.
    if tool.id != "claude-acp" {
        return Ok(());
    }
    validate_claude(node_install_dir, staging_dir, tool, timeout).await
}

async fn validate_claude(
    node_install_dir: &Path,
    staging_dir: &Path,
    tool: &ManagedTool,
    timeout: Duration,
) -> Result<(), ManagedToolError> {
    let incomplete = |reason| {
        ManagedToolError::Incomplete(format!(
            "{} failed post-install validation: `--cli --version` {reason}",
            tool.package
        ))
    };
    let mut command = tokio::process::Command::new(node_binary(node_install_dir));
    command
        .arg(npm_entrypoint(staging_dir, tool.package))
        .args(["--cli", "--version"])
        .current_dir(staging_dir)
        // In particular, CLAUDE_CODE_EXECUTABLE and NODE_OPTIONS must not
        // let a personal installation mask a missing managed dependency.
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in ["HOME", "TMPDIR", "TMP", "TEMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }

    let mut child = command
        .spawn()
        .map_err(|error| incomplete(format!("could not start: {error}")))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out = Vec::new();
    let mut err = Vec::new();
    // Bound the whole probe, including pipe reads: a subprocess holding a pipe
    // open must not wedge installs or Update all. Keep only a small excerpt,
    // but drain both streams concurrently so verbose failures cannot deadlock.
    let result = tokio::time::timeout(timeout, async {
        tokio::try_join!(
            child.wait(),
            capture_excerpt(&mut stdout, &mut out),
            capture_excerpt(&mut stderr, &mut err),
        )
    })
    .await;
    let reason = match result {
        Ok(Ok((status, (), ()))) if status.success() => return Ok(()),
        Ok(Ok((status, (), ()))) => format!("exited with {status}"),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            format!("could not complete: {error}")
        }
        Err(_) => {
            let _ = child.kill().await;
            format!("timed out after {} seconds", timeout.as_secs_f64())
        }
    };
    Err(incomplete(format!(
        "{reason}{}{}",
        diagnostic("stderr", &err),
        diagnostic("stdout", &out),
    )))
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
