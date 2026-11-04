//! What a transfer made before the database switched, and taking it back.
//!
//! The orchestrator records each thing in the manifest before it makes it,
//! so [`roll_back`] can undo a transfer that failed — and, at the next start,
//! one Staged never finished (see [`super::recovery`]). Since a record can
//! precede what it describes, or outlive it, every step checks what is there
//! first: a branch is deleted, and its config put back, only while it is
//! where the transfer left it; a remote is removed only while it has the URL
//! the transfer gave it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::carry::cleanup;
use super::host::TransferHost;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Undo {
    /// The workspace created for a transfer to remote.
    pub workspace: Option<String>,
    /// Checkouts snapshotted at the source: their temporary refs, and a
    /// workspace's scratch directory.
    pub sources: Vec<SourceCheckout>,
    /// Local clones a transfer to local fetches bundles into: their
    /// temporary refs.
    pub clones: Vec<PathBuf>,
    /// Local branches the apply creates.
    pub branches: Vec<CreatedBranch>,
    /// Every local branch's `branch.<b>.*` config before the apply replaces
    /// it.
    pub branch_configs: Vec<ReplacedConfig>,
    /// Worktrees added for the carried branches.
    pub worktrees: Vec<AddedWorktree>,
    /// Remotes the apply adds to a local clone.
    pub remotes: Vec<AddedRemote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCheckout {
    /// `None`: this machine.
    pub workspace: Option<String>,
    pub dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedBranch {
    pub clone: PathBuf,
    pub branch: String,
    /// Where the apply puts it.
    pub tip: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplacedConfig {
    pub clone: PathBuf,
    pub branch: String,
    /// Where the apply puts the branch: its config is only put back while it
    /// is there, or gone.
    pub head: String,
    pub entries: BranchConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddedWorktree {
    pub clone: PathBuf,
    pub worktree: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddedRemote {
    pub clone: PathBuf,
    pub remote: String,
    /// The URL the apply gives it.
    pub url: String,
}

/// A branch's `branch.<branch>.*` config, as `(key, value)` in order.
pub type BranchConfig = Vec<(String, String)>;

/// Undo everything in `undo`, best-effort: the destination first, then the
/// temporary refs at both ends. The bundles on this machine stay. Returns
/// what it left, one sentence each.
pub fn roll_back(undo: &Undo, host: &dyn TransferHost, transfer_id: &str) -> Vec<String> {
    let mut left = Vec::new();
    if let Some(workspace) = &undo.workspace {
        if let Err(e) = host.delete_workspace(workspace) {
            left.push(format!("cannot delete workspace {workspace}: {e}"));
        }
    }
    for added in undo.worktrees.iter().rev() {
        if let Err(e) = remove_worktree_keeping_branch(&added.clone, &added.worktree) {
            left.push(e);
        }
    }
    for created in &undo.branches {
        let branch = &created.branch;
        match branch_tip(&created.clone, branch) {
            Ok(None) => {}
            Ok(Some(tip)) if tip == created.tip => {
                let branch_ref = format!("refs/heads/{branch}");
                if let Err(e) =
                    crate::git::cli_run(&created.clone, &["update-ref", "-d", &branch_ref, &tip])
                {
                    left.push(format!("cannot delete branch {branch}: {e}"));
                }
            }
            Ok(Some(tip)) => left.push(format!(
                "kept branch {branch} in {}: it moved to {tip} since",
                created.clone.display()
            )),
            Err(e) => left.push(format!("cannot read branch {branch}: {e}")),
        }
    }
    // Before the branch config goes back: removing a remote also unsets the
    // config of the branches that track it.
    for added in &undo.remotes {
        let remote = &added.remote;
        match remote_url(&added.clone, remote) {
            None => {}
            Some(url) if url == added.url => {
                if let Err(e) = crate::git::cli_run(&added.clone, &["remote", "remove", remote]) {
                    left.push(format!("cannot remove remote {remote}: {e}"));
                }
            }
            Some(url) => left.push(format!(
                "kept remote {remote} in {}: it points at {url} now",
                added.clone.display()
            )),
        }
    }
    for replaced in undo.branch_configs.iter().rev() {
        let branch = &replaced.branch;
        match branch_tip(&replaced.clone, branch) {
            // Gone (this rollback deleted the branch the apply created) or
            // still where the transfer put it: the config is the transfer's.
            Ok(tip) if tip.is_none() || tip.as_deref() == Some(replaced.head.as_str()) => {
                if let Err(e) = restore_branch_config(&replaced.clone, branch, &replaced.entries) {
                    left.push(format!("cannot restore {branch}'s config: {e}"));
                }
            }
            // At another tip, the branch (and its config) is someone else's
            // now.
            Ok(_) => {}
            Err(e) => left.push(format!("cannot read branch {branch}: {e}")),
        }
    }
    for clone in &undo.clones {
        let result = match (host.local_endpoint(clone), clone.to_str()) {
            (Ok(endpoint), Some(dir)) => {
                cleanup(endpoint.as_ref(), dir, transfer_id, false).map_err(|e| e.to_string())
            }
            (Err(e), _) => Err(e),
            (_, None) => Err(format!("not a UTF-8 path: {}", clone.display())),
        };
        if let Err(e) = result {
            left.push(format!(
                "cannot drop the temporary refs in {}: {e}",
                clone.display()
            ));
        }
    }
    for source in &undo.sources {
        let result = match &source.workspace {
            Some(workspace) => cleanup(
                host.workspace_endpoint(workspace).as_ref(),
                &source.dir,
                transfer_id,
                true,
            )
            .map_err(|e| e.to_string()),
            None => host
                .local_endpoint(Path::new(&source.dir))
                .and_then(|endpoint| {
                    cleanup(endpoint.as_ref(), &source.dir, transfer_id, false)
                        .map_err(|e| e.to_string())
                }),
        };
        if let Err(e) = result {
            left.push(format!(
                "cannot drop the temporary refs in {}: {e}",
                source.dir
            ));
        }
    }
    for what in &left {
        log::warn!("[project_transfer] rollback of {transfer_id}: {what}");
    }
    left
}

/// Where `refs/heads/<branch>` is in `clone`, if it exists.
pub fn branch_tip(clone: &Path, branch: &str) -> Result<Option<String>, String> {
    let branch_ref = format!("refs/heads/{branch}");
    let out = crate::git::cli_run(
        clone,
        &["for-each-ref", "--format=%(objectname)", &branch_ref],
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(out.trim().to_string()).filter(|tip| !tip.is_empty()))
}

/// `remote.<remote>.url` in `clone`'s own config, if set.
pub fn remote_url(clone: &Path, remote: &str) -> Option<String> {
    crate::git::cli_run(
        clone,
        &[
            "config",
            "--local",
            "--get",
            &format!("remote.{remote}.url"),
        ],
    )
    .ok()
    .map(|url| url.trim_end_matches('\n').to_string())
}

/// `branch.<branch>.*` in `clone`'s own config. A variable set without a
/// value reads as `true`, as git takes it.
pub fn branch_config(clone: &Path, branch: &str) -> Result<BranchConfig, String> {
    let listing = crate::git::cli_run(clone, &["config", "--local", "-z", "--list"])
        .map_err(|e| e.to_string())?;
    let prefix = format!("branch.{branch}.");
    Ok(listing
        .split('\0')
        .filter_map(|entry| {
            let (key, value) = entry.split_once('\n').unwrap_or((entry, "true"));
            let var = key.strip_prefix(&prefix)?;
            (!var.is_empty() && !var.contains('.')).then(|| (key.to_string(), value.to_string()))
        })
        .collect())
}

/// Replace `branch.<branch>.*` in `clone` with `entries`, as
/// [`branch_config`] read them.
fn restore_branch_config(
    clone: &Path,
    branch: &str,
    entries: &[(String, String)],
) -> Result<(), String> {
    // Fails when there is no such section, which is as good as removed.
    let _ = crate::git::cli_run(
        clone,
        &[
            "config",
            "--local",
            "--remove-section",
            &format!("branch.{branch}"),
        ],
    );
    for (key, value) in entries {
        crate::git::cli_run(clone, &["config", "--local", "--add", key, value])
            .map_err(|e| format!("cannot set {key}: {e}"))?;
    }
    Ok(())
}

/// Remove a worktree the transfer added, without `remove_worktree`'s branch
/// deletion: the branch is the rollback's to decide about.
fn remove_worktree_keeping_branch(clone: &Path, worktree: &Path) -> Result<(), String> {
    let Some(path) = worktree.to_str() else {
        return Err(format!("not a UTF-8 path: {}", worktree.display()));
    };
    if crate::git::cli_run(clone, &["worktree", "remove", "--force", path]).is_ok() {
        return Ok(());
    }
    let removed = if worktree.exists() {
        std::fs::remove_dir_all(worktree).map_err(|e| format!("cannot remove {path}: {e}"))
    } else {
        Ok(())
    };
    let _ = crate::git::cli_run(clone, &["worktree", "prune"]);
    removed
}
