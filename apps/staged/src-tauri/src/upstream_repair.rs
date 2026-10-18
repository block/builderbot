//! Startup repair for de-duplicated PR branches that lost their upstream.
//!
//! Importing a PR whose head branch is already checked out in another
//! project's worktree gives the import a local `feature-x-2`, which
//! `create_and_link_worktree` makes track `origin/feature-x`. Before pushes
//! used an explicit `<local>:<remote>` refspec, the first `git push -u origin
//! feature-x-2` created a stray `origin/feature-x-2` and re-pointed the
//! upstream at it, so from then on every push, pull and comparison used the
//! wrong branch.
//!
//! This walks the local branches that can still be in that state — linked to
//! a PR, named `<stem>-<digits>`, and resolving their push target to their
//! own name — asks `gh pr view <pr> --json headRefName` what the PR's head
//! really is, and when it is exactly `<stem>` runs `git branch
//! --set-upstream-to=origin/<stem> <local>`. Nothing is recorded: a repaired
//! branch resolves to `<stem>` on the next launch and drops out of the walk.
//!
//! `<stem>-<digits>` also matches ticket-suffixed names (`fix-login-1234`),
//! whose PR head is their own name. Those cost one `gh` round-trip per launch
//! and are left alone. When `gh` is missing, logged out or hanging that
//! round-trip fails identically for every candidate, so the first such
//! failure disables `gh` for the rest of the run; a failure specific to one
//! PR (deleted, or in a repo the user can no longer see) skips only that
//! branch. Stray `origin/feature-x-2` branches already pushed are not
//! deleted. Blox workspaces are not walked: each candidate would cost a
//! workspace exec, and workspace branches are never de-duplicated.

use std::path::Path;
use std::sync::Arc;

use crate::git::{github, GitError};
use crate::store::{self, Store};
use crate::{git, prs};

/// The stem of a name shaped like `<stem>-<digits>` — the shape
/// `unique_branch_name` produces. `None` for anything else, including a
/// bare `-2` with no stem.
pub(crate) fn dedup_stem(branch_name: &str) -> Option<&str> {
    let (stem, suffix) = branch_name.rsplit_once('-')?;
    if stem.is_empty() || suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(stem)
}

/// Decide which branch on origin a candidate should be re-pointed at.
///
/// `current_upstream` is what the branch resolves to today (see
/// `git::upstream_branch_name`); anything other than the branch's own name
/// means it already tracks something and is left alone. `pr_head` is only
/// consulted past that point, since it costs a `gh` round-trip. The only
/// trusted answer is the de-dup stem: a head equal to the local name is a
/// ticket-suffixed branch that was never de-duplicated, and anything else
/// means the PR link is stale or loose, not a push target.
pub(crate) fn decide_repair(
    branch_name: &str,
    current_upstream: &str,
    pr_head: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let stem = dedup_stem(branch_name)?;
    if current_upstream != branch_name {
        return None;
    }
    let head = pr_head().filter(|head| !head.is_empty())?;
    if head == stem {
        return Some(head);
    }
    if head != branch_name {
        log::warn!(
            "[upstream_repair] PR head '{head}' for '{branch_name}' is neither '{stem}' nor the \
             local name; leaving its upstream alone"
        );
    }
    None
}

/// Looks up a PR's head branch via `gh`, giving up for the rest of the run
/// once `gh` itself proves unusable.
///
/// Only the global failures trip that breaker — `gh` not installed, not
/// authenticated, failing to spawn or timing out (see
/// [`github::is_gh_unavailable_error`]) — because those fail identically for
/// every candidate. Anything else is `gh` answering about one PR (deleted,
/// inaccessible): that branch is skipped and the walk carries on, so one
/// dead PR can't starve the rest.
pub(crate) struct PrHeadLookup<F> {
    lookup: F,
    unavailable: bool,
}

impl<F> PrHeadLookup<F>
where
    F: FnMut(&str, u64) -> Result<String, GitError>,
{
    pub(crate) fn new(lookup: F) -> Self {
        Self {
            lookup,
            unavailable: false,
        }
    }

    /// The head branch of `pr_number` in `repo_slug`, or `None` when `gh`
    /// could not say — including when an earlier call this run found `gh`
    /// unusable, in which case `gh` is not invoked at all.
    pub(crate) fn head(&mut self, repo_slug: &str, pr_number: u64) -> Option<String> {
        if self.unavailable {
            return None;
        }
        match (self.lookup)(repo_slug, pr_number) {
            Ok(head) => Some(head),
            Err(e) if github::is_gh_unavailable_error(&e) => {
                log::warn!(
                    "[upstream_repair] gh pr view #{pr_number} in {repo_slug} failed: {e}; \
                     skipping the remaining gh lookups until the next launch"
                );
                self.unavailable = true;
                None
            }
            Err(e) => {
                log::warn!(
                    "[upstream_repair] gh pr view #{pr_number} in {repo_slug} failed: {e}; \
                     skipping this branch"
                );
                None
            }
        }
    }
}

/// The two git operations the walk needs on a worktree, so it can be driven
/// without repositories in tests.
pub(crate) trait WorktreeUpstream {
    /// The branch on origin `local` currently pushes to.
    fn current(&mut self, worktree: &Path, local: &str, base_branch: &str) -> String;

    /// Make `local` track `origin/<remote>`. `Ok(false)` when origin has no
    /// such branch.
    fn repoint(&mut self, worktree: &Path, local: &str, remote: &str) -> Result<bool, String>;
}

struct GitWorktreeUpstream;

impl WorktreeUpstream for GitWorktreeUpstream {
    fn current(&mut self, worktree: &Path, local: &str, base_branch: &str) -> String {
        git::upstream_branch_name(worktree, local, base_branch)
    }

    fn repoint(&mut self, worktree: &Path, local: &str, remote: &str) -> Result<bool, String> {
        // `origin/<remote>` has to exist locally before it can be tracked;
        // a clone that only ever fetched `refs/pull/N/head` may not have it.
        // A failed fetch surfaces as `Ok(false)` below.
        let _ = git::cli_run(worktree, &["fetch", "origin", remote]);
        git::set_upstream_to_origin(worktree, local, remote).map_err(|e| e.to_string())
    }
}

/// Run the repair over every candidate branch. Blocking — callers run it
/// off the startup critical path.
pub(crate) fn run(store: &Arc<Store>) {
    let mut lookup = PrHeadLookup::new(|repo_slug: &str, pr_number| {
        github::get_pr_for_repo(repo_slug, pr_number).map(|pr| pr.head_ref)
    });
    run_with(store, &mut lookup, &mut GitWorktreeUpstream);
}

/// [`run`] with the `gh` lookup and the git operations injected.
pub(crate) fn run_with<F>(
    store: &Arc<Store>,
    lookup: &mut PrHeadLookup<F>,
    worktrees: &mut impl WorktreeUpstream,
) where
    F: FnMut(&str, u64) -> Result<String, GitError>,
{
    let ids = match store.list_branch_ids() {
        Ok(ids) => ids,
        Err(e) => {
            log::warn!("[upstream_repair] could not list branches: {e}");
            return;
        }
    };

    let mut repaired = 0usize;
    for id in ids {
        let Ok(Some(branch)) = store.get_branch(&id) else {
            continue;
        };
        if branch.branch_type != store::BranchType::Local
            || branch.pr_number.is_none()
            || dedup_stem(&branch.branch_name).is_none()
        {
            continue;
        }
        match repair_branch(store, &branch, lookup, worktrees) {
            Ok(Some(remote)) => {
                log::info!(
                    "[upstream_repair] branch '{}' (PR #{}) now pushes to origin/{remote}",
                    branch.branch_name,
                    branch.pr_number.unwrap_or_default(),
                );
                repaired += 1;
            }
            Ok(None) => {}
            Err(e) => log::warn!(
                "[upstream_repair] failed to repair '{}': {e}",
                branch.branch_name
            ),
        }
    }
    if repaired > 0 {
        log::info!("[upstream_repair] repaired {repaired} branch(es)");
    }
}

/// Re-point one candidate. Returns the branch it now tracks, or `None` when
/// it was already tracking something, has no worktree, or no trusted source
/// could say what the PR head is.
fn repair_branch<F>(
    store: &Arc<Store>,
    branch: &store::Branch,
    lookup: &mut PrHeadLookup<F>,
    worktrees: &mut impl WorktreeUpstream,
) -> Result<Option<String>, String>
where
    F: FnMut(&str, u64) -> Result<String, GitError>,
{
    let Some(workdir) = store
        .get_workdir_for_branch(&branch.id)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let worktree = Path::new(&workdir.path);
    if !worktree.exists() {
        return Ok(None);
    }

    let current = worktrees.current(worktree, &branch.branch_name, &branch.base_branch);
    let Some(remote) = decide_repair(&branch.branch_name, &current, || {
        let pr_number = branch.pr_number?;
        let project = store.get_project(&branch.project_id).ok().flatten()?;
        let (repo_slug, _) = prs::resolve_branch_repo_and_subpath(store, &project, branch).ok()?;
        lookup.head(&repo_slug, pr_number)
    }) else {
        return Ok(None);
    };

    if worktrees.repoint(worktree, &branch.branch_name, &remote)? {
        Ok(Some(remote))
    } else {
        log::info!(
            "[upstream_repair] origin/{remote} does not exist; leaving '{}' as is",
            branch.branch_name
        );
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TempDir;
    use std::cell::Cell;
    use std::collections::HashMap;

    #[test]
    fn dedup_stem_only_matches_the_unique_branch_name_shape() {
        assert_eq!(dedup_stem("feature-x-2"), Some("feature-x"));
        assert_eq!(dedup_stem("feature-x-10"), Some("feature-x"));
        assert_eq!(dedup_stem("feature-x"), None);
        assert_eq!(dedup_stem("feature-2x"), None);
        assert_eq!(dedup_stem("-2"), None);
        assert_eq!(dedup_stem("feature-"), None);
    }

    #[test]
    fn a_branch_already_tracking_something_is_left_alone_without_asking_gh() {
        let remote = decide_repair("feature-x-2", "feature-x", || {
            panic!("gh should not be consulted when the upstream already differs")
        });
        assert_eq!(remote, None);
    }

    #[test]
    fn a_clobbered_branch_is_repointed_at_the_stem_when_the_pr_agrees() {
        let remote = decide_repair("feature-x-2", "feature-x-2", || {
            Some("feature-x".to_string())
        });
        assert_eq!(remote.as_deref(), Some("feature-x"));
    }

    #[test]
    fn a_ticket_suffixed_branch_whose_pr_head_is_its_own_name_is_untouched() {
        let remote = decide_repair("fix-login-1234", "fix-login-1234", || {
            Some("fix-login-1234".to_string())
        });
        assert_eq!(remote, None);
    }

    /// A PR head that de-duplication could not have produced means the PR
    /// link itself is wrong; re-pointing at it would redirect every push.
    #[test]
    fn rejects_a_pr_head_that_is_neither_the_stem_nor_the_local_name() {
        let remote = decide_repair("feature-x-2", "feature-x-2", || {
            Some("some-other-branch".to_string())
        });
        assert_eq!(remote, None);
        let main = decide_repair("feature-x-2", "feature-x-2", || Some("main".to_string()));
        assert_eq!(main, None);
    }

    #[test]
    fn gives_up_when_gh_cannot_say() {
        assert_eq!(decide_repair("feature-x-2", "feature-x-2", || None), None);
        assert_eq!(
            decide_repair("feature-x-2", "feature-x-2", || Some(String::new())),
            None
        );
        // Not a de-duplicated name at all: no sources consulted.
        assert_eq!(
            decide_repair("feature-x", "feature-x", || panic!("not a candidate")),
            None
        );
    }

    /// Every way `gh` can fail before answering anything trips the breaker.
    #[test]
    fn an_unusable_gh_disables_gh_for_the_rest_of_the_run() {
        let global_failures = [
            github::GH_NOT_AUTHENTICATED.to_string(),
            github::GH_NOT_FOUND.to_string(),
            github::GH_TIMED_OUT.to_string(),
            "Failed to run gh: permission denied".to_string(),
        ];
        for message in global_failures {
            let calls = Cell::new(0);
            let mut lookup = PrHeadLookup::new(|_: &str, _| {
                calls.set(calls.get() + 1);
                Err(GitError::CommandFailed(message.clone()))
            });

            assert_eq!(lookup.head("acme/widgets", 7), None);
            assert_eq!(lookup.head("acme/widgets", 8), None);
            assert_eq!(lookup.head("acme/gadgets", 9), None);
            assert_eq!(
                calls.get(),
                1,
                "gh should be invoked once per launch when it fails with {message:?}"
            );
            assert!(lookup.unavailable);
        }
    }

    /// `gh` answering "no such PR" is about that PR, not about `gh`: the
    /// next branch still gets its lookup.
    #[test]
    fn a_per_branch_gh_failure_skips_only_that_branch() {
        let calls = Cell::new(0);
        let mut lookup = PrHeadLookup::new(|_: &str, pr_number| {
            calls.set(calls.get() + 1);
            if pr_number == 7 {
                Err(GitError::CommandFailed(
                    "GraphQL: Could not resolve to a PullRequest with the number of 7. \
                     (repository.pullRequest)"
                        .to_string(),
                ))
            } else {
                Ok(format!("head-{pr_number}"))
            }
        });

        assert_eq!(lookup.head("acme/widgets", 7), None);
        assert!(!lookup.unavailable);
        assert_eq!(lookup.head("acme/widgets", 8).as_deref(), Some("head-8"));
        assert_eq!(lookup.head("acme/widgets", 7), None);
        assert_eq!(lookup.head("acme/gadgets", 9).as_deref(), Some("head-9"));
        assert_eq!(calls.get(), 4);
    }

    /// In-memory stand-in for the worktrees: `upstreams` maps a local name
    /// to what it tracks (absent = tracks itself), `repoints` records the
    /// `--set-upstream-to` calls.
    #[derive(Default)]
    struct FakeWorktrees {
        upstreams: HashMap<String, String>,
        repoints: Vec<(String, String)>,
    }

    impl WorktreeUpstream for FakeWorktrees {
        fn current(&mut self, _: &Path, local: &str, _: &str) -> String {
            self.upstreams
                .get(local)
                .cloned()
                .unwrap_or_else(|| local.to_string())
        }

        fn repoint(&mut self, _: &Path, local: &str, remote: &str) -> Result<bool, String> {
            self.repoints.push((local.to_string(), remote.to_string()));
            self.upstreams.insert(local.to_string(), remote.to_string());
            Ok(true)
        }
    }

    /// Local PR branches, each linked to its own (empty, existing) worktree
    /// directory. The directories must outlive the walk.
    fn store_with_pr_branches(names: &[(&str, u64)]) -> (Arc<Store>, Vec<TempDir>) {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::new("acme/widgets");
        store.create_project(&project).unwrap();
        let dirs = names
            .iter()
            .map(|(name, pr)| {
                let branch = store::Branch::new(&project.id, name, "main").with_pr(*pr);
                store.create_branch(&branch).unwrap();
                let dir = TempDir::new("worktree");
                let workdir = store::Workdir::new(&project.id, dir.path().to_str().unwrap())
                    .with_branch(&branch.id);
                store.create_workdir(&workdir).unwrap();
                dir
            })
            .collect();
        (store, dirs)
    }

    /// The full walk: a clobbered de-dup branch is re-pointed, a branch that
    /// already tracks the stem is never asked about, a ticket-suffixed
    /// branch and an untrusted head are left as they are.
    #[test]
    fn run_repoints_clobbered_branches_and_leaves_the_rest() {
        let (store, _dirs) = store_with_pr_branches(&[
            ("feature-x-2", 7),
            ("fix-login-1234", 8),
            ("feature-y-2", 9),
            ("feature-z-2", 10),
        ]);
        let asked = std::cell::RefCell::new(Vec::new());
        let mut lookup = PrHeadLookup::new(|_: &str, pr_number| {
            asked.borrow_mut().push(pr_number);
            Ok(match pr_number {
                7 => "feature-x",
                8 => "fix-login-1234",
                9 => "unrelated",
                _ => unreachable!("PR #{pr_number} already tracks its head"),
            }
            .to_string())
        });
        let mut worktrees = FakeWorktrees::default();
        worktrees
            .upstreams
            .insert("feature-z-2".to_string(), "feature-z".to_string());

        run_with(&store, &mut lookup, &mut worktrees);

        let mut asked = asked.into_inner();
        asked.sort_unstable();
        assert_eq!(asked, vec![7, 8, 9]);
        assert_eq!(
            worktrees.repoints,
            vec![("feature-x-2".to_string(), "feature-x".to_string())]
        );

        // Self-limiting: a second walk has nothing left to re-point and only
        // the branches that can never be settled are asked about again.
        let mut lookup = PrHeadLookup::new(|_: &str, pr_number| {
            Ok(match pr_number {
                8 => "fix-login-1234",
                9 => "unrelated",
                _ => unreachable!("PR #{pr_number} should not be asked about again"),
            }
            .to_string())
        });
        run_with(&store, &mut lookup, &mut worktrees);
        assert_eq!(worktrees.repoints.len(), 1);
    }

    /// One candidate's PR was deleted: the walk steps over it and still
    /// repairs the others.
    #[test]
    fn run_keeps_repairing_other_branches_after_a_deleted_pr() {
        let (store, _dirs) =
            store_with_pr_branches(&[("feature-x-2", 7), ("feature-y-2", 8), ("feature-w-3", 9)]);
        let calls = Cell::new(0);
        let mut lookup = PrHeadLookup::new(|_: &str, pr_number| {
            calls.set(calls.get() + 1);
            match pr_number {
                7 => Err(GitError::CommandFailed(
                    "GraphQL: Could not resolve to a PullRequest with the number of 7.".to_string(),
                )),
                8 => Ok("feature-y".to_string()),
                9 => Ok("feature-w".to_string()),
                _ => unreachable!(),
            }
        });
        let mut worktrees = FakeWorktrees::default();

        run_with(&store, &mut lookup, &mut worktrees);

        assert_eq!(
            calls.get(),
            3,
            "every branch after the dead PR is still asked"
        );
        let mut repoints = worktrees.repoints.clone();
        repoints.sort();
        assert_eq!(
            repoints,
            vec![
                ("feature-w-3".to_string(), "feature-w".to_string()),
                ("feature-y-2".to_string(), "feature-y".to_string()),
            ]
        );
    }

    #[test]
    fn run_with_an_unavailable_gh_calls_it_once_and_repoints_nothing() {
        let (store, _dirs) =
            store_with_pr_branches(&[("feature-x-2", 7), ("fix-login-1234", 8), ("rel-3", 9)]);
        let calls = Cell::new(0);
        let mut lookup = PrHeadLookup::new(|_: &str, _| {
            calls.set(calls.get() + 1);
            Err(GitError::CommandFailed(github::GH_NOT_FOUND.to_string()))
        });
        let mut worktrees = FakeWorktrees::default();

        run_with(&store, &mut lookup, &mut worktrees);

        assert_eq!(calls.get(), 1);
        assert!(worktrees.repoints.is_empty());
    }

    /// Branches with no worktree on disk, no PR, or a remote workspace are
    /// not candidates and never reach `gh`.
    #[test]
    fn run_skips_branches_without_a_local_worktree_or_pr() {
        let store = Arc::new(Store::in_memory().unwrap());
        let project = store::Project::new("acme/widgets");
        store.create_project(&project).unwrap();
        // De-dup shape and a PR, but no worktree row at all.
        store
            .create_branch(&store::Branch::new(&project.id, "feature-x-2", "main").with_pr(7))
            .unwrap();
        // A worktree that no longer exists on disk.
        let gone = store::Branch::new(&project.id, "feature-y-2", "main").with_pr(8);
        store.create_branch(&gone).unwrap();
        let missing_dir = TempDir::new("gone");
        let missing_path = missing_dir.path().to_str().unwrap().to_string();
        drop(missing_dir);
        store
            .create_workdir(&store::Workdir::new(&project.id, &missing_path).with_branch(&gone.id))
            .unwrap();
        // No PR.
        store
            .create_branch(&store::Branch::new(&project.id, "feature-z-2", "main"))
            .unwrap();
        // A Blox workspace branch.
        store
            .create_branch(
                &store::Branch::new_remote(&project.id, "feature-w-2", "main", "ws-1").with_pr(9),
            )
            .unwrap();

        let mut lookup = PrHeadLookup::new(|_: &str, pr_number| -> Result<String, GitError> {
            panic!("PR #{pr_number} is not a candidate")
        });
        let mut worktrees = FakeWorktrees::default();
        run_with(&store, &mut lookup, &mut worktrees);
        assert!(worktrees.repoints.is_empty());
    }
}
