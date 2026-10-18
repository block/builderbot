//! Which branch on `origin` a local branch pushes to.
//!
//! Git's own upstream tracking (`branch.<name>.remote` / `branch.<name>.merge`)
//! is the single source of truth for this; Staged records it nowhere else. A
//! PR import whose local name had to be de-duplicated (`feature-x-2` for PR
//! head `feature-x`) is made to track `origin/feature-x` when its worktree is
//! created, `git push -u origin <local>:<remote>` keeps that tracking entry
//! current, `git branch -m` carries it across renames, and every push, pull,
//! rebase, comparison and PR lookup reads the remote name back from here.
//!
//! The two config entries are read directly rather than through
//! `<name>@{upstream}`: git only resolves that notation when some
//! `remote.origin.fetch` refspec maps the upstream branch, which a
//! `--single-branch` clone has for one branch only. The config entries are
//! what `git push -u` writes and what `git push`/`git pull` act on, with or
//! without such a mapping.

use std::path::Path;

use super::cli;
use super::refs::branch_name_without_origin;

const HEADS_PREFIX: &str = "refs/heads/";

/// The branch on `origin` that `local` pushes to and is compared against.
///
/// Reads `branch.<local>.remote` and `branch.<local>.merge` in `worktree`.
/// Falls back to `local` itself when no upstream is configured, when the
/// upstream is on another remote, or when it is the branch's own base: git
/// makes a branch created from `origin/<base>` track `<base>`, and a Staged
/// branch never pushes onto its base, so that entry means "nothing to say"
/// rather than "push here".
pub fn upstream_branch_name(worktree: &Path, local: &str, base_branch: &str) -> String {
    // `git config --get` only reads `.git/config`: the lite env is enough.
    upstream_branch_name_with(
        |args| cli::run_lite(worktree, args).map_err(|e| e.to_string()),
        local,
        base_branch,
    )
}

/// [`upstream_branch_name`] over an arbitrary git runner — a Blox workspace
/// exec, or a fake in tests. Runs `git config --get` once for the remote and,
/// only when that is `origin`, once more for the merge ref.
pub fn upstream_branch_name_with<F>(mut run_git: F, local: &str, base_branch: &str) -> String
where
    F: FnMut(&[&str]) -> Result<String, String>,
{
    let remote_key = format!("branch.{local}.remote");
    let remote = run_git(&["config", "--get", &remote_key]).ok();
    let merge = match remote.as_deref().map(str::trim) {
        Some("origin") => {
            let merge_key = format!("branch.{local}.merge");
            run_git(&["config", "--get", &merge_key]).ok()
        }
        _ => None,
    };
    upstream_branch_name_from_config(remote.as_deref(), merge.as_deref(), local, base_branch)
}

/// The decision behind [`upstream_branch_name`]: `remote` and `merge` are the
/// values of `branch.<local>.remote` and `branch.<local>.merge`, `None` when
/// unset.
pub(crate) fn upstream_branch_name_from_config(
    remote: Option<&str>,
    merge: Option<&str>,
    local: &str,
    base_branch: &str,
) -> String {
    if remote.map(str::trim) != Some("origin") {
        return local.to_string();
    }
    let Some(remote_branch) = merge
        .map(str::trim)
        .and_then(|full| full.strip_prefix(HEADS_PREFIX))
        .filter(|name| !name.is_empty())
    else {
        return local.to_string();
    };
    if remote_branch == branch_name_without_origin(base_branch) {
        return local.to_string();
    }
    remote_branch.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::worktree::{set_upstream_to_origin, set_upstream_to_origin_or_unset};
    use crate::test_utils::{run_git_in, TempDir, TempGitRepo};

    #[test]
    fn no_upstream_means_the_local_name() {
        assert_eq!(
            upstream_branch_name_from_config(None, None, "feature-x-2", "main"),
            "feature-x-2"
        );
        assert_eq!(
            upstream_branch_name_from_config(Some(""), Some(""), "feature-x-2", "main"),
            "feature-x-2"
        );
        // Half an entry is no entry.
        assert_eq!(
            upstream_branch_name_from_config(Some("origin"), None, "feature-x-2", "main"),
            "feature-x-2"
        );
        assert_eq!(
            upstream_branch_name_from_config(
                None,
                Some("refs/heads/feature-x"),
                "feature-x-2",
                "main"
            ),
            "feature-x-2"
        );
    }

    #[test]
    fn an_upstream_on_origin_is_the_remote_name() {
        assert_eq!(
            upstream_branch_name_from_config(
                Some("origin\n"),
                Some("refs/heads/feature-x\n"),
                "feature-x-2",
                "main"
            ),
            "feature-x"
        );
        // Slashes in the branch name survive the prefix strip.
        assert_eq!(
            upstream_branch_name_from_config(
                Some("origin"),
                Some("refs/heads/team/feature"),
                "team/feature",
                "main"
            ),
            "team/feature"
        );
    }

    #[test]
    fn an_upstream_on_another_remote_is_ignored() {
        assert_eq!(
            upstream_branch_name_from_config(
                Some("fork"),
                Some("refs/heads/feature-x"),
                "feature-x-2",
                "main"
            ),
            "feature-x-2"
        );
        // A local branch as upstream (`branch.<x>.remote = .`) is not
        // something Staged pushes to either.
        assert_eq!(
            upstream_branch_name_from_config(
                Some("."),
                Some("refs/heads/feature-x"),
                "feature-x-2",
                "main"
            ),
            "feature-x-2"
        );
        // Nor is a merge ref outside refs/heads/.
        assert_eq!(
            upstream_branch_name_from_config(
                Some("origin"),
                Some("refs/pull/7/head"),
                "feature-x-2",
                "main"
            ),
            "feature-x-2"
        );
    }

    /// `git worktree add -b feature origin/main` leaves `feature` tracking
    /// `origin/main`; pushing `feature:main` would be the worst possible
    /// reading of that, so the base is never a push target.
    #[test]
    fn tracking_the_base_branch_means_the_local_name() {
        assert_eq!(
            upstream_branch_name_from_config(
                Some("origin"),
                Some("refs/heads/main"),
                "feature",
                "main"
            ),
            "feature"
        );
        assert_eq!(
            upstream_branch_name_from_config(
                Some("origin"),
                Some("refs/heads/main"),
                "feature",
                "origin/main"
            ),
            "feature"
        );
        // Unless the branch *is* the base, where both readings agree.
        assert_eq!(
            upstream_branch_name_from_config(
                Some("origin"),
                Some("refs/heads/main"),
                "main",
                "main"
            ),
            "main"
        );
    }

    /// The merge ref is only read when the remote is origin: one exec for
    /// a Blox workspace branch that tracks nothing or another remote.
    #[test]
    fn skips_the_merge_lookup_unless_the_remote_is_origin() {
        let mut calls = Vec::new();
        let name = upstream_branch_name_with(
            |args| {
                calls.push(args.join(" "));
                Err("exit 1".to_string())
            },
            "feature-x-2",
            "main",
        );
        assert_eq!(name, "feature-x-2");
        assert_eq!(calls, vec!["config --get branch.feature-x-2.remote"]);

        let mut calls = Vec::new();
        let name = upstream_branch_name_with(
            |args| {
                calls.push(args.join(" "));
                Ok(if args[2].ends_with(".remote") {
                    "origin\n".to_string()
                } else {
                    "refs/heads/feature-x\n".to_string()
                })
            },
            "feature-x-2",
            "main",
        );
        assert_eq!(name, "feature-x");
        assert_eq!(
            calls,
            vec![
                "config --get branch.feature-x-2.remote",
                "config --get branch.feature-x-2.merge"
            ]
        );
    }

    /// A clone of a bare origin that has `main` and `feature-x`, with a local
    /// `feature-x-2` created from the PR head commit the way
    /// `create_and_link_worktree` does for a de-duplicated import.
    fn clone_with_detached_feature_x_2() -> (TempDir, TempDir) {
        let seed = TempGitRepo::new();
        seed.write_file("file.txt", "base\n");
        seed.commit("base");
        seed.run_git(&["checkout", "-b", "feature-x"]);
        seed.write_file("file.txt", "base\nfeature\n");
        seed.commit("feature");
        seed.run_git(&["checkout", "main"]);

        let origin = TempDir::new("origin");
        run_git_in(
            seed.path(),
            &["clone", "--bare", ".", origin.path().to_str().unwrap()],
        );
        let clone = TempDir::new("clone");
        run_git_in(
            origin.path(),
            &[
                "clone",
                origin.path().to_str().unwrap(),
                clone.path().to_str().unwrap(),
            ],
        );
        run_git_in(
            clone.path(),
            &["branch", "--no-track", "feature-x-2", "origin/feature-x"],
        );
        (origin, clone)
    }

    #[test]
    fn reads_the_upstream_git_has_configured() {
        let (_origin, clone) = clone_with_detached_feature_x_2();
        let repo = clone.path();

        // Nothing configured yet.
        assert_eq!(
            upstream_branch_name(repo, "feature-x-2", "main"),
            "feature-x-2"
        );

        assert!(set_upstream_to_origin(repo, "feature-x-2", "feature-x").unwrap());
        assert_eq!(
            run_git_in(repo, &["config", "branch.feature-x-2.merge"]),
            "refs/heads/feature-x"
        );
        assert_eq!(
            upstream_branch_name(repo, "feature-x-2", "main"),
            "feature-x"
        );

        // An upstream on a second remote is not a push target on origin.
        run_git_in(
            repo,
            &["remote", "add", "fork", _origin.path().to_str().unwrap()],
        );
        run_git_in(repo, &["fetch", "fork"]);
        run_git_in(repo, &["branch", "-u", "fork/feature-x", "feature-x-2"]);
        assert_eq!(
            upstream_branch_name(repo, "feature-x-2", "main"),
            "feature-x-2"
        );

        // And the base is never one.
        run_git_in(repo, &["branch", "-u", "origin/main", "feature-x-2"]);
        assert_eq!(
            upstream_branch_name(repo, "feature-x-2", "main"),
            "feature-x-2"
        );
    }

    /// What `git push -u origin feature-x-2:feature-x` leaves in a
    /// `--single-branch` clone: the two config entries, no fetch refspec for
    /// `feature-x`, and a `<local>@{upstream}` git itself refuses to resolve.
    /// The resolver reads it anyway.
    #[test]
    fn reads_an_upstream_no_fetch_refspec_maps() {
        let (_origin, full_clone) = clone_with_detached_feature_x_2();
        let clone = TempDir::new("single-branch");
        run_git_in(
            full_clone.path(),
            &[
                "clone",
                "--single-branch",
                "-b",
                "main",
                _origin.path().to_str().unwrap(),
                clone.path().to_str().unwrap(),
            ],
        );
        let repo = clone.path();
        run_git_in(
            repo,
            &[
                "fetch",
                "origin",
                "+refs/heads/feature-x:refs/remotes/origin/feature-x",
            ],
        );
        run_git_in(
            repo,
            &["branch", "--no-track", "feature-x-2", "origin/feature-x"],
        );
        run_git_in(repo, &["config", "branch.feature-x-2.remote", "origin"]);
        run_git_in(
            repo,
            &["config", "branch.feature-x-2.merge", "refs/heads/feature-x"],
        );
        assert!(cli::run(
            repo,
            &[
                "rev-parse",
                "--symbolic-full-name",
                "feature-x-2@{upstream}"
            ]
        )
        .is_err());

        assert_eq!(
            upstream_branch_name(repo, "feature-x-2", "main"),
            "feature-x"
        );
    }

    #[test]
    fn set_upstream_or_unset_clears_an_inherited_base_upstream() {
        let (_origin, clone) = clone_with_detached_feature_x_2();
        let repo = clone.path();
        // What `git worktree add -b` / `checkout -b` leave behind when the
        // start point is `origin/main`.
        run_git_in(repo, &["branch", "--track", "feature-new", "origin/main"]);
        assert_eq!(
            run_git_in(repo, &["config", "branch.feature-new.merge"]),
            "refs/heads/main"
        );

        // origin/feature-new does not exist: the inherited upstream goes.
        assert!(!set_upstream_to_origin_or_unset(repo, "feature-new", "feature-new").unwrap());
        assert!(run_git_in(
            repo,
            &["config", "--default", "", "branch.feature-new.merge"]
        )
        .is_empty());
        assert_eq!(
            upstream_branch_name(repo, "feature-new", "main"),
            "feature-new"
        );
        // Idempotent when there is nothing to unset.
        assert!(!set_upstream_to_origin_or_unset(repo, "feature-new", "feature-new").unwrap());

        // origin/feature-x does exist: tracked.
        assert!(set_upstream_to_origin_or_unset(repo, "feature-x-2", "feature-x").unwrap());
        assert_eq!(
            upstream_branch_name(repo, "feature-x-2", "main"),
            "feature-x"
        );
    }
}
