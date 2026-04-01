//! Binary resolution and command output formatting helpers.

use std::path::PathBuf;
use std::process::Command;

use super::types::ResolvedBinary;

/// Resolve a binary by trying login shell `which` then common install paths.
pub fn resolve_binary(cmd: &str) -> ResolvedBinary {
    let mut lines = vec![format!("resolve '{cmd}':")];

    // Strategy 1: Login shell `which` (primary)
    lines.push("  strategy 1 — login shell `which`:".to_string());
    for shell in &["/bin/zsh", "/bin/bash"] {
        let which_cmd = format!("which {cmd}");
        match Command::new(shell).args(["-l", "-c", &which_cmd]).output() {
            Ok(output) => {
                let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if output.status.success() && !result.is_empty() {
                    lines.push(format!(
                        "    {shell} -l -c 'which {cmd}' => {result} (resolved)"
                    ));
                    return ResolvedBinary {
                        path: Some(PathBuf::from(&result)),
                        search_output: lines.join("\n"),
                    };
                }
                lines.push(format!("    {shell} -l -c 'which {cmd}' => not found"));
            }
            Err(e) => {
                lines.push(format!("    {shell} -l -c 'which {cmd}' => error: {e}"));
            }
        }
    }

    // Strategy 2: Common install paths (fallback)
    lines.push("  strategy 2 — common install paths (fallback):".to_string());
    for dir in &[
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/home/linuxbrew/.linuxbrew/bin",
    ] {
        let path = PathBuf::from(dir).join(cmd);
        if path.exists() {
            lines.push(format!("    {} => found (resolved)", path.display()));
            return ResolvedBinary {
                path: Some(path),
                search_output: lines.join("\n"),
            };
        }
        lines.push(format!("    {} => not found", path.display()));
    }

    lines.push("  not found in any location".to_string());
    ResolvedBinary {
        path: None,
        search_output: lines.join("\n"),
    }
}

/// Format the raw output of a command invocation for debug diagnostics.
pub fn format_command_output(cmd_desc: &str, output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut raw = format!("$ {cmd_desc}\nexit code: {}", output.status);
    if !stdout.trim().is_empty() {
        raw.push_str(&format!("\nstdout:\n{}", stdout.trim()));
    }
    if !stderr.trim().is_empty() {
        raw.push_str(&format!("\nstderr:\n{}", stderr.trim()));
    }
    raw
}
