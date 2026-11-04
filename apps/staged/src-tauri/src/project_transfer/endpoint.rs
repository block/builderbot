//! The two ends of a transfer: this machine, or a Blox workspace.
//!
//! A transfer step is a script run at one end plus, between steps, a bundle
//! file moved through this machine. [`TransferEndpoint`] is exactly that much,
//! so the per-repo pipeline in [`super::carry`] is written once for both
//! directions, and tests stand a [`LocalEndpoint`] with its own scratch
//! directory in for a workspace.
//!
//! Every method blocks: run them from the blocking pool.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::blox;

use super::TransferError;

/// What a script left behind, whatever its exit status.
#[derive(Debug, Clone, Default)]
pub struct ScriptOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// One end of a transfer.
pub trait TransferEndpoint: Send + Sync {
    /// Where this end is, for logs and error messages.
    fn describe(&self) -> String;

    /// Run a POSIX `sh` script with positional arguments. `Err` means the
    /// script could not be run at all; a script that ran and failed comes
    /// back with `success: false`.
    fn run_script(&self, script: &str, args: &[String]) -> Result<ScriptOutput, TransferError>;

    /// [`Self::run_script`] for a script whose run time grows with
    /// `payload_bytes` (a snapshot, a bundle, a clone), so an end that gives
    /// up on a slow script waits long enough for this one.
    fn run_script_sized(
        &self,
        script: &str,
        args: &[String],
        payload_bytes: u64,
    ) -> Result<ScriptOutput, TransferError> {
        let _ = payload_bytes;
        self.run_script(script, args)
    }

    /// Where this end keeps the transfer's files: one directory per transfer.
    fn scratch_dir(&self, transfer_id: &str) -> Result<String, TransferError>;

    /// Copy `local`, on this machine, to `path` at this end.
    fn put_file(&self, local: &Path, path: &str) -> Result<(), TransferError>;

    /// Copy `path` at this end, expected to be `size` bytes, to `local` on
    /// this machine.
    fn get_file(&self, path: &str, local: &Path, size: u64) -> Result<(), TransferError>;

    /// `scratch_dir(transfer_id)/file_name`.
    fn scratch_path(&self, transfer_id: &str, file_name: &str) -> Result<String, TransferError> {
        Ok(format!("{}/{file_name}", self.scratch_dir(transfer_id)?))
    }
}

/// This machine. Scripts run under `shell` (normally `/bin/sh`) with the
/// captured login-shell environment of `env_dir` — Hermit-managed git, LFS,
/// credential helpers — the way every other git call here runs; and "moving"
/// a file is a copy, or nothing when it is already in place.
pub struct LocalEndpoint {
    shell: PathBuf,
    env_dir: PathBuf,
    scratch_root: PathBuf,
}

impl LocalEndpoint {
    /// An endpoint whose scripts see `env_dir`'s environment and whose
    /// transfer files live under `scratch_root/<transfer id>/`.
    pub fn new(env_dir: impl Into<PathBuf>, scratch_root: impl Into<PathBuf>) -> Self {
        Self {
            shell: PathBuf::from("/bin/sh"),
            env_dir: env_dir.into(),
            scratch_root: scratch_root.into(),
        }
    }

    /// Transfer files under `~/.staged/transfers/<transfer id>/`, where they
    /// stay as the backup.
    pub fn in_transfers_dir(env_dir: impl Into<PathBuf>) -> Result<Self, TransferError> {
        let root = crate::paths::transfers_dir().ok_or_else(|| TransferError::Exec {
            endpoint: "this machine".to_string(),
            message: "cannot resolve the home directory".to_string(),
        })?;
        Ok(Self::new(env_dir, root))
    }

    /// Run scripts under another POSIX shell, e.g. `/bin/dash`.
    pub fn with_shell(mut self, shell: impl Into<PathBuf>) -> Self {
        self.shell = shell.into();
        self
    }

    fn exec_error(&self, message: String) -> TransferError {
        TransferError::Exec {
            endpoint: self.describe(),
            message,
        }
    }

    /// Copy `from` to `to` through `<to>.part`, so `to` only appears whole.
    fn copy(&self, from: &Path, to: &Path) -> Result<(), TransferError> {
        if from == to {
            return Ok(());
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| self.exec_error(format!("cannot create {}: {e}", parent.display())))?;
        }
        let mut partial = to.as_os_str().to_owned();
        partial.push(".part");
        let partial = PathBuf::from(partial);
        std::fs::copy(from, &partial)
            .and_then(|_| std::fs::rename(&partial, to))
            .map_err(|e| {
                let _ = std::fs::remove_file(&partial);
                self.exec_error(format!(
                    "cannot copy {} to {}: {e}",
                    from.display(),
                    to.display()
                ))
            })
    }
}

impl TransferEndpoint for LocalEndpoint {
    fn describe(&self) -> String {
        "this machine".to_string()
    }

    fn run_script(&self, script: &str, args: &[String]) -> Result<ScriptOutput, TransferError> {
        let mut command = Command::new(&self.shell);
        command
            .arg("-c")
            .arg(script)
            .arg("staged-transfer")
            .args(args);
        crate::git::configure_captured(&mut command, &self.env_dir);
        let output = command
            .output()
            .map_err(|e| self.exec_error(format!("cannot run {}: {e}", self.shell.display())))?;
        Ok(ScriptOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
        })
    }

    fn scratch_dir(&self, transfer_id: &str) -> Result<String, TransferError> {
        let dir = self.scratch_root.join(transfer_id);
        dir.to_str()
            .map(str::to_string)
            .ok_or_else(|| self.exec_error(format!("not a UTF-8 path: {}", dir.display())))
    }

    fn put_file(&self, local: &Path, path: &str) -> Result<(), TransferError> {
        self.copy(local, Path::new(path))
    }

    fn get_file(&self, path: &str, local: &Path, _size: u64) -> Result<(), TransferError> {
        self.copy(Path::new(path), local)
    }
}

/// A Blox workspace. Scripts run through `ws exec`; files stream over
/// `ws ssh`, falling back to `ws upload` / `ws download` in parts.
pub struct BloxEndpoint {
    workspace: String,
}

impl BloxEndpoint {
    pub fn new(workspace: impl Into<String>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    fn exec_error(&self, e: impl std::fmt::Display) -> TransferError {
        TransferError::Exec {
            endpoint: self.describe(),
            message: e.to_string(),
        }
    }
}

impl TransferEndpoint for BloxEndpoint {
    fn describe(&self) -> String {
        format!("workspace {}", self.workspace)
    }

    fn run_script(&self, script: &str, args: &[String]) -> Result<ScriptOutput, TransferError> {
        self.run_script_sized(script, args, 0)
    }

    /// `ws exec` gives up after a fixed time; that time grows a second per
    /// megabyte of `payload_bytes`.
    fn run_script_sized(
        &self,
        script: &str,
        args: &[String],
        payload_bytes: u64,
    ) -> Result<ScriptOutput, TransferError> {
        let mut argv = vec!["sh", "-c", script, "staged-transfer"];
        argv.extend(args.iter().map(String::as_str));
        let output = blox::ws_exec_output_sized(&self.workspace, &argv, payload_bytes)
            .map_err(|e| self.exec_error(e))?;
        Ok(ScriptOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.success,
        })
    }

    /// `~/.staged-transfer/<transfer id>`, as an absolute path: `ws upload`
    /// only accepts destinations under the workstation's home.
    fn scratch_dir(&self, transfer_id: &str) -> Result<String, TransferError> {
        crate::branches::resolve_workspace_repo_path(
            &self.workspace,
            &format!("home:.staged-transfer/{transfer_id}"),
        )
        .map_err(|e| self.exec_error(e))
    }

    fn put_file(&self, local: &Path, path: &str) -> Result<(), TransferError> {
        let route =
            blox::ws_put_file(&self.workspace, local, path).map_err(|e| self.exec_error(e))?;
        log::info!(
            "[project_transfer] sent {} to {}:{path} via {route:?}",
            local.display(),
            self.workspace
        );
        Ok(())
    }

    fn get_file(&self, path: &str, local: &Path, size: u64) -> Result<(), TransferError> {
        let route = blox::ws_get_file(&self.workspace, path, local, size)
            .map_err(|e| self.exec_error(e))?;
        log::info!(
            "[project_transfer] fetched {}:{path} to {} via {route:?}",
            self.workspace,
            local.display()
        );
        Ok(())
    }
}
