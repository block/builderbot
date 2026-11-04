//! The transfer scripts end to end, against real repositories: a bare origin,
//! a source clone, and a destination clone, with a [`LocalEndpoint`] on its
//! own scratch directory standing in for the workspace. Every scenario runs
//! under `/bin/sh` and, where it exists, `/bin/dash` (Blox's `/bin/sh`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use super::*;
use crate::git::strip_git_env;
use crate::test_utils::TempDir;

fn shells() -> Vec<&'static str> {
    let mut shells = vec!["/bin/sh"];
    if Path::new("/bin/dash").exists() {
        shells.push("/bin/dash");
    }
    shells
}

/// Run git without hooks; `Err` carries stderr. Unlike `run_git_in`, stdout
/// is not trimmed: porcelain output starts with significant spaces.
pub(super) fn try_git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(["-c", "core.hooksPath=/dev/null", "-C"])
        .arg(repo)
        .args(args);
    strip_git_env(&mut command);
    let output = command.output().unwrap();
    if output.status.success() {
        Ok(String::from_utf8(output.stdout).unwrap())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

pub(super) fn git(repo: &Path, args: &[&str]) -> String {
    try_git(repo, args).unwrap_or_else(|e| panic!("git {args:?} in {}: {e}", repo.display()))
}

pub(super) fn rev(repo: &Path, rev: &str) -> String {
    git(repo, &["rev-parse", rev]).trim().to_string()
}

pub(super) fn write(repo: &Path, path: &str, contents: impl AsRef<[u8]>) {
    let path = repo.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

pub(super) fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", message]);
    rev(repo, "HEAD")
}

pub(super) fn configure_identity(repo: &Path) {
    for (key, value) in [
        ("user.name", "Test"),
        ("user.email", "test@example.com"),
        ("commit.gpgsign", "false"),
    ] {
        git(repo, &["config", key, value]);
    }
}

/// Bytes that differ throughout, so nothing compresses or deltas them away.
pub(super) fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

const TID: &str = "0b5e7d3c-1f2a-4c6b-9e8d-7a6b5c4d3e2f";

/// A bare origin, a source clone, a destination clone, and the two scratch
/// directories: this machine's, and the stand-in workspace's.
struct World {
    _root: TempDir,
    root: PathBuf,
    origin: PathBuf,
    source: PathBuf,
    dest: PathBuf,
    local_scratch: PathBuf,
    remote_scratch: PathBuf,
    shell: &'static str,
}

impl World {
    fn new(shell: &'static str) -> Self {
        let root_dir = TempDir::new("transfer");
        let root = root_dir.path().to_path_buf();
        let origin = root.join("origin.git");
        git(
            &root,
            &[
                "init",
                "-q",
                "--bare",
                "--initial-branch=main",
                "origin.git",
            ],
        );

        let seed = root.join("seed");
        git(&root, &["clone", "-q", "origin.git", "seed"]);
        configure_identity(&seed);
        write(&seed, ".gitignore", "*.log\n");
        write(&seed, "a.txt", "alpha\n");
        write(&seed, "rename-me.txt", "rename me\n");
        write(&seed, "delete-staged.txt", "staged delete\n");
        write(&seed, "delete-unstaged.txt", "unstaged delete\n");
        write(&seed, "mode.sh", "#!/bin/sh\necho hi\n");
        write(&seed, "partial.txt", "one\ntwo\nthree\nfour\nfive\n");
        write(&seed, "bin.dat", noise(4096, 7));
        commit(&seed, "seed");
        git(&seed, &["push", "-q", "origin", "main"]);

        let source = root.join("source");
        git(&root, &["clone", "-q", "origin.git", "source"]);
        configure_identity(&source);
        let dest = root.join("dest");
        git(&root, &["clone", "-q", "origin.git", "dest"]);

        Self {
            local_scratch: root.join("transfers"),
            remote_scratch: root.join("workspace-home/.staged-transfer"),
            _root: root_dir,
            root,
            origin,
            source,
            dest,
            shell,
        }
    }

    /// A second clone of origin, for pushing from "someone else".
    fn other_clone(&self, name: &str) -> PathBuf {
        git(&self.root, &["clone", "-q", "origin.git", name]);
        let clone = self.root.join(name);
        configure_identity(&clone);
        clone
    }

    fn endpoint(&self, scratch: &Path) -> LocalEndpoint {
        LocalEndpoint::new(&self.root, scratch).with_shell(self.shell)
    }

    fn plan(
        &self,
        source: &LocalEndpoint,
        landing: Landing,
        branches: (&str, &str),
        include_uncommitted: bool,
        exclude_tips: Vec<String>,
    ) -> CarryPlan {
        let (source_branch, dest_branch) = branches;
        let local_bundle = match landing {
            // Local source: the snapshot writes straight into this machine's
            // transfers directory.
            Landing::Checkout => source.scratch_path(TID, "repo.bundle").unwrap(),
            Landing::Branch => self
                .endpoint(&self.local_scratch)
                .scratch_path(TID, "repo.bundle")
                .unwrap(),
        };
        CarryPlan {
            transfer_id: TID.to_string(),
            source_dir: self.source.to_str().unwrap().to_string(),
            source_branch: source_branch.to_string(),
            dest_dir: self.dest.to_str().unwrap().to_string(),
            dest_branch: dest_branch.to_string(),
            landing,
            include_uncommitted,
            exclude_tips,
            bundle_name: "repo.bundle".to_string(),
            local_bundle: PathBuf::from(local_bundle),
            repo_bytes: 0,
            untracked_bytes: 0,
        }
    }

    /// The destination's origin tips for `base` and `upstream`, the way the
    /// orchestrator asks for them before snapshotting.
    fn dest_tips(&self, dest: &LocalEndpoint, names: &[&str]) -> Vec<String> {
        origin_tips(dest, self.dest.to_str().unwrap(), names)
            .unwrap()
            .tips
    }

    /// Local worktree → workspace: snapshot here, land with `checkout -B` in
    /// the destination clone.
    fn to_remote(
        &self,
        branch: &str,
        upstream: &str,
        include_uncommitted: bool,
    ) -> Result<(CarryOutcome, PathBuf), TransferError> {
        let source = self.endpoint(&self.local_scratch);
        let dest = self.endpoint(&self.remote_scratch);
        let tips = self.dest_tips(&dest, &["main", upstream]);
        let plan = self.plan(
            &source,
            Landing::Checkout,
            (branch, branch),
            include_uncommitted,
            tips,
        );
        carry_repo(&source, &dest, &plan).map(|outcome| (outcome, self.dest.clone()))
    }

    /// Workspace → local worktree: snapshot "remotely", create the branch in
    /// the destination clone, add a worktree for it, and finish there.
    fn to_local(
        &self,
        branches: (&str, &str),
        upstream: &str,
        include_uncommitted: bool,
    ) -> Result<(CarryOutcome, PathBuf), TransferError> {
        let source = self.endpoint(&self.remote_scratch);
        let dest = self.endpoint(&self.local_scratch);
        let tips = self.dest_tips(&dest, &["main", upstream]);
        let plan = self.plan(
            &source,
            Landing::Branch,
            branches,
            include_uncommitted,
            tips,
        );
        let outcome = carry_repo(&source, &dest, &plan)?;
        let worktree = self.root.join(format!("wt-{}", branches.1));
        git(
            &self.dest,
            &[
                "worktree",
                "add",
                "-q",
                worktree.to_str().unwrap(),
                branches.1,
            ],
        );
        finish(&dest, &plan, &outcome, worktree.to_str().unwrap())?;
        Ok((outcome, worktree))
    }
}

/// Everything a user could see of a checkout.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Observed {
    head: String,
    /// What the branch tracks and how far ahead/behind, from `status --branch`
    /// with the local branch name removed.
    tracking: String,
    status: String,
    staged: String,
    unstaged: String,
    /// Every tracked or untracked (not ignored) path: its kind, mode and a
    /// hash of its content.
    files: BTreeMap<String, String>,
    branch_config: Vec<(String, String)>,
}

pub(super) fn observe(repo: &Path) -> Observed {
    let status = git(
        repo,
        &[
            "status",
            "--porcelain=v1",
            "--branch",
            "--untracked-files=all",
        ],
    );
    let (header, status) = status.split_once('\n').unwrap_or((&status, ""));
    let tracking = header
        .split_once("...")
        .map(|(_, rest)| rest.to_string())
        .unwrap_or_default();
    // Detached and unborn HEADs are states the snapshot must refuse, so they
    // have to be observable too.
    let branch = try_git(repo, &["symbolic-ref", "-q", "--short", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_string();
    let config = try_git(
        repo,
        &[
            "config",
            "--local",
            "-z",
            "--get-regexp",
            &format!("^branch\\.{}\\.", branch.replace('.', "\\.")),
        ],
    )
    .unwrap_or_default();
    let branch_config = config
        .split('\0')
        .filter(|e| !e.is_empty())
        .map(|e| {
            let (key, value) = e.split_once('\n').unwrap_or((e, ""));
            (
                key.rsplit('.').next().unwrap().to_string(),
                value.to_string(),
            )
        })
        .collect();
    let mut files = BTreeMap::new();
    let listed = git(
        repo,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    );
    for path in listed.split('\0').filter(|p| !p.is_empty()) {
        let full = repo.join(path);
        let summary = match fs::symlink_metadata(&full) {
            Err(_) => "missing".to_string(),
            Ok(meta) if meta.file_type().is_symlink() => {
                format!("link:{}", fs::read_link(&full).unwrap().display())
            }
            // A repository inside the checkout, listed as `path/`.
            Ok(meta) if meta.is_dir() => "dir".to_string(),
            Ok(meta) => {
                use std::os::unix::fs::PermissionsExt;
                let exec = meta.permissions().mode() & 0o111 != 0;
                let hash = hex::encode(Sha256::digest(fs::read(&full).unwrap()));
                format!("file:exec={exec}:{hash}")
            }
        };
        files.insert(path.to_string(), summary);
    }
    Observed {
        head: try_git(repo, &["rev-parse", "-q", "--verify", "HEAD"]).unwrap_or_default(),
        tracking,
        status: status.to_string(),
        staged: git(repo, &["diff", "--cached", "--binary"]),
        unstaged: git(repo, &["diff", "--binary"]),
        files,
        branch_config,
    }
}

/// What a transfer must leave exactly as it found it at the source: the
/// visible state, the index's content and every ref but its own.
pub(super) fn source_fingerprint(repo: &Path) -> (Observed, String, String) {
    let refs = git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"])
        .lines()
        .filter(|l| !l.starts_with("refs/staged-transfer/"))
        .collect::<Vec<_>>()
        .join("\n");
    (observe(repo), git(repo, &["ls-files", "-s"]), refs)
}

/// Staged, unstaged and untracked changes of every kind git distinguishes.
pub(super) fn make_a_mess(repo: &Path) {
    // Partially staged: the index has the first edit, the worktree both.
    write(repo, "partial.txt", "ONE\ntwo\nthree\nfour\nfive\n");
    git(repo, &["add", "partial.txt"]);
    write(repo, "partial.txt", "ONE\ntwo\nthree\nfour\nFIVE\n");
    git(repo, &["mv", "rename-me.txt", "renamed.txt"]);
    git(repo, &["rm", "-q", "delete-staged.txt"]);
    fs::remove_file(repo.join("delete-unstaged.txt")).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(repo.join("mode.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    git(repo, &["add", "mode.sh"]);
    std::os::unix::fs::symlink("a.txt", repo.join("staged-link")).unwrap();
    git(repo, &["add", "staged-link"]);
    fs::create_dir_all(repo.join("nested")).unwrap();
    std::os::unix::fs::symlink("../a.txt", repo.join("nested/untracked-link")).unwrap();
    write(repo, "bin.dat", noise(4096, 99));
    write(repo, "staged new.txt", "staged and new\n");
    git(repo, &["add", "staged new.txt"]);
    write(repo, "dir with space/naïve 文件.txt", "unicode\n");
    write(repo, "nested/dir/deep.txt", "deep\n");
    write(repo, "untracked.bin", noise(1000, 3));
    write(repo, "debug.log", "ignored, must stay behind\n");
}

fn assert_same_checkout(source: &Path, dest: &Path) {
    let (from, to) = (observe(source), observe(dest));
    assert_eq!(from.head, to.head, "HEAD");
    assert_eq!(from.tracking, to.tracking, "tracking");
    assert_eq!(from.status, to.status, "status");
    assert_eq!(from.staged, to.staged, "staged diff");
    assert_eq!(from.unstaged, to.unstaged, "unstaged diff");
    assert_eq!(from.files, to.files, "files");
    assert_eq!(from.branch_config, to.branch_config, "branch config");
}

fn snapshot_ref_exists(repo: &Path) -> bool {
    try_git(
        repo,
        &[
            "rev-parse",
            "-q",
            "--verify",
            &format!("refs/staged-transfer/{TID}/snapshot"),
        ],
    )
    .is_ok()
}

#[test]
fn carries_staged_unstaged_and_untracked_state_both_ways() {
    for shell in shells() {
        for direction in ["to_remote", "to_local"] {
            let w = World::new(shell);
            git(&w.source, &["checkout", "-q", "-b", "feature"]);
            write(&w.source, "a.txt", "alpha, committed on the branch\n");
            commit(&w.source, "branch work");
            make_a_mess(&w.source);
            let before = source_fingerprint(&w.source);

            let (outcome, checkout) = match direction {
                "to_remote" => w.to_remote("feature", "feature", true),
                _ => w.to_local(("feature", "feature"), "feature", true),
            }
            .unwrap_or_else(|e| panic!("{shell} {direction}: {e}"));

            assert!(outcome.snapshot.has_uncommitted_changes());
            assert!(!outcome.retried_without_exclusions);
            assert_same_checkout(&w.source, &checkout);
            assert!(!checkout.join("debug.log").exists(), "ignored files stay");
            assert_eq!(
                source_fingerprint(&w.source),
                before,
                "{shell} {direction}: the source must not change"
            );
            assert!(
                !snapshot_ref_exists(&w.dest),
                "the destination's temporary ref is dropped"
            );
            assert!(
                snapshot_ref_exists(&w.source),
                "the source keeps its snapshot"
            );
        }
    }
}

#[test]
fn intent_to_add_paths_arrive_untracked() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "intent.txt", "intended\n");
        git(&w.source, &["add", "-N", "intent.txt"]);

        let (_, dest) = w.to_remote("feature", "feature", true).unwrap();

        assert_eq!(observe(&w.source).files, observe(&dest).files);
        assert_eq!(observe(&dest).status, "?? intent.txt\n");
    }
}

/// A file rewritten at the same size within the same second as the index
/// was written has the stat data the index recorded for it. Git catches that
/// by comparing such entries' mtimes against the index file's own; a snapshot
/// that loses the index's mtime would take the edit for clean and leave it out.
#[test]
fn a_same_size_edit_in_the_same_second_as_the_index_is_carried() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "local\n");
        commit(&w.source, "local");
        write(&w.source, "f.txt", "dirty\n");
        // The snapshot's copy of the index must not look newer than the edit.
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let (outcome, dest) = w.to_remote("feature", "feature", true).unwrap();

        assert!(outcome.snapshot.has_uncommitted_changes(), "{shell}");
        assert_eq!(fs::read_to_string(dest.join("f.txt")).unwrap(), "dirty\n");
        assert_same_checkout(&w.source, &dest);
    }
}

#[test]
fn a_never_pushed_branch_arrives_tracking_nothing() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "new.txt", "new\n");
        commit(&w.source, "one");
        write(&w.source, "new.txt", "newer\n");
        commit(&w.source, "two");

        let (outcome, dest) = w.to_remote("feature", "feature", true).unwrap();

        // Only main was excluded: origin has no `feature` to report.
        assert_eq!(outcome.snapshot.excluded, 1);
        assert_same_checkout(&w.source, &dest);
        assert!(observe(&dest).branch_config.is_empty());
        assert_eq!(observe(&dest).tracking, "");
    }
}

#[test]
fn a_pushed_branch_arrives_the_same_number_of_commits_ahead() {
    for shell in shells() {
        for direction in ["to_remote", "to_local"] {
            let w = World::new(shell);
            git(&w.source, &["checkout", "-q", "-b", "feature"]);
            write(&w.source, "f.txt", "pushed\n");
            commit(&w.source, "pushed");
            git(&w.source, &["push", "-q", "-u", "origin", "feature"]);
            write(&w.source, "f.txt", "local only\n");
            commit(&w.source, "unpushed");

            let (outcome, checkout) = match direction {
                "to_remote" => w.to_remote("feature", "feature", true),
                _ => w.to_local(("feature", "feature"), "feature", true),
            }
            .unwrap();

            assert_eq!(outcome.snapshot.excluded, 2);
            assert_same_checkout(&w.source, &checkout);
            assert_eq!(observe(&checkout).tracking, "origin/feature [ahead 1]");
        }
    }
}

#[test]
fn a_diverged_branch_arrives_ahead_and_behind() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "base\n");
        commit(&w.source, "base");
        git(&w.source, &["push", "-q", "-u", "origin", "feature"]);
        let other = w.other_clone("other");
        git(&other, &["checkout", "-q", "feature"]);
        write(&other, "g.txt", "theirs\n");
        commit(&other, "theirs");
        git(&other, &["push", "-q", "origin", "feature"]);
        write(&w.source, "f.txt", "mine\n");
        commit(&w.source, "mine");
        git(&w.source, &["fetch", "-q", "origin"]);

        let (_, dest) = w.to_remote("feature", "feature", true).unwrap();

        assert_same_checkout(&w.source, &dest);
        assert_eq!(
            observe(&dest).tracking,
            "origin/feature [ahead 1, behind 1]"
        );
    }
}

/// A fork PR's head lives on origin under a name the branch does not track.
#[test]
fn a_branch_with_no_upstream_config_stays_without_one() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "pr head\n");
        commit(&w.source, "pr head");
        git(&w.source, &["push", "-q", "origin", "feature"]);
        assert!(try_git(&w.source, &["config", "branch.feature.remote"]).is_err());

        let (_, checkout) = w.to_local(("feature", "feature"), "feature", true).unwrap();

        assert_same_checkout(&w.source, &checkout);
        assert!(observe(&checkout).branch_config.is_empty());
    }
}

#[test]
fn a_deduplicated_branch_keeps_tracking_the_original_name() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "pushed\n");
        commit(&w.source, "pushed");
        git(&w.source, &["push", "-q", "origin", "feature"]);
        git(&w.source, &["checkout", "-q", "main"]);
        git(&w.source, &["branch", "-q", "-D", "feature"]);
        git(
            &w.source,
            &[
                "checkout",
                "-q",
                "-b",
                "feature-2",
                "--track",
                "origin/feature",
            ],
        );
        write(&w.source, "f.txt", "local\n");
        commit(&w.source, "local");

        let (outcome, dest) = w.to_remote("feature-2", "feature", true).unwrap();

        assert_eq!(outcome.snapshot.excluded, 2);
        assert_same_checkout(&w.source, &dest);
        assert_eq!(
            observe(&dest).branch_config,
            [
                ("remote".to_string(), "origin".to_string()),
                ("merge".to_string(), "refs/heads/feature".to_string()),
            ]
        );
        assert_eq!(observe(&dest).tracking, "origin/feature [ahead 1]");
    }
}

#[test]
fn a_non_origin_upstream_remote_is_added_and_fetched() {
    for shell in shells() {
        for direction in ["to_remote", "to_local"] {
            let w = World::new(shell);
            git(
                &w.root,
                &["init", "-q", "--bare", "--initial-branch=main", "fork.git"],
            );
            let fork = w.root.join("fork.git");
            git(
                &w.source,
                &["remote", "add", "fork", fork.to_str().unwrap()],
            );
            git(&w.source, &["checkout", "-q", "-b", "feature"]);
            write(&w.source, "f.txt", "on the fork\n");
            commit(&w.source, "fork");
            git(&w.source, &["push", "-q", "-u", "fork", "feature"]);
            write(&w.source, "f.txt", "ahead of the fork\n");
            commit(&w.source, "ahead");

            let (outcome, checkout) = match direction {
                "to_remote" => w.to_remote("feature", "feature", true),
                _ => w.to_local(("feature", "feature"), "feature", true),
            }
            .unwrap();

            assert_eq!(outcome.applied.added_remotes, ["fork"]);
            assert!(outcome.applied.warnings.is_empty());
            assert_same_checkout(&w.source, &checkout);
            assert_eq!(observe(&checkout).tracking, "fork/feature [ahead 1]");
            assert_eq!(
                git(&checkout, &["config", "remote.fork.url"]).trim(),
                fork.to_str().unwrap()
            );
        }
    }
}

/// HEAD is one of the excluded tips, but the snapshot commits are new, so the
/// bundle is never empty.
#[test]
fn a_clean_branch_with_nothing_unpushed_still_transfers() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "pushed\n");
        commit(&w.source, "pushed");
        git(&w.source, &["push", "-q", "-u", "origin", "feature"]);

        let (outcome, dest) = w.to_remote("feature", "feature", true).unwrap();

        assert!(!outcome.snapshot.has_uncommitted_changes());
        assert_same_checkout(&w.source, &dest);
        assert_eq!(observe(&dest).tracking, "origin/feature");
        assert_eq!(observe(&dest).status, "");
    }
}

/// A source whose bundle is cut against `first`, a commit the destination
/// never had — as when origin is rewritten between the destination's fetch
/// and the apply. Returns `first`.
fn cut_against_a_commit_the_destination_lacks(w: &World) -> String {
    git(&w.source, &["checkout", "-q", "-b", "feature"]);
    write(&w.source, "f.txt", "one\n");
    let first = commit(&w.source, "one");
    write(&w.source, "f.txt", "two\n");
    commit(&w.source, "two");
    write(&w.source, "f.txt", "uncommitted\n");
    first
}

#[test]
fn a_bundle_missing_prerequisites_is_refused_as_such() {
    for shell in shells() {
        let w = World::new(shell);
        let first = cut_against_a_commit_the_destination_lacks(&w);
        let bundle = w.root.join("cut.bundle");
        let bundle = bundle.to_str().unwrap();
        let source = w.endpoint(&w.local_scratch);
        let snapshot = source
            .run_script(
                scripts::SNAPSHOT_SCRIPT,
                &scripts::snapshot_args(
                    w.source.to_str().unwrap(),
                    "feature",
                    bundle,
                    TID,
                    &[first],
                ),
            )
            .unwrap();
        let snapshot = Snapshot::parse(&snapshot.stdout, "feature").unwrap();
        assert_eq!(snapshot.excluded, 1);

        let applied = w
            .endpoint(&w.remote_scratch)
            .run_script(
                scripts::APPLY_SCRIPT,
                &scripts::ApplyRequest {
                    mode: scripts::ApplyMode::Checkout,
                    repo_dir: w.dest.to_str().unwrap(),
                    bundle_path: bundle,
                    bundle: &snapshot.bundle,
                    transfer_id: TID,
                    branch: "feature",
                    snapshot: &snapshot,
                    restore: true,
                }
                .args(),
            )
            .unwrap();

        assert!(!applied.success);
        assert!(
            applied.stdout.contains("error_kind=missing_prerequisites"),
            "{shell}: {}",
            applied.stdout
        );
        assert_eq!(
            rev(&w.dest, "HEAD"),
            rev(&w.origin, "main"),
            "nothing applied"
        );
    }
}

#[test]
fn a_missing_prerequisite_is_retried_without_exclusions() {
    for shell in shells() {
        let w = World::new(shell);
        let first = cut_against_a_commit_the_destination_lacks(&w);
        let source = w.endpoint(&w.local_scratch);
        let dest = w.endpoint(&w.remote_scratch);
        let plan = w.plan(
            &source,
            Landing::Checkout,
            ("feature", "feature"),
            true,
            vec![first],
        );

        let outcome = carry_repo(&source, &dest, &plan).unwrap();

        assert!(outcome.retried_without_exclusions);
        assert_ne!(outcome.bundle, outcome.snapshot.bundle, "a new bundle went");
        assert_same_checkout(&w.source, &w.dest);
    }
}

#[test]
fn leaving_uncommitted_changes_behind_keeps_them_in_the_backup() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "committed\n");
        commit(&w.source, "committed");
        make_a_mess(&w.source);
        let before = source_fingerprint(&w.source);

        let (outcome, dest) = w.to_remote("feature", "feature", false).unwrap();

        assert_eq!(rev(&dest, "HEAD"), rev(&w.source, "HEAD"));
        assert_eq!(observe(&dest).status, "", "nothing uncommitted arrives");
        assert_eq!(source_fingerprint(&w.source), before);
        // The bundle kept on this machine still holds the working tree.
        let backup = w.local_scratch.join(TID).join("repo.bundle");
        assert_eq!(
            outcome.dest_bundle,
            w.remote_scratch
                .join(TID)
                .join("repo.bundle")
                .to_str()
                .unwrap()
        );
        git(
            &dest,
            &[
                "fetch",
                "-q",
                backup.to_str().unwrap(),
                &format!("refs/staged-transfer/{TID}/snapshot:refs/backup-check"),
            ],
        );
        assert_eq!(
            rev(&dest, "refs/backup-check^{tree}"),
            outcome.snapshot.tree_w
        );
    }
}

#[test]
fn a_renamed_local_branch_lands_under_its_new_name() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "pushed\n");
        commit(&w.source, "pushed");
        git(&w.source, &["push", "-q", "-u", "origin", "feature"]);
        write(&w.source, "f.txt", "local\n");
        commit(&w.source, "local");
        write(&w.source, "f.txt", "dirty\n");
        // The destination clone already has `feature`, somewhere else.
        git(&w.dest, &["branch", "-q", "feature", "origin/main"]);

        let taken = w.to_local(("feature", "feature"), "feature", true);
        assert!(
            matches!(&taken, Err(TransferError::Script { message, .. }) if message.contains("already exists")),
            "{taken:?}"
        );
        assert_eq!(rev(&w.dest, "feature"), rev(&w.origin, "main"));

        let (_, worktree) = w
            .to_local(("feature", "feature-2"), "feature", true)
            .unwrap();

        assert_same_checkout(&w.source, &worktree);
        assert_eq!(
            git(&worktree, &["symbolic-ref", "--short", "HEAD"]).trim(),
            "feature-2"
        );
    }
}

/// A name, how to get the source into the state, and what the refusal says.
type Blocker = (&'static str, Box<dyn Fn(&World)>, &'static str);

#[test]
fn the_snapshot_refuses_states_it_cannot_carry() {
    for shell in shells() {
        let cases: Vec<Blocker> = vec![
            (
                "detached",
                Box::new(|w: &World| {
                    git(&w.source, &["checkout", "-q", "--detach"]);
                }),
                "detached",
            ),
            (
                "wrong branch",
                Box::new(|w: &World| {
                    git(&w.source, &["checkout", "-q", "-b", "other"]);
                }),
                "not feature",
            ),
            (
                "merge",
                Box::new(|w: &World| {
                    git(&w.source, &["checkout", "-q", "-b", "side"]);
                    write(&w.source, "a.txt", "side\n");
                    commit(&w.source, "side");
                    git(&w.source, &["checkout", "-q", "-b", "feature", "main"]);
                    write(&w.source, "a.txt", "feature\n");
                    commit(&w.source, "feature");
                    assert!(try_git(&w.source, &["merge", "-q", "side"]).is_err());
                }),
                "merge is in progress",
            ),
            (
                "conflicts",
                Box::new(|w: &World| {
                    git(&w.source, &["checkout", "-q", "-b", "side"]);
                    write(&w.source, "a.txt", "side\n");
                    commit(&w.source, "side");
                    git(&w.source, &["checkout", "-q", "-b", "feature", "main"]);
                    write(&w.source, "a.txt", "feature\n");
                    commit(&w.source, "feature");
                    assert!(try_git(&w.source, &["merge", "-q", "side"]).is_err());
                    // Abandon the merge but keep the conflicted index.
                    fs::remove_file(w.source.join(".git/MERGE_HEAD")).unwrap();
                }),
                "unresolved conflicts",
            ),
            (
                "unborn",
                Box::new(|w: &World| {
                    git(&w.source, &["checkout", "-q", "--orphan", "feature"]);
                    git(&w.source, &["rm", "-q", "-r", "--cached", "."]);
                }),
                "no commits",
            ),
        ];
        for (name, setup, expected) in cases {
            let w = World::new(shell);
            setup(&w);
            let before = source_fingerprint(&w.source);

            let err = w.to_remote("feature", "feature", true).unwrap_err();

            match &err {
                TransferError::Script { step, message, .. } => {
                    assert_eq!(*step, "snapshot", "{name}");
                    assert!(message.contains(expected), "{name}: {message}");
                }
                other => panic!("{name}: {other:?}"),
            }
            assert_eq!(source_fingerprint(&w.source), before, "{name}");
            assert!(!snapshot_ref_exists(&w.source), "{name}");
        }
    }
}

#[test]
fn hooks_never_run_at_either_end() {
    for shell in shells() {
        for direction in ["to_remote", "to_local"] {
            let w = World::new(shell);
            let hooks = w.root.join("hooks");
            let marker = w.root.join("hook-ran");
            fs::create_dir_all(&hooks).unwrap();
            for hook in [
                "post-checkout",
                "reference-transaction",
                "post-index-change",
                "pre-commit",
                "post-commit",
            ] {
                let path = hooks.join(hook);
                fs::write(
                    &path,
                    format!("#!/bin/sh\necho {hook} >> '{}'\n", marker.display()),
                )
                .unwrap();
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            git(&w.source, &["checkout", "-q", "-b", "feature"]);
            write(&w.source, "f.txt", "x\n");
            commit(&w.source, "x");
            make_a_mess(&w.source);
            for repo in [&w.source, &w.dest] {
                git(repo, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
            }

            match direction {
                "to_remote" => w.to_remote("feature", "feature", true),
                _ => w.to_local(("feature", "feature"), "feature", true),
            }
            .unwrap();

            assert!(
                !marker.exists(),
                "{direction}: {}",
                fs::read_to_string(&marker).unwrap_or_default()
            );
        }
    }
}

#[test]
fn a_checkout_landing_refuses_a_dirty_destination() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "x\n");
        commit(&w.source, "x");
        write(&w.source, "notes.txt", "the source's own\n");
        write(&w.dest, "a.txt", "someone's edit\n");

        let err = w.to_remote("feature", "feature", true).unwrap_err();

        assert!(err.to_string().contains("uncommitted changes"), "{err}");
        assert_eq!(
            fs::read_to_string(w.dest.join("a.txt")).unwrap(),
            "someone's edit\n"
        );

        // An untracked file the branch would write over is refused, whether
        // the branch's commits have it or only its working state does.
        git(&w.dest, &["checkout", "-q", "--", "a.txt"]);
        for clash in ["f.txt", "notes.txt"] {
            write(&w.dest, clash, "not the branch's\n");
            let err = w.to_remote("feature", "feature", true).unwrap_err();
            assert!(
                err.to_string()
                    .contains(&format!("has an untracked {clash}, which the branch")),
                "{err}"
            );
            assert_eq!(rev(&w.dest, "HEAD"), rev(&w.dest, "origin/main"));
            assert_eq!(
                fs::read_to_string(w.dest.join(clash)).unwrap(),
                "not the branch's\n"
            );
            fs::remove_file(w.dest.join(clash)).unwrap();
        }

        // Any other (a workspace's bootstrap leaves some) stays, with a
        // warning so it isn't taken for the branch's.
        write(&w.dest, "bootstrap/done", "bootstrapped\n");
        let (outcome, dest) = w.to_remote("feature", "feature", true).unwrap();
        assert_eq!(
            outcome.applied.warnings,
            vec!["1 untracked file was already in the destination checkout \
                 (bootstrap/done, for one) and stayed there"
                .to_string()]
        );
        assert_eq!(
            fs::read_to_string(dest.join("bootstrap/done")).unwrap(),
            "bootstrapped\n"
        );
        assert_eq!(
            fs::read_to_string(dest.join("notes.txt")).unwrap(),
            "the source's own\n"
        );
    }
}

/// `created_branch` is what a rollback deletes, so a branch the apply only
/// reused must not be reported — and a failed apply takes back the branch and
/// remotes it did add.
#[test]
fn a_branch_landing_reports_only_a_branch_it_created_and_undoes_it_on_failure() {
    for shell in shells() {
        let w = World::new(shell);
        let fork = w.root.join("fork.git");
        git(
            &w.root,
            &["init", "-q", "--bare", "--initial-branch=main", "fork.git"],
        );
        git(
            &w.source,
            &["remote", "add", "fork", fork.to_str().unwrap()],
        );
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "x\n");
        let head = commit(&w.source, "x");
        git(&w.source, &["push", "-q", "-u", "fork", "feature"]);
        let source = w.endpoint(&w.remote_scratch);
        let dest = w.endpoint(&w.local_scratch);
        let plan = w.plan(
            &source,
            Landing::Branch,
            ("feature", "feature"),
            true,
            vec![],
        );

        let created = carry_repo(&source, &dest, &plan).unwrap();
        assert!(created.applied.created_branch);
        assert_eq!(rev(&w.dest, "refs/heads/feature"), head);

        // The same tip again: reused, so not reported.
        git(&w.dest, &["remote", "remove", "fork"]);
        let reused = carry_repo(&source, &dest, &plan).unwrap();
        assert!(!reused.applied.created_branch);
        assert_eq!(reused.applied.added_remotes, ["fork"]);

        // An apply that fails after creating the branch and adding the
        // remote leaves neither.
        git(&w.dest, &["branch", "-q", "-D", "feature"]);
        git(&w.dest, &["remote", "remove", "fork"]);
        let request = scripts::ApplyRequest {
            mode: scripts::ApplyMode::Branch,
            repo_dir: w.dest.to_str().unwrap(),
            bundle_path: &reused.dest_bundle,
            bundle: &reused.bundle,
            transfer_id: TID,
            branch: "feature",
            snapshot: &reused.snapshot,
            restore: false,
        };
        let mut args = request.args();
        args.push("bogus".to_string());
        let output = dest.run_script(scripts::APPLY_SCRIPT, &args).unwrap();
        assert!(!output.success);
        assert!(
            output.stdout.contains("created_branch=1"),
            "{}",
            output.stdout
        );
        assert!(
            output.stdout.contains("added_remote=fork"),
            "{}",
            output.stdout
        );
        assert!(
            output.stdout.contains("unknown config record"),
            "{}",
            output.stdout
        );
        assert!(try_git(&w.dest, &["rev-parse", "--verify", "refs/heads/feature"]).is_err());
        assert!(try_git(
            &w.dest,
            &[
                "config",
                "--get-regexp",
                "^(branch\\.feature|remote\\.fork)\\."
            ]
        )
        .is_err());
    }
}

#[test]
fn a_same_named_remote_elsewhere_is_kept_with_a_warning() {
    for shell in shells() {
        let w = World::new(shell);
        for name in ["fork.git", "theirs.git"] {
            git(
                &w.root,
                &["init", "-q", "--bare", "--initial-branch=main", name],
            );
        }
        let (fork, theirs) = (w.root.join("fork.git"), w.root.join("theirs.git"));
        git(
            &w.source,
            &["remote", "add", "fork", fork.to_str().unwrap()],
        );
        git(
            &w.dest,
            &["remote", "add", "fork", theirs.to_str().unwrap()],
        );
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "x\n");
        commit(&w.source, "x");
        git(&w.source, &["push", "-q", "-u", "fork", "feature"]);

        let (outcome, dest) = w.to_remote("feature", "feature", true).unwrap();

        assert!(outcome.applied.added_remotes.is_empty());
        let expected = format!(
            "remote fork here points at {}, not {} as at the source; feature tracks it as it is here",
            theirs.display(),
            fork.display()
        );
        assert!(
            outcome.applied.warnings.contains(&expected),
            "{:?}",
            outcome.applied.warnings
        );
        assert_eq!(
            git(&dest, &["config", "remote.fork.url"]).trim(),
            theirs.to_str().unwrap(),
            "theirs is left alone"
        );
    }
}

#[test]
fn verify_says_whether_a_checkout_is_still_its_snapshot_and_writes_nothing() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "x\n");
        commit(&w.source, "x");
        make_a_mess(&w.source);
        let (outcome, _) = w.to_remote("feature", "feature", true).unwrap();
        let source = w.endpoint(&w.local_scratch);
        let plan = w.plan(
            &source,
            Landing::Checkout,
            ("feature", "feature"),
            true,
            vec![],
        );
        let check = || unchanged(&source, &plan, &outcome.snapshot).unwrap();

        let before = source_fingerprint(&w.source);
        assert_eq!(check(), None);
        assert_eq!(source_fingerprint(&w.source), before, "read only");
        assert!(!w.local_scratch.join(TID).join("repo.bundle.index").exists());

        write(&w.source, "late.txt", "late\n");
        assert_eq!(check().as_deref(), Some("files changed"));
        fs::remove_file(w.source.join("late.txt")).unwrap();
        // Ignored files are not part of the snapshot, so not a change.
        write(&w.source, "late.log", "ignored\n");
        assert_eq!(check(), None);

        git(&w.source, &["add", "untracked.bin"]);
        assert_eq!(check().as_deref(), Some("the index changed"));
        git(&w.source, &["rm", "-q", "--cached", "untracked.bin"]);
        assert_eq!(check(), None);

        git(&w.source, &["checkout", "-q", "-b", "elsewhere"]);
        assert_eq!(check().as_deref(), Some("feature is no longer checked out"));
        git(&w.source, &["checkout", "-q", "feature"]);
        let head = outcome.snapshot.head.clone();
        let moved = git(
            &w.source,
            &[
                "commit-tree",
                "-m",
                "moved",
                "-p",
                &head,
                &format!("{head}^{{tree}}"),
            ],
        );
        git(
            &w.source,
            &["update-ref", "refs/heads/feature", moved.trim()],
        );
        assert_eq!(
            check(),
            Some(format!("HEAD moved from {head} to {}", moved.trim()))
        );
    }
}

#[test]
fn verify_checks_every_checkout_at_an_end_in_one_run() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "x\n");
        commit(&w.source, "x");
        write(&w.source, "notes.txt", "untracked\n");
        let (outcome, _) = w.to_remote("feature", "feature", true).unwrap();
        let source = Recording::new(w.endpoint(&w.local_scratch));
        let plan = CarryPlan {
            untracked_bytes: 9,
            ..w.plan(
                &source.inner,
                Landing::Checkout,
                ("feature", "feature"),
                true,
                vec![],
            )
        };
        let gone = CarryPlan {
            source_dir: w.root.join("gone").to_str().unwrap().to_string(),
            bundle_name: "gone.bundle".to_string(),
            ..plan.clone()
        };
        let elsewhere = CarryPlan {
            source_branch: "elsewhere".to_string(),
            bundle_name: "elsewhere.bundle".to_string(),
            ..plan.clone()
        };
        let snapshot = &outcome.snapshot;

        let answers = unchanged_all(
            &source,
            &[(&plan, snapshot), (&gone, snapshot), (&elsewhere, snapshot)],
        )
        .unwrap();

        assert_eq!(
            source.runs(),
            vec![("verify", 3 * (9 + snapshot.bundle.size))]
        );
        assert!(matches!(answers[0], Ok(None)), "{:?}", answers[0]);
        let gone_err = answers[1].as_ref().unwrap_err().to_string();
        assert!(gone_err.contains("cannot enter"), "{shell}: {gone_err}");
        assert_eq!(
            answers[2].as_ref().unwrap().as_deref(),
            Some("elsewhere is no longer checked out")
        );
    }
}

/// A [`LocalEndpoint`] noting which script each run was and the payload it
/// was sized for.
struct Recording {
    inner: LocalEndpoint,
    runs: std::sync::Mutex<Vec<(&'static str, u64)>>,
}

impl Recording {
    fn new(inner: LocalEndpoint) -> Self {
        Self {
            inner,
            runs: Default::default(),
        }
    }

    fn runs(&self) -> Vec<(&'static str, u64)> {
        self.runs.lock().unwrap().clone()
    }
}

fn script_name(script: &str) -> &'static str {
    [
        ("snapshot", scripts::SNAPSHOT_SCRIPT),
        ("rebundle", scripts::REBUNDLE_SCRIPT),
        ("verify", scripts::VERIFY_SCRIPT),
        ("apply", scripts::APPLY_SCRIPT),
        ("origin_tips", scripts::ORIGIN_TIPS_SCRIPT),
        ("cleanup", scripts::CLEANUP_SCRIPT),
    ]
    .into_iter()
    .find(|(_, s)| *s == script)
    .map_or("other", |(name, _)| name)
}

impl TransferEndpoint for Recording {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn run_script(&self, script: &str, args: &[String]) -> Result<ScriptOutput, TransferError> {
        self.run_script_sized(script, args, 0)
    }

    fn run_script_sized(
        &self,
        script: &str,
        args: &[String],
        payload_bytes: u64,
    ) -> Result<ScriptOutput, TransferError> {
        self.runs
            .lock()
            .unwrap()
            .push((script_name(script), payload_bytes));
        self.inner.run_script_sized(script, args, payload_bytes)
    }

    fn scratch_dir(&self, transfer_id: &str) -> Result<String, TransferError> {
        self.inner.scratch_dir(transfer_id)
    }

    fn put_file(&self, local: &Path, path: &str) -> Result<(), TransferError> {
        self.inner.put_file(local, path)
    }

    fn get_file(&self, path: &str, local: &Path, size: u64) -> Result<(), TransferError> {
        self.inner.get_file(path, local, size)
    }
}

/// What a step takes time in proportion to is what an end that gives up on
/// slow scripts waits for: the untracked files and the bundle — the whole
/// repository only to re-bundle all of it.
#[test]
fn each_step_is_sized_by_what_it_reads_and_writes() {
    let w = World::new("/bin/sh");
    let first = cut_against_a_commit_the_destination_lacks(&w);
    let source = Recording::new(w.endpoint(&w.remote_scratch));
    let dest = Recording::new(w.endpoint(&w.local_scratch));
    let plan = CarryPlan {
        repo_bytes: 1_000_000,
        untracked_bytes: 10,
        ..w.plan(
            &source.inner,
            Landing::Branch,
            ("feature", "feature"),
            true,
            vec![first],
        )
    };

    let outcome = carry_repo(&source, &dest, &plan).unwrap();
    assert!(outcome.retried_without_exclusions);
    let worktree = w.root.join("wt-feature");
    git(
        &w.dest,
        &[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "feature",
        ],
    );
    finish(&dest, &plan, &outcome, worktree.to_str().unwrap()).unwrap();

    let (cut, whole) = (outcome.snapshot.bundle.size, outcome.bundle.size);
    assert_eq!(
        source.runs(),
        vec![("snapshot", 10), ("rebundle", 1_000_000)]
    );
    assert_eq!(
        dest.runs(),
        vec![
            ("apply", 10 + cut),
            ("apply", 10 + whole),
            ("apply", 10 + whole)
        ]
    );
}

#[test]
fn cleanup_removes_temporary_refs_and_the_scratch_directory() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "f.txt", "x\n");
        commit(&w.source, "x");
        let source = w.endpoint(&w.local_scratch);
        let dest = w.endpoint(&w.remote_scratch);
        let plan = w.plan(
            &source,
            Landing::Branch,
            ("feature", "feature"),
            true,
            vec![],
        );
        carry_repo(&source, &dest, &plan).unwrap();
        assert!(snapshot_ref_exists(&w.dest), "kept for finish");
        assert!(w.remote_scratch.join(TID).exists());

        cleanup(&dest, w.dest.to_str().unwrap(), TID, true).unwrap();
        cleanup(&source, w.source.to_str().unwrap(), TID, false).unwrap();

        assert!(!snapshot_ref_exists(&w.dest));
        assert!(!snapshot_ref_exists(&w.source));
        assert!(!w.remote_scratch.join(TID).exists());
        assert!(
            w.local_scratch.join(TID).join("repo.bundle").exists(),
            "the backup stays"
        );
        assert!(matches!(
            cleanup(&dest, w.dest.to_str().unwrap(), "../..", true),
            Err(TransferError::InvalidPlan(_))
        ));
    }
}

#[test]
fn carries_a_hundred_megabyte_untracked_file() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        write(&w.source, "big.bin", noise(100 * 1024 * 1024, 42));

        let (outcome, dest) = w.to_remote("feature", "feature", true).unwrap();

        assert!(outcome.bundle.size > 100 * 1000 * 1000);
        assert_same_checkout(&w.source, &dest);
    }
}

#[test]
fn carries_ten_thousand_files() {
    for shell in shells() {
        let w = World::new(shell);
        git(&w.source, &["checkout", "-q", "-b", "feature"]);
        for i in 0..10_000 {
            write(
                &w.source,
                &format!("many/{:03}/file-{i}.txt", i % 100),
                format!("file {i}\n"),
            );
        }
        // Half staged, half untracked.
        git(&w.source, &["add", "many/0[0-4]*"]);

        let (_, checkout) = w.to_local(("feature", "feature"), "feature", true).unwrap();

        // The seed's eight files, and the ten thousand.
        assert_eq!(observe(&checkout).files.len(), 10_008);
        assert_same_checkout(&w.source, &checkout);
    }
}
