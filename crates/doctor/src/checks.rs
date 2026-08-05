//! Individual health-check functions for tool dependencies (git, gh, git-lfs, etc.).

use std::process::Command;

use crate::command::{run_command_with_timeout, CommandError, DEFAULT_PROBE_TIMEOUT};
use crate::environment::{apply_doctor_env, DoctorEnv};
use crate::resolve::format_command_output;
use crate::timeout_check::{command_timeout_check, TimeoutCheck};
use crate::types::{CheckStatus, DoctorCheck, ResolvedBinary};

/// Check that `git` is installed and reachable.
pub fn check_git(resolved: &ResolvedBinary, env: Option<&DoctorEnv>) -> DoctorCheck {
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
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: None,
                self_updating: None,
                main: None,
                bridge: None,
            };
        }
    };
    let path_str = git_path.to_string_lossy().to_string();

    let mut command = Command::new(git_path);
    command.arg("--version");
    apply_doctor_env(&mut command, env);
    match run_command_with_timeout(command, "git --version", DEFAULT_PROBE_TIMEOUT) {
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
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: resolved.install_source.clone(),
                self_updating: None,
                main: None,
                bridge: None,
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
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: resolved.install_source.clone(),
                self_updating: None,
                main: None,
                bridge: None,
            }
        }
        Err(CommandError::Timeout { command, timeout }) => command_timeout_check(
            TimeoutCheck::new(id, label, CheckStatus::Fail, header, command, timeout)
                .path(Some(path_str))
                .install_source(resolved.install_source.clone())
                .raw_suffix(Some(search)),
        ),
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
            auth_status: None,
            installed_version: None,
            latest_version: None,
            update_available: None,
            install_source: resolved.install_source.clone(),
            self_updating: None,
            main: None,
            bridge: None,
        },
    }
}

/// Check that the GitHub CLI (`gh`) is installed.
pub fn check_gh(resolved: &ResolvedBinary, env: Option<&DoctorEnv>) -> DoctorCheck {
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
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: None,
                self_updating: None,
                main: None,
                bridge: None,
            };
        }
    };
    let path_str = gh_path.to_string_lossy().to_string();

    let mut command = Command::new(gh_path);
    command.arg("--version");
    apply_doctor_env(&mut command, env);
    match run_command_with_timeout(command, "gh --version", DEFAULT_PROBE_TIMEOUT) {
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
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: resolved.install_source.clone(),
                self_updating: None,
                main: None,
                bridge: None,
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
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: resolved.install_source.clone(),
                self_updating: None,
                main: None,
                bridge: None,
            }
        }
        Err(CommandError::Timeout { command, timeout }) => command_timeout_check(
            TimeoutCheck::new(id, label, CheckStatus::Fail, header, command, timeout)
                .path(Some(path_str))
                .install_source(resolved.install_source.clone())
                .raw_suffix(Some(search)),
        ),
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
            auth_status: None,
            installed_version: None,
            latest_version: None,
            update_available: None,
            install_source: resolved.install_source.clone(),
            self_updating: None,
            main: None,
            bridge: None,
        },
    }
}

/// Check that `gh auth status` succeeds (user is logged in).
pub fn check_gh_auth(gh: &ResolvedBinary, env: Option<&DoctorEnv>) -> DoctorCheck {
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
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: None,
                self_updating: None,
                main: None,
                bridge: None,
            };
        }
    };

    let mut command = Command::new(gh_path);
    command.args(["auth", "status"]);
    apply_doctor_env(&mut command, env);
    match run_command_with_timeout(command, "gh auth status", DEFAULT_PROBE_TIMEOUT) {
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
                    auth_status: None,
                    installed_version: None,
                    latest_version: None,
                    update_available: None,
                    install_source: None,
                    self_updating: None,
                    main: None,
                    bridge: None,
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
        }
        Err(CommandError::Timeout { command, timeout }) => command_timeout_check(
            TimeoutCheck::new(id, label, CheckStatus::Fail, header, command, timeout),
        ),
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
            auth_status: None,
            installed_version: None,
            latest_version: None,
            update_available: None,
            install_source: None,
            self_updating: None,
            main: None,
            bridge: None,
        },
    }
}

/// Check that Git LFS is installed.
pub fn check_git_lfs(
    git: &ResolvedBinary,
    git_lfs: &ResolvedBinary,
    env: Option<&DoctorEnv>,
) -> DoctorCheck {
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
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: None,
                self_updating: None,
                main: None,
                bridge: None,
            };
        }
    };

    let mut command = Command::new(git_path);
    command.args(["lfs", "version"]);
    apply_doctor_env(&mut command, env);
    match run_command_with_timeout(command, "git lfs version", DEFAULT_PROBE_TIMEOUT) {
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
                auth_status: None,
                installed_version: None,
                latest_version: None,
                update_available: None,
                install_source: git_lfs.install_source.clone(),
                self_updating: None,
                main: None,
                bridge: None,
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
        Err(CommandError::Timeout { command, timeout }) => command_timeout_check(
            TimeoutCheck::new(id, label, CheckStatus::Warn, header, command, timeout)
                .path(
                    git_lfs
                        .path
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string()),
                )
                .install_source(git_lfs.install_source.clone())
                .raw_suffix(Some(search)),
        ),
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
            auth_status: None,
            installed_version: None,
            latest_version: None,
            update_available: None,
            install_source: None,
            self_updating: None,
            main: None,
            bridge: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn check_git_receives_env_snapshot() {
        let dir = std::env::temp_dir().join(format!("doctor-check-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let git = dir.join("git");
        std::fs::write(
            &git,
            "#!/bin/sh\n\
             if [ \"$DOCTOR_CHECK_MARKER\" = yes ]; then\n\
               echo 'git version 9.8.7'\n\
               exit 0\n\
             fi\n\
             echo 'missing marker' >&2\n\
             exit 42\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let resolved = ResolvedBinary {
            path: Some(PathBuf::from(&git)),
            search_output: "fake git resolved from test".to_string(),
            install_source: None,
        };
        let env = DoctorEnv::new(vec![
            ("DOCTOR_CHECK_MARKER".to_string(), "yes".to_string()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("HOME".to_string(), dir.to_string_lossy().to_string()),
            ("USER".to_string(), "doctor-test".to_string()),
        ]);

        let check = check_git(&resolved, Some(&env));

        assert_eq!(check.status, CheckStatus::Pass);
        assert_eq!(check.message, "git version 9.8.7");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
