use super::state::{
    compute_local_branch_git_state, BranchGitState, FetchMode, UpstreamRelation,
    WorktreeStatusScope,
};
use super::strip_git_env;
use crate::test_utils::TempGitRepo;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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
        "main",
        fetch_mode,
        WorktreeStatusScope::Full,
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
        "main",
        FetchMode::Never,
        WorktreeStatusScope::Full,
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
