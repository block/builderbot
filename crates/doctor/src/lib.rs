//! Health Check ("Doctor") — backend checks for external dependencies.
//!
//! Each check probes a single external dependency and returns a status
//! (pass / warn / fail) with a human-readable summary and an optional
//! URL the user can visit to install or configure the dependency.

pub mod agents;
pub mod checks;
pub mod resolve;
pub mod types;

pub use types::{CheckStatus, DoctorCheck, DoctorReport, FixType};

use std::collections::HashMap;

use agents::{check_single_ai_agent, lookup_fix_command, AI_AGENT_CHECKS};
use checks::{check_clonefile, check_gh, check_gh_auth, check_git, check_git_lfs};
use resolve::resolve_binary;
use types::ResolvedBinary;

/// Fallback check returned when a spawn_blocking task panics.
fn empty_check(id: &str, label: &str) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        label: label.to_string(),
        status: CheckStatus::Fail,
        message: "Check failed to run".to_string(),
        fix_url: None,
        fix_command: None,
        fix_type: None,
        path: None,
        bridge_path: None,
        raw_output: None,
    }
}

/// Run all health checks and return the report.
pub async fn run_checks() -> DoctorReport {
    let mut binary_names: Vec<&'static str> = vec!["git", "gh", "git-lfs"];
    for info in AI_AGENT_CHECKS {
        for cmd in info.commands {
            if !binary_names.contains(cmd) {
                binary_names.push(cmd);
            }
        }
        if let Some(main) = info.main_command {
            if !binary_names.contains(&main) {
                binary_names.push(main);
            }
        }
    }

    let handles: Vec<_> = binary_names
        .iter()
        .map(|&name| tokio::task::spawn_blocking(move || (name, resolve_binary(name))))
        .collect();

    let mut resolved: HashMap<&str, ResolvedBinary> = HashMap::new();
    for handle in handles {
        if let Ok((name, rb)) = handle.await {
            resolved.insert(name, rb);
        }
    }

    let fallback = ResolvedBinary {
        path: None,
        search_output: "resolution task panicked".to_string(),
    };
    let r_git = resolved
        .get("git")
        .cloned()
        .unwrap_or_else(|| fallback.clone());
    let r_gh = resolved
        .get("gh")
        .cloned()
        .unwrap_or_else(|| fallback.clone());
    let r_git_lfs = resolved
        .get("git-lfs")
        .cloned()
        .unwrap_or_else(|| fallback.clone());

    let any_agent_found = AI_AGENT_CHECKS.iter().any(|info| {
        info.commands
            .iter()
            .any(|cmd| resolved.get(cmd).is_some_and(|rb| rb.path.is_some()))
    });

    let git_r = r_git.clone();
    let gh_r = r_gh.clone();
    let gh_r2 = r_gh.clone();
    let git_r2 = r_git.clone();
    let git_lfs_r = r_git_lfs;
    let git_r3 = r_git;

    let c_git = tokio::task::spawn_blocking(move || check_git(&git_r));
    let c_gh = tokio::task::spawn_blocking(move || check_gh(&gh_r));
    let c_gh_auth = tokio::task::spawn_blocking(move || check_gh_auth(&gh_r2));
    let c_git_lfs = tokio::task::spawn_blocking(move || check_git_lfs(&git_r2, &git_lfs_r));
    let c_clonefile = tokio::task::spawn_blocking(move || check_clonefile(&git_r3));

    let agent_handles: Vec<_> = AI_AGENT_CHECKS
        .iter()
        .map(|info| {
            let found = any_agent_found;
            let cmds: Vec<ResolvedBinary> = info
                .commands
                .iter()
                .map(|cmd| {
                    resolved
                        .get(cmd)
                        .cloned()
                        .unwrap_or_else(|| fallback.clone())
                })
                .collect();
            let main = info.main_command.and_then(|cmd| resolved.get(cmd).cloned());
            tokio::task::spawn_blocking(move || {
                check_single_ai_agent(info, found, &cmds, main.as_ref())
            })
        })
        .collect();

    let (c_git, c_gh, c_gh_auth, c_git_lfs, c_clonefile) =
        tokio::join!(c_git, c_gh, c_gh_auth, c_git_lfs, c_clonefile);

    let mut checks = vec![
        c_git.unwrap_or_else(|_| empty_check("git", "Git")),
        c_gh.unwrap_or_else(|_| empty_check("gh", "GitHub CLI")),
        c_gh_auth.unwrap_or_else(|_| empty_check("gh-auth", "GitHub Auth")),
        c_git_lfs.unwrap_or_else(|_| empty_check("git-lfs", "Git LFS")),
        c_clonefile.unwrap_or_else(|_| empty_check("git-clonefile", "Copy on Write Git Clones")),
    ];

    for (i, handle) in agent_handles.into_iter().enumerate() {
        let info = &AI_AGENT_CHECKS[i];
        checks.push(
            handle
                .await
                .unwrap_or_else(|_| empty_check(info.id, info.label)),
        );
    }

    DoctorReport { checks }
}

/// Run a fix command for a doctor check, identified by check ID and fix type.
///
/// The actual shell command is looked up from the static check definitions —
/// the caller never sends a raw command string.
pub async fn execute_fix(check_id: String, fix_type: FixType) -> Result<(), String> {
    let command = lookup_fix_command(&check_id, &fix_type)
        .ok_or_else(|| format!("Unknown check '{check_id}' or fix type '{fix_type:?}'"))?;

    execute_command(command).await
}

/// Run an arbitrary shell command in a login shell.
///
/// This is the lower-level primitive used by [`execute_fix`] and can also be
/// called directly by consumers that already have a validated command string
/// (e.g. the Staged app's existing frontend API).
pub async fn execute_command(command: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let (shell, args) = if std::path::Path::new("/bin/zsh").exists() {
            ("/bin/zsh", vec!["-l", "-c", &command])
        } else {
            ("/bin/bash", vec!["-l", "-c", &command])
        };
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let user = std::env::var("USER").unwrap_or_default();
        let output = std::process::Command::new(shell)
            .args(&args)
            .env_clear()
            .env("HOME", &home)
            .env("USER", &user)
            .env("TERM", "xterm-256color")
            .current_dir(&home)
            .output()
            .map_err(|e| format!("Failed to run command: {e}"))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("Command failed with exit code {}", output.status)
            } else {
                stderr
            })
        }
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task failed: {e}")))
}
