//! Individual health-check functions for tool dependencies (git, gh, git-lfs, etc.).

use std::process::Command;

use crate::resolve::format_command_output;
use crate::types::{CheckStatus, DoctorCheck, FixType, ResolvedBinary};

/// Fix command for enabling copy-on-write git clones.
pub(crate) const CLONEFILE_FIX_COMMAND: &str = "git config --global core.clonefile true";

/// Check that `git` is installed and reachable.
pub fn check_git(resolved: &ResolvedBinary) -> DoctorCheck {
    let label = "Git".to_string();
    let id = "git".to_string();
    let search = &resolved.search_output;
    let header = "# Check: Git — verify git is installed and reachable";

    let git_path = match &resolved.path {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "Git not found".to_string(),
                fix_url: Some("https://git-scm.com/downloads".to_string()),
                fix_command: None,
                fix_type: None,
                path: None,
                bridge_path: None,
                raw_output: Some(format!("{header}\nnot found via resolve_binary\n{search}")),
            };
        }
    };
    let path_str = git_path.to_string_lossy().to_string();

    match Command::new(git_path).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("git --version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Pass,
                message: version,
                fix_url: None,
                fix_command: None,
                fix_type: None,
                path: Some(path_str),
                bridge_path: None,
                raw_output: Some(raw),
            }
        }
        Ok(output) => {
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("git --version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "Git not found".to_string(),
                fix_url: Some("https://git-scm.com/downloads".to_string()),
                fix_command: None,
                fix_type: None,
                path: Some(path_str),
                bridge_path: None,
                raw_output: Some(raw),
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "Git not found".to_string(),
            fix_url: Some("https://git-scm.com/downloads".to_string()),
            fix_command: None,
            fix_type: None,
            path: Some(path_str),
            bridge_path: None,
            raw_output: Some(format!("{header}\n$ git --version\nerror: {e}\n{search}")),
        },
    }
}

/// Check that the GitHub CLI (`gh`) is installed.
pub fn check_gh(resolved: &ResolvedBinary) -> DoctorCheck {
    let label = "GitHub CLI".to_string();
    let id = "gh".to_string();
    let search = &resolved.search_output;
    let header = "# Check: GitHub CLI — verify gh is installed";

    let gh_path = match &resolved.path {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "GitHub CLI not found".to_string(),
                fix_url: Some("https://cli.github.com".to_string()),
                fix_command: None,
                fix_type: None,
                path: None,
                bridge_path: None,
                raw_output: Some(format!("{header}\nnot found via resolve_binary\n{search}")),
            };
        }
    };
    let path_str = gh_path.to_string_lossy().to_string();

    match Command::new(gh_path).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            let first_line = version.lines().next().unwrap_or("gh").trim().to_string();
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("gh --version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Pass,
                message: first_line,
                fix_url: None,
                fix_command: None,
                fix_type: None,
                path: Some(path_str),
                bridge_path: None,
                raw_output: Some(raw),
            }
        }
        Ok(output) => {
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("gh --version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "GitHub CLI not found".to_string(),
                fix_url: Some("https://cli.github.com".to_string()),
                fix_command: None,
                fix_type: None,
                path: Some(path_str),
                bridge_path: None,
                raw_output: Some(raw),
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "GitHub CLI not found".to_string(),
            fix_url: Some("https://cli.github.com".to_string()),
            fix_command: None,
            fix_type: None,
            path: Some(path_str),
            bridge_path: None,
            raw_output: Some(format!("{header}\n$ gh --version\nerror: {e}\n{search}")),
        },
    }
}

/// Check that `gh auth status` succeeds (user is logged in).
pub fn check_gh_auth(gh: &ResolvedBinary) -> DoctorCheck {
    let label = "GitHub Auth".to_string();
    let id = "gh-auth".to_string();
    let header = "# Check: GitHub Auth — verify user is logged in to GitHub";

    let gh_path = match &gh.path {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Fail,
                message: "GitHub CLI not found — install gh first".to_string(),
                fix_url: Some("https://cli.github.com".to_string()),
                fix_command: None,
                fix_type: None,
                path: None,
                bridge_path: None,
                raw_output: Some(format!("{header}\ngh not found via resolve_binary")),
            };
        }
    };

    match Command::new(gh_path).args(["auth", "status"]).output() {
        Ok(output) => {
            let raw = format!(
                "{header}\n{}",
                format_command_output("gh auth status", &output)
            );
            if output.status.success() {
                DoctorCheck {
                    id,
                    label,
                    status: CheckStatus::Pass,
                    message: "Authenticated".to_string(),
                    fix_url: None,
                    fix_command: None,
                    fix_type: None,
                    path: None,
                    bridge_path: None,
                    raw_output: Some(raw),
                }
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let hint = if stderr.contains("not logged in") || stderr.contains("no oauth token")
                {
                    "Not authenticated — run `gh auth login`".to_string()
                } else {
                    "Not authenticated".to_string()
                };
                DoctorCheck {
                    id,
                    label,
                    status: CheckStatus::Fail,
                    message: hint,
                    fix_url: Some("https://cli.github.com/manual/gh_auth_login".to_string()),
                    fix_command: None,
                    fix_type: None,
                    path: None,
                    bridge_path: None,
                    raw_output: Some(raw),
                }
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Fail,
            message: "Not authenticated".to_string(),
            fix_url: Some("https://cli.github.com/manual/gh_auth_login".to_string()),
            fix_command: None,
            fix_type: None,
            path: None,
            bridge_path: None,
            raw_output: Some(format!("{header}\n$ gh auth status\nerror: {e}")),
        },
    }
}

/// Check that Git LFS is installed.
pub fn check_git_lfs(git: &ResolvedBinary, git_lfs: &ResolvedBinary) -> DoctorCheck {
    let label = "Git LFS".to_string();
    let id = "git-lfs".to_string();
    let search = &git_lfs.search_output;
    let header =
        "# Check: Git LFS — verify git-lfs is installed (optional, needed for large files)";

    let git_path = match &git.path {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Warn,
                message: "Git LFS not installed (optional, needed for large files)".to_string(),
                fix_url: Some("https://git-lfs.com".to_string()),
                fix_command: None,
                fix_type: None,
                path: None,
                bridge_path: None,
                raw_output: Some(format!(
                    "{header}\ngit not found via resolve_binary\n{search}"
                )),
            };
        }
    };

    match Command::new(git_path).args(["lfs", "version"]).output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let path = git_lfs
                .path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("git lfs version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Pass,
                message: version,
                fix_url: None,
                fix_command: None,
                fix_type: None,
                path,
                bridge_path: None,
                raw_output: Some(raw),
            }
        }
        Ok(output) => {
            let raw = format!(
                "{header}\n{}\n{}",
                format_command_output("git lfs version", &output),
                search
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Warn,
                message: "Git LFS not installed (optional, needed for large files)".to_string(),
                fix_url: Some("https://git-lfs.com".to_string()),
                fix_command: None,
                fix_type: None,
                path: None,
                bridge_path: None,
                raw_output: Some(raw),
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Warn,
            message: "Git LFS not installed (optional, needed for large files)".to_string(),
            fix_url: Some("https://git-lfs.com".to_string()),
            fix_command: None,
            fix_type: None,
            path: None,
            bridge_path: None,
            raw_output: Some(format!("{header}\n$ git lfs version\nerror: {e}\n{search}")),
        },
    }
}

/// Check that `core.clonefile` is enabled in the global git config.
pub fn check_clonefile(git: &ResolvedBinary) -> DoctorCheck {
    let label = "Copy on Write Git Clones".to_string();
    let id = "git-clonefile".to_string();
    let fix_cmd = CLONEFILE_FIX_COMMAND.to_string();
    let header = "# Check: Copy on Write Git Clones — verify core.clonefile is enabled for disk space savings";

    let git_path = match &git.path {
        Some(p) => p,
        None => {
            return DoctorCheck {
                id,
                label,
                status: CheckStatus::Warn,
                message: "Git not found — cannot check clonefile setting".to_string(),
                fix_url: Some("https://git-scm.com/downloads".to_string()),
                fix_command: None,
                fix_type: None,
                path: None,
                bridge_path: None,
                raw_output: Some(format!("{header}\ngit not found via resolve_binary")),
            };
        }
    };

    match Command::new(git_path)
        .args(["config", "--global", "core.clonefile"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let raw = format!(
                "{header}\n{}",
                format_command_output("git config --global core.clonefile", &output)
            );
            let value = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_lowercase();
            if value == "true" {
                DoctorCheck {
                    id,
                    label,
                    status: CheckStatus::Pass,
                    message: "Enabled — reduces disk space used by new worktrees".to_string(),
                    fix_url: None,
                    fix_command: None,
                    fix_type: None,
                    path: None,
                    bridge_path: None,
                    raw_output: Some(raw),
                }
            } else {
                DoctorCheck {
                    id,
                    label,
                    status: CheckStatus::Warn,
                    message: "Disabled — enable to reduce disk space used by new worktrees"
                        .to_string(),
                    fix_url: None,
                    fix_command: Some(fix_cmd),
                    fix_type: Some(FixType::Command),
                    path: None,
                    bridge_path: None,
                    raw_output: Some(raw),
                }
            }
        }
        // Key not set — treat as not enabled
        Ok(output) => {
            let raw = format!(
                "{header}\n{}",
                format_command_output("git config --global core.clonefile", &output)
            );
            DoctorCheck {
                id,
                label,
                status: CheckStatus::Warn,
                message: "Not set — enable to reduce disk space used by new worktrees".to_string(),
                fix_url: None,
                fix_command: Some(fix_cmd),
                fix_type: Some(FixType::Command),
                path: None,
                bridge_path: None,
                raw_output: Some(raw),
            }
        }
        Err(e) => DoctorCheck {
            id,
            label,
            status: CheckStatus::Warn,
            message: "Not set — enable to reduce disk space used by new worktrees".to_string(),
            fix_url: None,
            fix_command: Some(fix_cmd),
            fix_type: Some(FixType::Command),
            path: None,
            bridge_path: None,
            raw_output: Some(format!(
                "{header}\n$ git config --global core.clonefile\nerror: {e}"
            )),
        },
    }
}
