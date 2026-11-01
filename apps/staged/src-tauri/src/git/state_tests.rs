use super::cli::EnvSource;
use super::state::{
    compute_branch_git_state_with, compute_local_branch_git_state, BranchGitState,
    BranchGitStateQuery, FetchMode, UpstreamRelation, WorktreeStatusScope,
};
use super::strip_git_env;
use crate::test_utils::TempGitRepo;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use uuid::Uuid;

struct TempPath {
    path: PathBuf,
}

impl TempPath {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("staged-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_git(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-C")
        .arg(repo)
        .args(args);
    strip_git_env(&mut command);

    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn clone_repo(origin: &TempGitRepo) -> TempPath {
    let clone_dir = TempPath::new("clone");
    let mut command = Command::new("git");
    command.arg("clone").arg(origin.path()).arg(&clone_dir.path);
    strip_git_env(&mut command);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    run_git(
        &clone_dir.path,
        &["config", "user.email", "test@example.com"],
    );
    run_git(&clone_dir.path, &["config", "user.name", "Test"]);
    clone_dir
}

fn remote_backed_feature() -> (TempGitRepo, TempPath) {
    let origin = TempGitRepo::new();
    origin.write_file("file.txt", "base\n");
    origin.commit("base");

    let clone = clone_repo(&origin);
    run_git(&clone.path, &["checkout", "-b", "feature"]);
    fs::write(clone.path.join("file.txt"), "base\nfeature\n").unwrap();
    run_git(&clone.path, &["add", "."]);
    run_git(&clone.path, &["commit", "-m", "feature"]);
    run_git(&clone.path, &["push", "origin", "feature:feature"]);
    run_git(&clone.path, &["fetch", "origin", "main", "feature"]);
    (origin, clone)
}

fn state(repo: &Path, fetch_mode: FetchMode) -> BranchGitState {
    compute_local_branch_git_state(
        repo,
        "feature",
        "feature",
        "main",
        fetch_mode,
        WorktreeStatusScope::Full,
        EnvSource::Captured,
    )
}

#[test]
fn detects_in_sync_branch() {
    let (_origin, clone) = remote_backed_feature();
    let state = state(&clone.path, FetchMode::Force);

    assert_eq!(state.upstream.relation, UpstreamRelation::InSync);
    assert_eq!(state.upstream.ahead, 0);
    assert_eq!(state.upstream.behind, 0);
    assert!(state.expected_branch_matches);
    assert!(!state.worktree.dirty);
}

#[test]
fn detects_local_ahead_branch() {
    let (_origin, clone) = remote_backed_feature();
    fs::write(clone.path.join("local.txt"), "local\n").unwrap();
    run_git(&clone.path, &["add", "."]);
    run_git(&clone.path, &["commit", "-m", "local"]);

    let state = state(&clone.path, FetchMode::Force);

    assert_eq!(state.upstream.relation, UpstreamRelation::LocalAhead);
    assert_eq!(state.upstream.ahead, 1);
    assert_eq!(state.upstream.behind, 0);
}

#[test]
fn detects_origin_ahead_branch() {
    let (origin, clone) = remote_backed_feature();
    origin.run_git(&["checkout", "feature"]);
    origin.write_file("origin.txt", "origin\n");
    origin.commit("origin");

    let state = state(&clone.path, FetchMode::Force);

    assert_eq!(state.upstream.relation, UpstreamRelation::OriginAhead);
    assert_eq!(state.upstream.ahead, 0);
    assert_eq!(state.upstream.behind, 1);
}

#[test]
fn detects_diverged_branch() {
    let (origin, clone) = remote_backed_feature();
    fs::write(clone.path.join("local.txt"), "local\n").unwrap();
    run_git(&clone.path, &["add", "."]);
    run_git(&clone.path, &["commit", "-m", "local"]);

    origin.run_git(&["checkout", "feature"]);
    origin.write_file("origin.txt", "origin\n");
    origin.commit("origin");

    let state = state(&clone.path, FetchMode::Force);

    assert_eq!(state.upstream.relation, UpstreamRelation::Diverged);
    assert_eq!(state.upstream.ahead, 1);
    assert_eq!(state.upstream.behind, 1);
    assert!(state.upstream.merge_base_sha.is_some());
}

#[test]
fn detects_missing_upstream_branch() {
    let origin = TempGitRepo::new();
    origin.write_file("file.txt", "base\n");
    origin.commit("base");
    let clone = clone_repo(&origin);
    run_git(&clone.path, &["checkout", "-b", "feature"]);
    fs::write(clone.path.join("file.txt"), "base\nlocal\n").unwrap();
    run_git(&clone.path, &["add", "."]);
    run_git(&clone.path, &["commit", "-m", "feature"]);

    let state = state(&clone.path, FetchMode::Force);

    assert_eq!(state.upstream.relation, UpstreamRelation::Missing);
    assert!(!state.upstream.exists);
}

#[test]
fn detects_dirty_worktree_counts() {
    let (_origin, clone) = remote_backed_feature();
    fs::write(clone.path.join("staged.txt"), "staged\n").unwrap();
    run_git(&clone.path, &["add", "staged.txt"]);
    fs::write(clone.path.join("file.txt"), "base\nfeature\nunstaged\n").unwrap();
    fs::write(clone.path.join("untracked.txt"), "untracked\n").unwrap();

    let state = state(&clone.path, FetchMode::Never);

    assert!(state.worktree.dirty);
    assert_eq!(state.worktree.added, 1);
    assert_eq!(state.worktree.modified, 1);
    assert_eq!(state.worktree.untracked, 1);
}

#[test]
fn detects_conflicted_worktree() {
    let repo = TempGitRepo::new();
    repo.write_file("file.txt", "base\n");
    repo.commit("base");
    repo.run_git(&["update-ref", "refs/remotes/origin/main", "main"]);
    repo.run_git(&["checkout", "-b", "feature"]);
    repo.write_file("file.txt", "feature\n");
    repo.commit("feature");
    repo.run_git(&["checkout", "main"]);
    repo.write_file("file.txt", "main\n");
    repo.commit("main");
    repo.run_git(&["checkout", "feature"]);

    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-C")
        .arg(repo.path())
        .args(["merge", "main"]);
    strip_git_env(&mut command);
    let output = command.output().unwrap();
    assert!(!output.status.success());

    let state = compute_local_branch_git_state(
        repo.path(),
        "feature",
        "feature",
        "main",
        FetchMode::Never,
        WorktreeStatusScope::Full,
        EnvSource::Captured,
    );

    assert!(state.worktree.dirty);
    assert_eq!(state.worktree.conflicted, 1);
}

#[test]
fn detects_detached_head() {
    let (_origin, clone) = remote_backed_feature();
    let head = run_git(&clone.path, &["rev-parse", "HEAD"]);
    run_git(&clone.path, &["checkout", "--detach", head.trim()]);

    let state = state(&clone.path, FetchMode::Never);

    assert!(state.detached_head);
    assert!(state.current_branch.is_none());
    assert!(!state.expected_branch_matches);
}

#[test]
fn detects_wrong_checked_out_branch() {
    let (_origin, clone) = remote_backed_feature();
    run_git(&clone.path, &["checkout", "-b", "other"]);

    let state = state(&clone.path, FetchMode::Never);

    assert_eq!(state.current_branch.as_deref(), Some("other"));
    assert!(!state.detached_head);
    assert!(!state.expected_branch_matches);
}

#[test]
fn detects_base_branch_moved() {
    let (origin, clone) = remote_backed_feature();
    origin.run_git(&["checkout", "main"]);
    origin.write_file("base.txt", "new base\n");
    origin.commit("base moved");

    let state = state(&clone.path, FetchMode::Force);

    assert_eq!(state.base.commits_since_fork, 1);
}

#[test]
fn upstream_behind_base_zero_when_origin_branch_is_caught_up() {
    let (_origin, clone) = remote_backed_feature();
    let state = state(&clone.path, FetchMode::Force);

    // Branch and its origin tip are both forked from main, so origin/main is
    // not ahead of origin/feature.
    assert_eq!(state.upstream.behind_base, 0);
}

#[test]
fn upstream_behind_base_counts_origin_base_commits_missing_from_origin_branch() {
    let (origin, clone) = remote_backed_feature();
    // Land two commits on origin/main *after* origin/feature was pushed, so
    // origin/feature is now two commits behind origin/main.
    origin.run_git(&["checkout", "main"]);
    origin.write_file("base1.txt", "one\n");
    origin.commit("base 1");
    origin.write_file("base2.txt", "two\n");
    origin.commit("base 2");

    let state = state(&clone.path, FetchMode::Force);

    assert_eq!(state.upstream.behind_base, 2);
}

/// A de-duplicated PR import: HEAD is on local `feature-x-2` while the branch
/// pushes to and is compared against `origin/feature-x`. The identity check
/// must use the local name and the upstream the remote one — comparing
/// against the non-existent `origin/feature-x-2` is the bug this guards.
#[test]
fn upstream_uses_the_remote_branch_while_identity_uses_the_local_name() {
    let (_origin, clone) = remote_backed_feature();
    run_git(&clone.path, &["push", "origin", "feature:feature-x"]);
    run_git(&clone.path, &["fetch", "origin", "feature-x"]);
    run_git(
        &clone.path,
        &["checkout", "-b", "feature-x-2", "origin/feature-x"],
    );
    fs::write(clone.path.join("file.txt"), "base\nfeature\nmore\n").unwrap();
    run_git(&clone.path, &["add", "."]);
    run_git(&clone.path, &["commit", "-m", "more"]);

    let state = compute_local_branch_git_state(
        &clone.path,
        "feature-x-2",
        "feature-x",
        "main",
        FetchMode::Force,
        WorktreeStatusScope::Full,
        EnvSource::Captured,
    );

    assert!(state.expected_branch_matches);
    assert_eq!(state.current_branch.as_deref(), Some("feature-x-2"));
    assert_eq!(state.upstream.r#ref, "origin/feature-x");
    assert!(state.upstream.exists);
    assert_eq!(state.upstream.relation, UpstreamRelation::LocalAhead);
    assert_eq!(state.upstream.ahead, 1);
    assert_eq!(state.upstream.behind, 0);
}

/// Run a build against real git in `repo`, recording every invocation, so a
/// test can assert on what it spawns. The companion keeps the fork point it
/// was handed.
fn recorded_state(
    repo: &Path,
    branch: &str,
    remote_branch: &str,
    fetch_mode: FetchMode,
) -> (BranchGitState, Option<String>, Vec<String>) {
    let calls = Mutex::new(Vec::new());
    let (state, fork_point) = compute_branch_git_state_with(
        &format!("local:{}:{branch}:main", repo.display()),
        |args| {
            calls.lock().unwrap().push(args.join(" "));
            super::cli::run(repo, args).map_err(|e| e.to_string())
        },
        BranchGitStateQuery {
            branch_name: branch,
            remote_branch,
            base_branch: "main",
            fetch_mode,
            worktree_scope: WorktreeStatusScope::Full,
        },
        |merge_base| merge_base.map(str::to_string),
    );
    (state, fork_point, calls.into_inner().unwrap())
}

fn subcommands(calls: &[String]) -> Vec<&str> {
    let mut names: Vec<&str> = calls
        .iter()
        .map(|call| call.split(' ').next().unwrap())
        .collect();
    names.sort_unstable();
    names
}

fn position(calls: &[String], prefix: &str) -> usize {
    calls
        .iter()
        .position(|call| call.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix} in {calls:?}"))
}

/// Fetch-free, in sync with its upstream: seven spawns in two phases, no
/// `fetch`, one `merge-base` (HEAD against origin/main, handed to the
/// companion), the upstream's merge base derived from the counts.
#[test]
fn fetch_free_build_spawns_seven_commands_with_one_merge_base() {
    let (_origin, clone) = remote_backed_feature();
    let head = run_git(&clone.path, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    let fork = run_git(&clone.path, &["merge-base", "HEAD", "origin/main"])
        .trim()
        .to_string();

    let (state, fork_point, calls) =
        recorded_state(&clone.path, "feature", "feature", FetchMode::Never);

    assert_eq!(state.upstream.relation, UpstreamRelation::InSync);
    assert_eq!(
        state.upstream.merge_base_sha.as_deref(),
        Some(head.as_str())
    );
    assert_eq!(state.base.sha.as_deref(), Some(fork.as_str()));
    assert_eq!(state.base.commits_since_fork, 0);
    assert_eq!(state.head_sha.as_deref(), Some(head.as_str()));
    assert_eq!(state.current_branch.as_deref(), Some("feature"));
    assert_eq!(fork_point.as_deref(), Some(fork.as_str()));

    assert_eq!(
        subcommands(&calls),
        [
            "for-each-ref",
            "merge-base",
            "rev-list",
            "rev-list",
            "rev-list",
            "rev-parse",
            "status"
        ],
        "{calls:?}"
    );
    assert!(calls.contains(&"rev-parse HEAD --symbolic-full-name HEAD".to_string()));
    assert!(calls.contains(
        &"for-each-ref --format=%(refname) %(objectname) refs/remotes/origin/main refs/remotes/origin/feature"
            .to_string()
    ));
    assert!(calls.contains(&"merge-base HEAD origin/main".to_string()));
    assert!(calls.contains(&format!("rev-list --count {fork}..origin/main")));
    assert!(calls.contains(&"rev-list --left-right --count HEAD...origin/feature".to_string()));
    assert!(calls.contains(&"rev-list --count origin/feature..origin/main".to_string()));
}

/// Diverged is the one relation whose upstream merge base needs git.
#[test]
fn diverged_build_asks_git_for_the_upstream_merge_base() {
    let (origin, clone) = remote_backed_feature();
    let fork = run_git(&clone.path, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    fs::write(clone.path.join("local.txt"), "local\n").unwrap();
    run_git(&clone.path, &["add", "."]);
    run_git(&clone.path, &["commit", "-m", "local"]);
    origin.run_git(&["checkout", "feature"]);
    origin.write_file("origin.txt", "origin\n");
    origin.commit("origin");
    run_git(&clone.path, &["fetch", "origin", "feature"]);

    let (state, _, calls) = recorded_state(&clone.path, "feature", "feature", FetchMode::Never);

    assert_eq!(state.upstream.relation, UpstreamRelation::Diverged);
    assert_eq!(
        state.upstream.merge_base_sha.as_deref(),
        Some(fork.as_str())
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("merge-base"))
            .count(),
        2
    );
    assert!(calls.contains(&"merge-base HEAD origin/feature".to_string()));
}

/// A failed upstream count leaves nothing to derive the upstream merge base
/// from, so the build spends one more spawn asking git for it; the counts
/// themselves read as zero, as they always have.
#[test]
fn failed_upstream_count_asks_git_for_the_upstream_merge_base() {
    let (_origin, clone) = remote_backed_feature();
    let upstream_tip = run_git(&clone.path, &["rev-parse", "origin/feature"])
        .trim()
        .to_string();
    fs::write(clone.path.join("local.txt"), "local\n").unwrap();
    run_git(&clone.path, &["add", "."]);
    run_git(&clone.path, &["commit", "-m", "local"]);

    let calls = Mutex::new(Vec::new());
    let (state, fork_point) = compute_branch_git_state_with(
        &format!("local:{}:feature:main:failed-count", clone.path.display()),
        |args| {
            calls.lock().unwrap().push(args.join(" "));
            if args.starts_with(&["rev-list", "--left-right"]) {
                return Err("fatal: bad revision 'HEAD...origin/feature'".to_string());
            }
            super::cli::run(&clone.path, args).map_err(|e| e.to_string())
        },
        BranchGitStateQuery {
            branch_name: "feature",
            remote_branch: "feature",
            base_branch: "main",
            fetch_mode: FetchMode::Never,
            worktree_scope: WorktreeStatusScope::Full,
        },
        |merge_base| merge_base.map(str::to_string),
    );
    let calls = calls.into_inner().unwrap();

    assert!(state.upstream.exists);
    assert_eq!(
        state.upstream.merge_base_sha.as_deref(),
        Some(upstream_tip.as_str())
    );
    assert_eq!((state.upstream.ahead, state.upstream.behind), (0, 0));
    assert!(fork_point.is_some());
    assert!(
        calls.contains(&"merge-base HEAD origin/feature".to_string()),
        "{calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("merge-base"))
            .count(),
        2,
        "{calls:?}"
    );
}

/// Ahead of and behind the upstream derive the merge base from the counts
/// too: the upstream tip and HEAD respectively.
#[test]
fn ahead_and_behind_builds_derive_the_upstream_merge_base() {
    let (origin, clone) = remote_backed_feature();
    let upstream_tip = run_git(&clone.path, &["rev-parse", "origin/feature"])
        .trim()
        .to_string();
    fs::write(clone.path.join("local.txt"), "local\n").unwrap();
    run_git(&clone.path, &["add", "."]);
    run_git(&clone.path, &["commit", "-m", "local"]);
    let (state, _, calls) = recorded_state(&clone.path, "feature", "feature", FetchMode::Never);
    assert_eq!(state.upstream.relation, UpstreamRelation::LocalAhead);
    assert_eq!(
        state.upstream.merge_base_sha.as_deref(),
        Some(upstream_tip.as_str())
    );
    assert_eq!(subcommands(&calls).len(), 7);

    run_git(&clone.path, &["reset", "--hard", &upstream_tip]);
    origin.run_git(&["checkout", "feature"]);
    origin.write_file("origin.txt", "origin\n");
    origin.commit("origin");
    run_git(&clone.path, &["fetch", "origin", "feature"]);
    let (state, _, calls) = recorded_state(&clone.path, "feature", "feature", FetchMode::Never);
    assert_eq!(state.upstream.relation, UpstreamRelation::OriginAhead);
    assert_eq!(
        state.upstream.merge_base_sha.as_deref(),
        Some(upstream_tip.as_str())
    );
    assert_eq!(subcommands(&calls).len(), 7);
}

/// No upstream on origin: `for-each-ref` lists nothing for its pattern, the
/// three upstream counts never run, and the build is five spawns.
#[test]
fn build_without_an_upstream_spawns_five_commands() {
    let origin = TempGitRepo::new();
    origin.write_file("file.txt", "base\n");
    let base = origin.commit("base");
    let clone = clone_repo(&origin);
    run_git(&clone.path, &["checkout", "-b", "feature"]);
    fs::write(clone.path.join("file.txt"), "base\nlocal\n").unwrap();
    run_git(&clone.path, &["add", "."]);
    run_git(&clone.path, &["commit", "-m", "feature"]);

    let (state, fork_point, calls) =
        recorded_state(&clone.path, "feature", "feature", FetchMode::Never);

    assert!(!state.upstream.exists);
    assert_eq!(state.upstream.relation, UpstreamRelation::Missing);
    assert_eq!(state.upstream.r#ref, "origin/feature");
    assert!(state.upstream.merge_base_sha.is_none());
    assert_eq!(state.base.sha.as_deref(), Some(base.as_str()));
    assert_eq!(fork_point.as_deref(), Some(base.as_str()));
    assert_eq!(
        subcommands(&calls),
        [
            "for-each-ref",
            "merge-base",
            "rev-list",
            "rev-parse",
            "status"
        ],
        "{calls:?}"
    );
}

/// No `origin/main` at all: nothing resolves, nothing is counted, the
/// companion gets no fork point, and only the four phase-one spawns run (the
/// `merge-base` among them failing fast).
#[test]
fn build_without_the_base_ref_has_no_fork_point() {
    let repo = TempGitRepo::new();
    repo.write_file("file.txt", "base\n");
    repo.commit("base");
    repo.run_git(&["checkout", "-b", "feature"]);

    let (state, fork_point, calls) =
        recorded_state(repo.path(), "feature", "feature", FetchMode::Never);

    assert!(state.base.sha.is_none());
    assert_eq!(state.base.commits_since_fork, 0);
    assert!(!state.upstream.exists);
    assert!(fork_point.is_none());
    assert!(state.head_sha.is_some());
    assert_eq!(
        subcommands(&calls),
        ["for-each-ref", "merge-base", "rev-parse", "status"],
        "{calls:?}"
    );
}

/// A branch with nothing on it yet forks at HEAD itself.
#[test]
fn build_with_no_commits_ahead_forks_at_head() {
    let origin = TempGitRepo::new();
    origin.write_file("file.txt", "base\n");
    let base = origin.commit("base");
    let clone = clone_repo(&origin);
    run_git(&clone.path, &["checkout", "-b", "feature"]);

    let (state, fork_point, _) =
        recorded_state(&clone.path, "feature", "feature", FetchMode::Never);

    assert_eq!(state.head_sha.as_deref(), Some(base.as_str()));
    assert_eq!(fork_point.as_deref(), Some(base.as_str()));
    assert_eq!(state.base.commits_since_fork, 0);
    assert!(state.expected_branch_matches);
}

/// Detached HEAD comes out of the same `rev-parse` spawn; the refs and the
/// fork point resolve as usual.
#[test]
fn detached_build_resolves_head_and_refs_in_one_spawn() {
    let (_origin, clone) = remote_backed_feature();
    let head = run_git(&clone.path, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    run_git(&clone.path, &["checkout", "--detach", &head]);

    let (state, fork_point, calls) =
        recorded_state(&clone.path, "feature", "feature", FetchMode::Never);

    assert!(state.detached_head);
    assert!(state.current_branch.is_none());
    assert!(!state.expected_branch_matches);
    assert_eq!(state.head_sha.as_deref(), Some(head.as_str()));
    assert!(state.upstream.exists);
    assert!(fork_point.is_some());
    assert!(!calls.iter().any(|call| call.starts_with("branch ")));
}

/// An unborn branch is the one case `rev-parse HEAD` cannot answer, so the
/// branch name costs a second spawn — and nothing else is counted.
#[test]
fn unborn_branch_still_reports_its_name() {
    let repo = TempGitRepo::new();
    let (state, fork_point, calls) = recorded_state(repo.path(), "main", "main", FetchMode::Never);
    assert!(state.head_sha.is_none());
    assert_eq!(state.current_branch.as_deref(), Some("main"));
    assert!(state.expected_branch_matches);
    assert!(!state.detached_head);
    assert!(fork_point.is_none());
    assert!(calls.contains(&"branch --show-current".to_string()));
}

/// With a fetch the remote refs are read only after it, and a fetch that
/// learns the upstream is not on origin keeps the build from looking it up.
#[test]
fn fetching_build_reads_remote_refs_after_the_fetch() {
    let (origin, clone) = remote_backed_feature();
    origin.run_git(&["checkout", "main"]);
    origin.write_file("base.txt", "new base\n");
    origin.commit("base moved");

    let (state, _, calls) = recorded_state(&clone.path, "feature", "feature", FetchMode::Force);

    assert_eq!(state.base.commits_since_fork, 1);
    let fetch = position(&calls, "fetch ");
    assert!(position(&calls, "for-each-ref") > fetch, "{calls:?}");
    assert!(position(&calls, "merge-base") > fetch, "{calls:?}");

    // The upstream is missing on origin: the fetch says so, and
    // `for-each-ref` is asked for the base alone.
    run_git(&clone.path, &["checkout", "-b", "unpublished"]);
    let (state, _, calls) =
        recorded_state(&clone.path, "unpublished", "unpublished", FetchMode::Force);
    assert!(!state.upstream.exists);
    assert!(calls.contains(
        &"for-each-ref --format=%(refname) %(objectname) refs/remotes/origin/main".to_string()
    ));
    // Only the fetch's own refspec names the missing branch.
    assert!(!calls.iter().any(|call| {
        !call.starts_with("fetch ") && call.contains("refs/remotes/origin/unpublished")
    }));
}
