//! Binary resolution and command output formatting helpers.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::types::ResolvedBinary;

/// Resolve a binary by trying login shell path lookup then common install paths.
pub fn resolve_binary(cmd: &str) -> ResolvedBinary {
    let mut lines = vec![format!("resolve '{cmd}':")];

    // Strategy 1: Login shell path lookup (primary)
    lines.push("  strategy 1 — login shell path lookup:".to_string());
    for (shell, lookup_cmd) in shell_lookup_commands(cmd) {
        match Command::new(shell).args(["-l", "-c", &lookup_cmd]).output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !output.status.success() {
                    lines.push(format!("    {shell} -l -c '{lookup_cmd}' => not found"));
                    continue;
                }

                let candidate_paths = candidate_paths_from_shell_output(stdout.as_ref());
                if let Some(path) = candidate_paths
                    .iter()
                    .rev()
                    .find(|path| is_executable_file(path))
                {
                    lines.push(format!(
                        "    {shell} -l -c '{lookup_cmd}' => {} (resolved)",
                        path.display()
                    ));
                    return ResolvedBinary {
                        path: Some(path.clone()),
                        search_output: lines.join("\n"),
                    };
                }

                if let Some(path) = candidate_paths.first() {
                    lines.push(format!(
                        "    {shell} -l -c '{lookup_cmd}' => {} (ignored: not an executable file)",
                        path.display()
                    ));
                } else if stdout.trim().is_empty() {
                    lines.push(format!("    {shell} -l -c '{lookup_cmd}' => not found"));
                } else {
                    lines.push(format!(
                        "    {shell} -l -c '{lookup_cmd}' => {} (ignored: not an absolute path)",
                        summarize_output(stdout.as_ref())
                    ));
                }
            }
            Err(e) => {
                lines.push(format!("    {shell} -l -c '{lookup_cmd}' => error: {e}"));
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
        if is_executable_file(&path) {
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

fn shell_lookup_commands(cmd: &str) -> [(&'static str, String); 2] {
    let quoted = shell_quote(cmd);
    [
        ("/bin/zsh", format!("whence -p -- {quoted}")),
        ("/bin/bash", format!("type -P -- {quoted}")),
    ]
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn candidate_paths_from_shell_output(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .map(str::trim)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect()
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn summarize_output(output: &str) -> String {
    let trimmed = output.trim();
    const MAX_LEN: usize = 120;
    if trimmed.len() <= MAX_LEN {
        return trimmed.replace('\n', "\\n");
    }
    let summary: String = trimmed.chars().take(MAX_LEN).collect();
    format!("{}...", summary.replace('\n', "\\n"))
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

#[cfg(test)]
mod tests {
    use super::{candidate_paths_from_shell_output, is_executable_file, shell_quote};
    use std::fs::{self, File};
    use std::path::PathBuf;

    #[test]
    fn candidate_accepts_single_absolute_path() {
        assert_eq!(
            candidate_paths_from_shell_output("/opt/homebrew/bin/git\n"),
            vec![PathBuf::from("/opt/homebrew/bin/git")]
        );
    }

    #[test]
    fn candidate_tolerates_startup_output_before_absolute_path() {
        assert_eq!(
            candidate_paths_from_shell_output("hello from shell init\n/opt/homebrew/bin/git\n"),
            vec![PathBuf::from("/opt/homebrew/bin/git")]
        );
    }

    #[test]
    fn candidate_rejects_function_body_output() {
        let output = "git () {\n\tcommand git \"$@\"\n}\n";
        assert_eq!(
            candidate_paths_from_shell_output(output),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn candidate_rejects_relative_or_command_name_output() {
        assert_eq!(
            candidate_paths_from_shell_output("git\n"),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn shell_quote_handles_single_quotes() {
        assert_eq!(shell_quote("git'bad"), "'git'\\''bad'");
    }

    #[test]
    fn picks_last_executable_when_rc_file_echoes_absolute_path() {
        // Simulates a rc file printing an absolute path of an unrelated
        // executable before the shell builtin prints the real lookup answer.
        let dir = std::env::temp_dir().join(format!("doctor-resolve-last-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let decoy = dir.join("decoy");
        let real = dir.join("real");
        File::create(&decoy).unwrap();
        File::create(&real).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&decoy, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let stdout = format!("{}\n{}\n", decoy.display(), real.display());
        let candidates = candidate_paths_from_shell_output(&stdout);
        let picked = candidates.iter().rev().find(|p| is_executable_file(p));

        assert_eq!(picked, Some(&real));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn executable_file_validation_checks_file_and_mode() {
        let dir = std::env::temp_dir().join(format!("doctor-resolve-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let executable = dir.join("tool");
        File::create(&executable).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!is_executable_file(&executable));

            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(is_executable_file(&executable));
        }

        #[cfg(not(unix))]
        {
            assert!(is_executable_file(&executable));
        }

        assert!(!is_executable_file(&dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
