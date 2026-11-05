//! What a transfer made before the database switched, and taking it back.
//!
//! The orchestrator records each thing in the manifest before it makes it,
//! so [`roll_back`] can undo a transfer that failed — and, at the next start,
//! one Staged never finished (see [`super::recovery`]). Since a record can
//! precede what it describes, or outlive it, every step checks what is there
//! first: a branch is deleted, and its config put back, only while it is
//! where the transfer left it; a remote is removed only while it has the URL
//! the transfer gave it; an `info/exclude` entry is taken out only while the
//! same line is there.

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
    /// Entries a finish adds to a local clone's `info/exclude`.
    pub excludes: Vec<AddedExcludes>,
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

/// Entries a finish adds to a local clone's `info/exclude`, for ignored
/// files only the source's own rules ignored. Every worktree of the clone
/// reads that file, and it outlives the worktree, so a rollback takes them
/// back out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AddedExcludes {
    /// Where the finish notes each change before making it (`file=`,
    /// `created=1`, `terminated=1`, then a `line=` per entry): what a
    /// rollback reads while `file` is unset, as when the finish, or Staged,
    /// stopped part way.
    pub journal: PathBuf,
    /// The `info/exclude` they went into, once the journal has been read.
    pub file: Option<PathBuf>,
    /// The finish created the file.
    pub created: bool,
    /// The file's last line had no newline, and the finish ended it before
    /// adding its own.
    pub terminated: bool,
    /// Each entry added, as written.
    pub lines: Vec<String>,
}

impl AddedExcludes {
    /// Entries the finish will note in `journal`.
    pub fn noted_in(journal: PathBuf) -> Self {
        Self {
            journal,
            ..Self::default()
        }
    }

    /// What the journal says was added, or `None` when there is no journal:
    /// nothing was.
    pub fn read_journal(journal: &Path) -> Result<Option<Self>, String> {
        let text = match std::fs::read(journal) {
            Ok(text) => String::from_utf8_lossy(&text).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("cannot read {}: {e}", journal.display())),
        };
        let mut added = Self::noted_in(journal.to_path_buf());
        for record in text.split('\n') {
            match record.split_once('=') {
                Some(("file", path)) if !path.is_empty() => added.file = Some(path.into()),
                Some(("created", "1")) => added.created = true,
                Some(("terminated", "1")) => added.terminated = true,
                Some(("line", line)) => added.lines.push(line.to_string()),
                _ => {}
            }
        }
        Ok(added.file.is_some().then_some(added))
    }

    /// These entries with what the journal says filled in, unless they
    /// already were.
    fn resolved(&self) -> Result<Option<Self>, String> {
        if self.file.is_some() {
            Ok(Some(self.clone()))
        } else {
            Self::read_journal(&self.journal)
        }
    }
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
    for added in undo.excludes.iter().rev() {
        if let Err(e) = remove_added_excludes(added) {
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

/// Take the entries `added` names back out of its `info/exclude`: for each,
/// the last line that is still exactly it. What anyone else wrote stays
/// as it is; the newline that ended the file's last line goes again when
/// the entries were still the last lines; and the file goes when the
/// transfer created it and nothing else is left in it.
pub fn remove_added_excludes(added: &AddedExcludes) -> Result<(), String> {
    let Some(added) = added.resolved()? else {
        return Ok(());
    };
    let Some(file) = &added.file else {
        return Ok(());
    };
    remove_exclude_lines(file, &added.lines, added.created, added.terminated)
}

/// Take `lines` back out of the `info/exclude` at `file`, as
/// [`remove_added_excludes`] does: `created` when the transfer made the file,
/// `terminated` when it ended the file's last line before adding them.
pub fn remove_exclude_lines(
    file: &Path,
    lines: &[String],
    created: bool,
    terminated: bool,
) -> Result<(), String> {
    let text = match std::fs::read(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot read {}: {e}", file.display())),
    };
    let Some(kept) = without_lines(&text, lines, terminated) else {
        return Ok(());
    };
    let written = if kept.is_empty() && created {
        std::fs::remove_file(file)
    } else {
        std::fs::write(file, kept)
    };
    written.map_err(|e| {
        format!(
            "cannot take the transfer's entries out of {}: {e}",
            file.display()
        )
    })
}

/// `text` without the last line exactly matching each of `lines`, or `None`
/// when none is there. With `terminated`, the final newline goes too when
/// the lines taken out were the last ones.
fn without_lines(text: &[u8], lines: &[String], terminated: bool) -> Option<Vec<u8>> {
    let ends_in_newline = text.last() == Some(&b'\n');
    let body = if ends_in_newline {
        &text[..text.len() - 1]
    } else {
        text
    };
    let mut present: Vec<Option<&[u8]>> = if text.is_empty() {
        Vec::new()
    } else {
        body.split(|b| *b == b'\n').map(Some).collect()
    };
    let mut taken = 0;
    for line in lines.iter().rev() {
        if let Some(i) = present.iter().rposition(|l| *l == Some(line.as_bytes())) {
            present[i] = None;
            taken += 1;
        }
    }
    if taken == 0 {
        return None;
    }
    let were_last = present.iter().rev().take(taken).all(Option::is_none);
    let kept: Vec<&[u8]> = present.into_iter().flatten().collect();
    let mut out = kept.join(&b'\n');
    if !kept.is_empty() && ends_in_newline && !(terminated && were_last) {
        out.push(b'\n');
    }
    Some(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|e| e.to_string()).collect()
    }

    fn without(text: &str, entries: &[&str], terminated: bool) -> Option<String> {
        without_lines(text.as_bytes(), &lines(entries), terminated)
            .map(|out| String::from_utf8(out).unwrap())
    }

    #[test]
    fn takes_out_only_the_lines_named_leaving_the_rest_as_it_was() {
        // Before them, after them, and among them.
        assert_eq!(
            without("# x\n.penpal\n/a\n/b\n*.swp\n", &["/a", "/b"], false).as_deref(),
            Some("# x\n.penpal\n*.swp\n")
        );
        // A copy someone else wrote before stays; the last one goes.
        assert_eq!(
            without("/a\nmine\n/a\n", &["/a"], false).as_deref(),
            Some("/a\nmine\n")
        );
        // Without a final newline, the file stays without one.
        assert_eq!(
            without("x\n/a\nlater", &["/a"], false).as_deref(),
            Some("x\nlater")
        );
        // Blank lines are lines.
        assert_eq!(without("\n\n/a\n", &["/a"], false).as_deref(), Some("\n\n"));
        // Nothing to take out: leave the file be.
        assert_eq!(without("x\n", &["/a"], false), None);
        assert_eq!(without("", &["/a"], false), None);
        // Only the transfer's: nothing is left.
        assert_eq!(
            without("/a\n/b\n", &["/a", "/b"], false).as_deref(),
            Some("")
        );
    }

    #[test]
    fn the_newline_that_ended_the_last_line_goes_only_with_the_last_lines() {
        assert_eq!(
            without(".penpal\n/a\n/b\n", &["/a", "/b"], true).as_deref(),
            Some(".penpal")
        );
        // Someone wrote after them: the newline ends `.penpal` before theirs.
        assert_eq!(
            without(".penpal\n/a\nlater\n", &["/a"], true).as_deref(),
            Some(".penpal\nlater\n")
        );
    }

    #[test]
    fn a_rollback_reads_the_journal_until_the_entries_are_recorded() {
        let root = crate::test_utils::TempDir::new("transfer-undo-excludes");
        let exclude = root.path().join("info/exclude");
        std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        let journal = root.path().join("0-widgets.info-exclude.added");

        // No journal: the finish never got as far as adding any.
        std::fs::write(&exclude, "# x\n").unwrap();
        remove_added_excludes(&AddedExcludes::noted_in(journal.clone())).unwrap();
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), "# x\n");

        // Noted, then cut off before the last was added.
        std::fs::write(
            &journal,
            format!(
                "file={}\nterminated=1\nline=/a\nline=/b =c\nline=/never\n",
                exclude.display()
            ),
        )
        .unwrap();
        std::fs::write(&exclude, "# x\n.penpal\n/a\n/b =c\n").unwrap();
        let noted = AddedExcludes::read_journal(&journal).unwrap().unwrap();
        assert_eq!(noted.file.as_deref(), Some(exclude.as_path()));
        assert!(noted.terminated && !noted.created);
        assert_eq!(noted.lines, lines(&["/a", "/b =c", "/never"]));
        remove_added_excludes(&AddedExcludes::noted_in(journal.clone())).unwrap();
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), "# x\n.penpal");

        // Recorded: the journal is no longer read, and a file the transfer
        // made goes once it is empty again.
        std::fs::remove_file(&journal).unwrap();
        std::fs::write(&exclude, "/a\n").unwrap();
        let made = AddedExcludes {
            file: Some(exclude.clone()),
            created: true,
            lines: lines(&["/a"]),
            ..AddedExcludes::noted_in(journal.clone())
        };
        remove_added_excludes(&made).unwrap();
        assert!(!exclude.exists());
        // Already gone: nothing to do.
        remove_added_excludes(&made).unwrap();
        // Made by the transfer, written to since: it stays, with theirs.
        std::fs::write(&exclude, "/a\nlater\n").unwrap();
        remove_added_excludes(&made).unwrap();
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), "later\n");
    }
}
