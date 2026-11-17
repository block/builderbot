//! A branch's local worktree, started while its capture still runs.
//!
//! Coming to local, a worktree needs nothing from the capture but the
//! commit to check out, and the clone already holds one nearby: the head
//! the inspection saw, or what the destination just fetched from origin.
//! So the leg adds the worktree detached at that commit, and starts the
//! capture of its shell environment, on a thread of its own while the
//! snapshot, the download and the landing run; once the branch has landed
//! it only switches the worktree onto it, which rewrites the files that
//! differ.
//!
//! This is an optimisation with a complete fallback. A leg with no commit
//! to start from, an early add or a switch that fails, or a guess about
//! Git LFS the snapshot contradicts, all end in the sequence without it:
//! `git worktree add <path> <branch>` once the branch has landed. The
//! worktree is recorded in the undo list before the thread starts and the
//! thread is joined before the leg returns, so rollback and startup recovery
//! find it as they find a worktree added late.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::super::carry::{CarryOutcome, CarryPlan};
use super::super::undo::{self, AddedWorktree};
use super::{origin_tip_names, Failure, Leg, Run};

#[cfg(test)]
thread_local! {
    /// A test's request that early adds planned from this thread fail.
    static FAIL_EARLY_ADDS_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While it lives, the early worktree adds of legs run from this thread
/// fail part way, leaving a directory git knows nothing of — for a test of
/// the fallback. The legs of a one-repository transfer, or of one under
/// [`super::SerialLegs`], run on the thread that runs the transfer.
#[cfg(test)]
pub(crate) struct FailingEarlyAdds;

#[cfg(test)]
impl FailingEarlyAdds {
    pub(crate) fn for_this_thread() -> Self {
        FAIL_EARLY_ADDS_FOR_TEST.with(|f| f.set(true));
        Self
    }
}

#[cfg(test)]
impl Drop for FailingEarlyAdds {
    fn drop(&mut self) {
        FAIL_EARLY_ADDS_FOR_TEST.with(|f| f.set(false));
    }
}

/// A worktree to add before its branch exists. Owns all it needs, so the
/// thread adding it borrows nothing from the leg.
pub(super) struct EarlyWorktree {
    clone: PathBuf,
    path: PathBuf,
    /// The commit it is added at, which the clone already holds.
    start: String,
    /// Whether the snapshot is expected to bring LFS objects, in which case
    /// LFS files check out as pointers for the finish to replace.
    skip_lfs_smudge: bool,
    #[cfg(test)]
    fail: bool,
}

/// How an early add went.
pub(super) struct EarlyAdd {
    result: Result<(), String>,
    took: Duration,
}

impl EarlyWorktree {
    /// Add the worktree, and start capturing its shell environment so the
    /// finish finds it cached or waits only for the rest. Hooks stay off:
    /// `post-checkout` fires once, when the worktree goes onto its branch.
    pub(super) fn add(&self) -> EarlyAdd {
        let started = Instant::now();
        let result = self.add_detached();
        if result.is_ok() {
            // Whatever is cached for the path is from a worktree that was
            // there before.
            crate::git::invalidate_shell_env(&self.path);
            crate::git::prewarm_shell_env(&self.path);
        }
        EarlyAdd {
            result,
            took: started.elapsed(),
        }
    }

    fn add_detached(&self) -> Result<(), String> {
        #[cfg(test)]
        if self.fail {
            std::fs::create_dir_all(&self.path).map_err(|e| e.to_string())?;
            std::fs::write(self.path.join("half-added"), "").map_err(|e| e.to_string())?;
            return Err("injected failure".to_string());
        }
        crate::git::create_detached_worktree_without_hooks(
            &self.clone,
            &self.start,
            &self.path,
            self.skip_lfs_smudge,
        )
        .map(drop)
        .map_err(|e| e.to_string())
    }
}

/// How a leg's worktree came to be, for the log.
#[derive(Default)]
pub(super) struct WorktreeTimings {
    /// The early add and the commit it started from.
    early_add: Option<(Duration, String)>,
    switch: Option<Duration>,
    /// The add once the branch had landed: the only one, or the fallback's.
    late_add: Option<Duration>,
    /// Why the early worktree was discarded, when it was.
    fell_back: Option<String>,
    pub(super) environment: Duration,
    pub(super) finish: Duration,
}

impl WorktreeTimings {
    pub(super) fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some((took, start)) = &self.early_add {
            parts.push(format!("early add at {start} {took:.1?}"));
        }
        if let Some(took) = self.switch {
            parts.push(format!("switch {took:.1?}"));
        }
        if let Some(took) = self.late_add {
            parts.push(format!("late add {took:.1?}"));
        }
        parts.push(format!("environment {:.1?}", self.environment));
        parts.push(format!("finish {:.1?}", self.finish));
        parts.push(match &self.fell_back {
            Some(why) => format!("fell back ({why})"),
            None => "no fallback".to_string(),
        });
        parts.join(", ")
    }
}

/// The remote-tracking refs a leg's destination fetched from origin, in the
/// order an early worktree prefers them: the branch, its upstream, then
/// the base.
fn start_refs(leg: &Leg) -> Vec<String> {
    // The base comes first there.
    let names = origin_tip_names(leg);
    names
        .iter()
        .skip(1)
        .chain(names.first())
        .map(|name| format!("refs/remotes/origin/{name}"))
        .collect()
}

/// The commit to add an early worktree at: `head`, the source's head as
/// the inspection saw it, when `clone` holds it (a branch pushed in full,
/// or one this machine has carried before), else the first of `refs` that
/// resolves. `None` when the clone holds none of them.
pub(super) fn start_commit(clone: &Path, head: Option<&str>, refs: &[String]) -> Option<String> {
    let commit = |rev: &str| {
        // Peeling makes git read the object, so a head the clone lacks
        // fails here instead of in the checkout.
        let peeled = format!("{rev}^{{commit}}");
        crate::git::cli_run(clone, &["rev-parse", "--verify", "--quiet", &peeled])
            .ok()
            .map(|out| out.trim().to_string())
            .filter(|id| !id.is_empty())
    };
    head.and_then(commit)
        .or_else(|| refs.iter().find_map(|r| commit(r)))
}

/// Whether a shell environment captured in a worktree at `start` may differ
/// from one captured after it moved to `head`: whether the two commits
/// differ in anything a capture at the worktree's root reads
/// ([`crate::shell_env::env_root_inputs`]). Not being able to tell counts
/// as differing, which costs one more capture.
pub(super) fn env_capture_is_stale(clone: &Path, start: &str, head: &str) -> bool {
    if start == head {
        return false;
    }
    let mut args = vec!["diff", "--name-only", "--no-renames", start, head, "--"];
    for input in crate::shell_env::env_root_inputs() {
        args.push(input);
    }
    match crate::git::cli_run(clone, &args) {
        Ok(changed) => !changed.trim().is_empty(),
        Err(e) => {
            log::warn!(
                "[project_transfer] cannot compare {start} and {head} in {}: {e}",
                clone.display()
            );
            true
        }
    }
}

impl Run<'_> {
    /// Before the leg's capture starts: pick the commit and the path for
    /// its worktree and record it, as the late add would have. `None` leaves
    /// the worktree to the late add, which reports whatever stood in the
    /// way here.
    pub(super) fn plan_early_worktree(&self, leg: &Leg, clone: &Path) -> Option<EarlyWorktree> {
        let branch = &leg.dest_branch;
        let start = start_commit(clone, leg.row.head.as_deref(), &start_refs(leg))?;
        let planned = self.free_worktree_path(leg).and_then(|path| {
            self.record(|undo| {
                undo.worktrees.push(AddedWorktree {
                    clone: clone.to_path_buf(),
                    worktree: path.clone(),
                })
            })?;
            Ok(path)
        });
        let path = match planned {
            Ok(path) => path,
            Err(e) => {
                log::warn!("[project_transfer] no early worktree for {branch}: {e}");
                return None;
            }
        };
        Some(EarlyWorktree {
            clone: clone.to_path_buf(),
            path,
            start,
            skip_lfs_smudge: leg.row.lfs_present + leg.row.lfs_missing > 0,
            #[cfg(test)]
            fail: FAIL_EARLY_ADDS_FOR_TEST.with(|f| f.get()),
        })
    }

    /// Once the leg's branch has landed: put it in its worktree. The early
    /// worktree is switched onto the branch; without one, or when it or the
    /// switch failed, or its LFS guess was wrong, the worktree is added on
    /// the branch as it always was — at the early one's path, once that is
    /// cleared.
    pub(super) fn settle_worktree(
        &self,
        leg: &Leg,
        plan: &CarryPlan,
        outcome: &CarryOutcome,
        early: Option<(EarlyWorktree, EarlyAdd)>,
    ) -> Result<(PathBuf, WorktreeTimings), Failure> {
        let clone = PathBuf::from(&plan.dest_dir);
        let branch = &leg.dest_branch;
        // LFS files check out as pointers, which `finish` replaces with
        // the objects that came: a smudge filter would download each one
        // again, and fail the checkout on any it can't.
        let skip_lfs_smudge = outcome.snapshot.lfs.is_some();
        let mut timings = WorktreeTimings::default();

        let worktree = match early {
            Some((early, add)) => {
                timings.early_add = Some((add.took, early.start.clone()));
                let added = add.result.is_ok();
                let switching = Instant::now();
                let switched = add
                    .result
                    .map_err(|e| format!("the early add failed: {e}"))
                    .and_then(|()| {
                        if early.skip_lfs_smudge == skip_lfs_smudge {
                            Ok(())
                        } else {
                            Err(format!(
                                "it was added {} LFS objects to come, and the snapshot {}",
                                if early.skip_lfs_smudge {
                                    "expecting"
                                } else {
                                    "not expecting"
                                },
                                if skip_lfs_smudge {
                                    "brought some"
                                } else {
                                    "brought none"
                                },
                            ))
                        }
                    })
                    .and_then(|()| {
                        crate::git::switch_detached_worktree_to_branch(
                            &clone,
                            &early.path,
                            branch,
                            skip_lfs_smudge,
                        )
                        .map_err(|e| format!("cannot switch it to {branch}: {e}"))
                    });
                match switched {
                    Ok(()) => {
                        timings.switch = Some(switching.elapsed());
                        if env_capture_is_stale(&clone, &early.start, &outcome.snapshot.head) {
                            crate::git::invalidate_shell_env(&early.path);
                        }
                        return Ok((early.path, timings));
                    }
                    Err(why) => {
                        log::warn!(
                            "[project_transfer] transfer {}: discarding the early worktree \
                             for {branch} at {}: {why}",
                            self.guard.transfer_id(),
                            early.path.display()
                        );
                        if added {
                            // The capture it started runs in the directory
                            // about to go.
                            crate::git::wait_for_shell_env(&early.path);
                        }
                        undo::remove_worktree_keeping_branch(&clone, &early.path)
                            .map_err(|e| format!("Cannot add the worktree for {branch}: {e}"))?;
                        crate::git::invalidate_shell_env(&early.path);
                        timings.fell_back = Some(why);
                        early.path
                    }
                }
            }
            None => {
                let worktree = self.free_worktree_path(leg)?;
                self.record(|undo| {
                    undo.worktrees.push(AddedWorktree {
                        clone: clone.clone(),
                        worktree: worktree.clone(),
                    })
                })?;
                worktree
            }
        };

        let adding = Instant::now();
        let add_worktree = if skip_lfs_smudge {
            crate::git::create_worktree_for_existing_branch_without_lfs_smudge
        } else {
            crate::git::create_worktree_for_existing_branch_at_path
        };
        add_worktree(&clone, branch, &worktree)
            .map_err(|e| format!("Cannot add the worktree for {branch}: {e}"))?;
        timings.late_add = Some(adding.elapsed());
        Ok((worktree, timings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TempGitRepo;

    fn repo_with_base() -> (TempGitRepo, String) {
        let repo = TempGitRepo::new();
        repo.write_file("a.txt", "alpha\n");
        let base = repo.commit("base");
        (repo, base)
    }

    fn commit_file(repo: &TempGitRepo, name: &str) -> String {
        if let Some(parent) = Path::new(name).parent() {
            std::fs::create_dir_all(repo.path().join(parent)).unwrap();
        }
        repo.write_file(name, "content\n");
        repo.commit(name)
    }

    #[test]
    fn a_capture_is_stale_only_when_what_it_reads_changed() {
        let (repo, base) = repo_with_base();
        assert!(!env_capture_is_stale(repo.path(), &base, &base));

        // Source files, and a marker below the root, are not read by a
        // capture at the root.
        commit_file(&repo, "src/main.rs");
        commit_file(&repo, "apps/web/.nvmrc");
        let unrelated = commit_file(&repo, "sbin/tool");
        assert!(!env_capture_is_stale(repo.path(), &base, &unrelated));

        for input in [
            "bin/activate-hermit",
            "bin/hermit.hcl",
            ".envrc",
            ".mise/config.toml",
        ] {
            let before = repo.run_git(&["rev-parse", "HEAD"]).trim().to_string();
            let after = commit_file(&repo, input);
            assert!(
                env_capture_is_stale(repo.path(), &before, &after),
                "{input}"
            );
            // Whichever way the worktree moved.
            assert!(
                env_capture_is_stale(repo.path(), &after, &before),
                "{input}"
            );
            assert!(env_capture_is_stale(repo.path(), &base, &after), "{input}");
        }
    }

    #[test]
    fn a_capture_counts_as_stale_when_the_commits_cannot_be_compared() {
        let (repo, base) = repo_with_base();
        let missing = "0123456789abcdef0123456789abcdef01234567";
        assert!(env_capture_is_stale(repo.path(), &base, missing));
    }

    #[test]
    fn the_start_commit_is_the_head_when_the_clone_holds_it_else_the_first_ref_it_has() {
        let (repo, base) = repo_with_base();
        let tip = commit_file(&repo, "b.txt");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &base]);
        repo.run_git(&["update-ref", "refs/remotes/origin/feature", &tip]);
        let refs = |names: &[&str]| -> Vec<String> {
            names
                .iter()
                .map(|n| format!("refs/remotes/origin/{n}"))
                .collect()
        };
        let missing = "0123456789abcdef0123456789abcdef01234567";

        assert_eq!(
            start_commit(repo.path(), Some(&base), &refs(&["feature", "main"])),
            Some(base.clone()),
            "the head, though a ref is ahead of it"
        );
        assert_eq!(
            start_commit(repo.path(), Some(missing), &refs(&["feature", "main"])),
            Some(tip.clone()),
            "a head the clone lacks"
        );
        assert_eq!(
            start_commit(repo.path(), None, &refs(&["gone", "main"])),
            Some(base),
            "a ref origin never had"
        );
        assert_eq!(
            start_commit(repo.path(), Some(missing), &refs(&["gone"])),
            None
        );
        // A tree is not something to check out.
        let tree = repo
            .run_git(&["rev-parse", "HEAD^{tree}"])
            .trim()
            .to_string();
        assert_eq!(start_commit(repo.path(), Some(&tree), &[]), None);
    }
}
