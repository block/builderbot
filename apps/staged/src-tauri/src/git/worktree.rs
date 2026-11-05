//! Git worktree operations for branch-based workflow.
//!
//! Manages worktrees under the platform data directory
//! (see [`crate::paths::worktrees_dir`]), including project-scoped layouts.

use super::cli::{self, GitError};
use std::path::{Path, PathBuf};

/// Get the standard worktree base directory via [`crate::paths::worktrees_dir`].
fn worktree_base_dir() -> Result<PathBuf, GitError> {
    crate::paths::worktrees_dir()
        .ok_or_else(|| GitError::CommandFailed("Cannot determine data directory".to_string()))
}

fn sanitize_path_segment(raw: &str, fallback: &str) -> String {
    let safe = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if safe.is_empty() {
        fallback.to_string()
    } else {
        safe
    }
}

fn sanitize_branch_name(branch_name: &str) -> String {
    branch_name.replace('/', "-")
}

fn ensure_worktree_parent_exists(worktree_path: &Path) -> Result<(), GitError> {
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            GitError::CommandFailed(format!("Failed to create worktree directory: {e}"))
        })?;
    }
    Ok(())
}

fn ensure_worktree_absent(worktree_path: &Path) -> Result<(), GitError> {
    if worktree_path.exists() {
        return Err(GitError::CommandFailed(format!(
            "Worktree already exists at {}",
            worktree_path.display()
        )));
    }
    Ok(())
}

/// Compute the worktree path for a given repo and branch.
/// Format: `<worktrees_dir>/<repo-name>/<sanitized-branch-name>/`
pub fn worktree_path_for(repo: &Path, branch_name: &str) -> Result<PathBuf, GitError> {
    let base = worktree_base_dir()?;

    // Get repo name from path (last component)
    let repo_name = repo
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| GitError::InvalidPath(repo.display().to_string()))?;

    let sanitized_branch = sanitize_branch_name(branch_name);

    Ok(base.join(repo_name).join(sanitized_branch))
}

/// Compute the project-scoped worktree path used for local multi-repo projects.
/// Format: `<worktrees_dir>/projects/<project-id>/<repo-slug>--<sanitized-branch-name>/`
pub fn project_worktree_path_for(
    project_id: &str,
    repo_slug: &str,
    branch_name: &str,
) -> Result<PathBuf, GitError> {
    let base = project_worktree_root_for(project_id)?;
    let repo_segment = sanitize_path_segment(repo_slug, "repo");
    let branch_segment = sanitize_branch_name(branch_name);
    let worktree_name = format!("{repo_segment}--{branch_segment}");
    Ok(base.join(worktree_name))
}

/// Compute the project-scoped root directory for all worktrees in a project.
/// Format: `<worktrees_dir>/projects/<project-id>/`
pub fn project_worktree_root_for(project_id: &str) -> Result<PathBuf, GitError> {
    let base = worktree_base_dir()?;
    let project_segment = sanitize_path_segment(project_id, "project");
    Ok(base.join("projects").join(project_segment))
}

/// Create a new worktree with a new branch.
///
/// Creates the branch from the specified start point and sets up a worktree
/// at the standard location. If the start point is a remote-tracking ref
/// (e.g. `origin/main`), fetches the latest from the remote first so the
/// branch starts from the up-to-date remote tip rather than a stale local copy.
///
/// Returns the path to the created worktree.
pub fn create_worktree(
    repo: &Path,
    branch_name: &str,
    start_point: &str,
) -> Result<PathBuf, GitError> {
    let worktree_path = worktree_path_for(repo, branch_name)?;
    create_worktree_at_path(repo, branch_name, start_point, &worktree_path)
}

/// Create a new worktree with a new branch at an explicit path.
pub fn create_worktree_at_path(
    repo: &Path,
    branch_name: &str,
    start_point: &str,
    worktree_path: &Path,
) -> Result<PathBuf, GitError> {
    ensure_worktree_parent_exists(worktree_path)?;
    ensure_worktree_absent(worktree_path)?;

    // Normalise the start point to "origin/<branch>" form unless it's
    // already a full ref or a bare SHA (e.g. from fetch_pr_head_sha).
    let is_sha = start_point.len() == 40 && start_point.chars().all(|c| c.is_ascii_hexdigit());
    let remote_start = if is_sha || start_point.starts_with("origin/") {
        start_point.to_string()
    } else {
        format!("origin/{start_point}")
    };

    let worktree_str = worktree_path
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

    // Create worktree with new branch from start point:
    // git worktree add <path> -b <branch> origin/<start-point>
    cli::run(
        repo,
        &[
            "worktree",
            "add",
            worktree_str,
            "-b",
            branch_name,
            &remote_start,
        ],
    )?;

    Ok(worktree_path.to_path_buf())
}

/// Create a new worktree for an existing local branch.
///
/// Uses the standard worktree location and checks out `branch_name` there.
/// Fails if the worktree path already exists.
pub fn create_worktree_for_existing_branch(
    repo: &Path,
    branch_name: &str,
) -> Result<PathBuf, GitError> {
    let worktree_path = worktree_path_for(repo, branch_name)?;
    create_worktree_for_existing_branch_at_path(repo, branch_name, &worktree_path)
}

/// Create a new worktree for an existing local branch at an explicit path.
pub fn create_worktree_for_existing_branch_at_path(
    repo: &Path,
    branch_name: &str,
    worktree_path: &Path,
) -> Result<PathBuf, GitError> {
    ensure_worktree_parent_exists(worktree_path)?;
    ensure_worktree_absent(worktree_path)?;

    let worktree_str = worktree_path
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

    // Create worktree for existing branch:
    // git worktree add <path> <branch>
    cli::run(repo, &["worktree", "add", worktree_str, branch_name])?;

    Ok(worktree_path.to_path_buf())
}

/// Move an existing worktree to a new path.
///
/// Goes through `git worktree move` rather than `fs::rename` because a
/// worktree is two pointers, not one directory: the `.git` gitfile inside the
/// worktree names the repo's admin directory, and that admin directory's
/// `gitdir` file names the worktree. A rename updates neither, leaving both
/// dangling; `git worktree move` rewrites both.
///
/// Runs via `cli::run_lite_or_captured`, like the teardown in
/// [`remove_worktree`]: the move renames the directory and rewrites those two
/// files, nothing more, so it has no use for the clone root's captured env
/// and shouldn't wait on its capture with the branch's launch lock held.
pub fn move_worktree(repo: &Path, from: &Path, to: &Path) -> Result<(), GitError> {
    ensure_worktree_parent_exists(to)?;
    ensure_worktree_absent(to)?;

    let from_str = from
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(from.display().to_string()))?;
    let to_str = to
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(to.display().to_string()))?;

    cli::run_lite_or_captured(repo, &["worktree", "move", from_str, to_str])?;
    Ok(())
}

/// Remove a worktree and its associated branch.
///
/// Removes the worktree directory, git worktree reference, and the local git branch.
/// Handles various edge cases:
/// - Normal case: directory exists and git knows about it
/// - Directory deleted: just prune stale git references
/// - Git references deleted: just remove the orphaned directory
/// - Directory not empty: git can't remove due to untracked files (node_modules, etc.)
///
/// The branch_name parameter is optional - if provided, the local branch will be deleted.
/// This is important for allowing the branch to be recreated later.
///
/// The worktree's own git calls go through `cli::run_lite_or_captured`:
/// they only touch `.git` admin files and the filesystem, so waiting on the
/// clone's shell-env capture (which nothing else may have warmed) would only
/// delay the teardown. The branch delete goes through [`force_delete_branch`],
/// which waits on the capture only if the lite env's delete fails in a way
/// the captured env might fix, and its failure fails the teardown, after the
/// worktree itself is gone.
pub fn remove_worktree(repo: &Path, worktree_path: &Path) -> Result<(), GitError> {
    // First, get the branch name from the worktree before removing it
    let branch_name = get_worktree_branch(repo, worktree_path);

    if worktree_path.exists() {
        // Worktree directory exists on disk - try to remove it normally
        let worktree_str = worktree_path
            .to_str()
            .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

        // Try: git worktree remove <path> --force
        let result =
            cli::run_lite_or_captured(repo, &["worktree", "remove", worktree_str, "--force"]);

        if let Err(e) = result {
            let error_msg = e.to_string();

            // If git doesn't recognize it as a worktree (admin files already deleted),
            // or if directory is not empty (untracked files like node_modules),
            // remove the directory manually
            if error_msg.contains("is not a working tree")
                || error_msg.contains("Directory not empty")
            {
                std::fs::remove_dir_all(worktree_path).map_err(|io_err| {
                    GitError::CommandFailed(format!(
                        "Failed to remove worktree directory: {io_err}"
                    ))
                })?;
                // Prune any remaining stale references
                cli::run_lite_or_captured(repo, &["worktree", "prune"])?;
            } else {
                return Err(e);
            }
        }
    } else {
        // Worktree was already deleted from disk - prune stale references
        cli::run_lite_or_captured(repo, &["worktree", "prune"])?;
    }

    // Delete the local branch if we found one
    // Use -D (force delete) since the branch may not be fully merged
    if let Some(branch) = branch_name {
        force_delete_branch(repo, &branch)?;
    }

    Ok(())
}

/// `git branch -D <branch>` as the last step of a teardown, so the branch
/// can be created again later.
///
/// Runs with the lite env first, so a teardown doesn't wait on the clone's
/// shell-env capture. The delete fires the `reference-transaction` hook,
/// which can abort it, and a hook that needs a tool only the login shell's
/// `PATH` has fails to start under the lite env. So does a lite env with no
/// usable git. Any failure but the two below is tried again with the
/// captured env, and if that fails too the error is returned, rather than
/// reporting a teardown that left the branch behind as done.
///
/// A branch that is already gone, or checked out in another worktree, is
/// fine: git's answer is the same with any env, and a branch in use
/// elsewhere isn't this teardown's to delete.
pub(crate) fn force_delete_branch(repo: &Path, branch: &str) -> Result<(), GitError> {
    force_delete_branch_with(repo, branch, cli::run_lite, cli::run)
}

/// [`force_delete_branch`] with the lite-env and captured-env runners
/// passed in, so tests can stand in for the captured env.
fn force_delete_branch_with(
    repo: &Path,
    branch: &str,
    lite: impl FnOnce(&Path, &[&str]) -> Result<String, GitError>,
    captured: impl FnOnce(&Path, &[&str]) -> Result<String, GitError>,
) -> Result<(), GitError> {
    let args = ["branch", "-D", branch];
    let error = match lite(repo, &args) {
        Ok(_) => return Ok(()),
        Err(error) => error,
    };
    if let Some(settled) = settled_branch_delete(branch, error.to_string()) {
        return settled;
    }
    if matches!(
        error,
        GitError::NotARepo(_) | GitError::InvalidUtf8 | GitError::InvalidPath(_)
    ) {
        return Err(error);
    }
    log::info!(
        "Deleting branch '{branch}' in {} with the inherited env failed; retrying with the \
         captured env: {error}",
        repo.display()
    );
    match captured(repo, &args) {
        Ok(_) => Ok(()),
        Err(error) => settled_branch_delete(branch, error.to_string()).unwrap_or(Err(error)),
    }
}

/// `Ok` when a failed `branch -D` leaves nothing for the teardown to do: the
/// branch is already gone, or another worktree has it checked out. Matched
/// against the `LC_ALL=C` wording `cli` pins: newer git says "used by
/// worktree at", older git "checked out at".
fn settled_branch_delete(branch: &str, message: String) -> Option<Result<(), GitError>> {
    if message.contains(&format!("branch '{branch}' not found")) {
        return Some(Ok(()));
    }
    let lower = message.to_ascii_lowercase();
    if lower.contains("cannot delete branch")
        && (lower.contains("checked out at") || lower.contains("used by worktree at"))
    {
        log::info!("Leaving branch '{branch}', which another worktree has checked out: {message}");
        return Some(Ok(()));
    }
    None
}

/// Get the branch name associated with a worktree.
/// Returns None if the worktree doesn't exist or has no branch (detached HEAD).
fn get_worktree_branch(repo: &Path, worktree_path: &Path) -> Option<String> {
    let output = cli::run_lite_or_captured(repo, &["worktree", "list", "--porcelain"]).ok()?;
    find_worktree_branch(&output, worktree_path)
}

/// Find `worktree_path`'s branch in `git worktree list --porcelain` output.
///
/// git lists each worktree by its real path, so a worktree recorded as
/// `/tmp/…` is listed as `/private/tmp/…` on macOS. Both sides are compared
/// with symlinks resolved, and only as far as they exist on disk: git keeps
/// listing a worktree whose directory is already gone, and its branch still
/// needs deleting.
fn find_worktree_branch(porcelain: &str, worktree_path: &Path) -> Option<String> {
    let target = resolve_existing_prefix(worktree_path);
    let mut in_target_worktree = false;

    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            let path = Path::new(path);
            in_target_worktree = path == worktree_path || resolve_existing_prefix(path) == target;
        } else if in_target_worktree {
            if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                return Some(branch.to_string());
            }
        }
    }

    None
}

/// `path` with symlinks resolved over the longest prefix that exists, and the
/// missing remainder appended unchanged. Falls back to `path` as given when no
/// prefix resolves.
fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut prefix = path;
    loop {
        if let Ok(real) = prefix.canonicalize() {
            return missing.iter().rev().fold(real, |acc, name| acc.join(name));
        }
        match (prefix.parent(), prefix.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                prefix = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// List all worktrees for a repository.
/// Returns (path, branch_name) pairs.
pub fn list_worktrees(repo: &Path) -> Result<Vec<(PathBuf, Option<String>)>, GitError> {
    let output = cli::run(repo, &["worktree", "list", "--porcelain"])?;

    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in output.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            // Save previous worktree if any
            if let Some(path) = current_path.take() {
                worktrees.push((path, current_branch.take()));
            }
            current_path = Some(PathBuf::from(path_str));
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(branch.to_string());
        }
    }

    // Don't forget the last one
    if let Some(path) = current_path {
        worktrees.push((path, current_branch));
    }

    Ok(worktrees)
}

/// Get the current HEAD commit SHA for a worktree/repo.
///
/// Runs via `cli::run_smart`: `rev-parse HEAD` only reads refs, so it never
/// needs the captured shell env. `build_diff_spec` calls this for the
/// `worktree` scope and for `branch` scope without a commit sha, so on the
/// captured path opening the uncommitted-changes diff before the project's
/// warm-up finished blocked on the `$SHELL -ils` capture — the same stall
/// [`get_parent_commit`] hit for commit diffs. The mutating callers (update,
/// delete-commit, PR creation) are unaffected: the only behavioural difference
/// is a lite-env attempt before the captured env, as in
/// [`super::refs::merge_base`].
pub fn get_head_sha(worktree: &Path) -> Result<String, GitError> {
    let output = cli::run_smart(worktree, &["rev-parse", "HEAD"])?;
    Ok(output.trim().to_string())
}

/// `git log` format for every commit producer that feeds a
/// [`CommitTimelineItem`](crate::CommitTimelineItem).
///
/// Carries both clocks: `%ct` (committer time) is what a rebase rewrites, so
/// it answers "did the branch change since?"; `%at` (author time) is what a
/// rebase preserves, so it answers "when was this commit written" and is what
/// the branch timeline sorts on.
///
/// Fields are separated by `%x1f` (the unit separator) because no printable
/// delimiter is safe: git permits `|` in author names — and technically in
/// emails — and a delimiter inside a field shifts every field after it. The
/// subject still goes last so [`parse_branch_commit_line`] can take it as the
/// remainder — the same shape as `commit_reassociation`'s format.
pub const BRANCH_COMMIT_LOG_FORMAT: &str = "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%ct%x1f%at%x1f%s";

/// One [`BRANCH_COMMIT_LOG_FORMAT`] line, borrowed from the log output.
#[derive(Debug, Clone)]
pub struct BranchCommitFields<'a> {
    pub sha: &'a str,
    pub short_sha: &'a str,
    pub author: &'a str,
    pub author_email: &'a str,
    /// Committer time (`%ct`), in unix seconds. Rewritten by a rebase.
    pub committer_timestamp: i64,
    /// Author time (`%at`), in unix seconds. Preserved by a rebase.
    pub author_timestamp: i64,
    pub subject: &'a str,
}

/// Parse one [`BRANCH_COMMIT_LOG_FORMAT`] line. The subject is the remainder,
/// so even a subject containing the separator byte survives intact. Returns
/// `None` for a line that doesn't carry every field — a blank line, or output
/// from some other format.
pub fn parse_branch_commit_line(line: &str) -> Option<BranchCommitFields<'_>> {
    let mut parts = line.splitn(7, '\x1f');
    let sha = parts.next().filter(|sha| !sha.is_empty())?;
    let short_sha = parts.next()?;
    let author = parts.next()?;
    let author_email = parts.next()?;
    let committer_timestamp = parts.next()?;
    let author_timestamp = parts.next()?;
    let subject = parts.next()?;
    Some(BranchCommitFields {
        sha,
        short_sha,
        author,
        author_email,
        committer_timestamp: committer_timestamp.parse().unwrap_or(0),
        author_timestamp: author_timestamp.parse().unwrap_or(0),
        subject,
    })
}

/// Get commits on a branch since it diverged from base.
/// Returns commits in reverse chronological order (newest first).
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub author: String,
    pub author_email: String,
    /// Committer time (`%ct`), in unix seconds. Rewritten by a rebase.
    pub timestamp: i64,
    /// Author time (`%at`), in unix seconds. Preserved by a rebase.
    pub author_timestamp: i64,
    /// Position in git's topological order (0 = oldest on the branch).
    /// Used as a tiebreaker when multiple commits share the same second-level timestamp.
    pub order: i64,
}

/// Get commits between base and head.
/// Returns commits in reverse chronological order (newest first).
///
/// `merge_base` is the fork point from `base` — `merge-base <base> HEAD` —
/// as the caller already has it, so that only the branch's own commits are
/// returned even after a rebase or when the base ref (e.g. `origin/main`)
/// has moved forward, without a second `merge-base` spawn here. The branch
/// timeline takes it from the git-state build, which counts
/// `commitsSinceFork` from the same point. With `None` (no fork point: `base`
/// missing, or unrelated histories) the log runs from `base` itself, so a
/// missing base still fails here the way it always has.
///
/// Runs via `cli::run_smart` because this is on the foreground first-paint
/// path — `git log` is an env-independent read, so the lite path is correct
/// and avoids blocking on the per-project `$SHELL -ils` capture.
pub fn get_commits_since_base(
    worktree: &Path,
    base: &str,
    merge_base: Option<&str>,
) -> Result<Vec<CommitInfo>, GitError> {
    let range = format!("{}..HEAD", merge_base.unwrap_or(base));

    let output = cli::run_smart(worktree, &["log", BRANCH_COMMIT_LOG_FORMAT, &range])?;

    Ok(parse_commit_info_lines(&output))
}

/// Parse [`BRANCH_COMMIT_LOG_FORMAT`] lines, newest first as `git log` emits
/// them, assigning `order` so that 0 is the oldest commit on the branch.
fn parse_commit_info_lines(output: &str) -> Vec<CommitInfo> {
    let mut commits: Vec<CommitInfo> = output
        .lines()
        .filter_map(parse_branch_commit_line)
        .map(|fields| CommitInfo {
            sha: fields.sha.to_string(),
            short_sha: fields.short_sha.to_string(),
            subject: fields.subject.to_string(),
            author: fields.author.to_string(),
            author_email: fields.author_email.to_string(),
            timestamp: fields.committer_timestamp,
            author_timestamp: fields.author_timestamp,
            order: 0, // placeholder, assigned below
        })
        .collect();

    // git log returns newest-first; assign order so that 0 = oldest.
    let len = commits.len() as i64;
    for (i, commit) in commits.iter_mut().enumerate() {
        commit.order = len - 1 - i as i64;
    }

    commits
}

/// Get the full commit log (with bodies) between base and HEAD.
///
/// Returns a formatted string with commits listed oldest-first. Each
/// record is delimited by `\0` and contains a unix timestamp prefix
/// separated from the display text by `\x01`:
///
/// ```text
/// \0<unix_ts>\x01commit <sha>\nAuthor: …\n\n<message>\0…
/// ```
///
/// Use [`super::parse_timestamped_log`] (in `session_commands`) to
/// split this into `(timestamp, display_text)` pairs for timeline
/// interleaving. There is no `Date:` line in the display text: the
/// caller renders one from the prefix, in UTC like every other item.
///
/// Uses `merge-base` to find the actual fork point, consistent with
/// [`get_commits_since_base`].
///
/// Both inner calls run via `cli::run_smart`, for the same reason
/// [`get_commits_since_base`] does: `merge-base` and `git log` are
/// env-independent reads, so the lite env is correct. This is the read behind
/// every branch session's prompt context, and `worktree` is the session's
/// working directory — the worktree joined with the project subpath, a cache
/// key nothing else warms — so on the captured path it made every cold Start
/// wait out a `$SHELL -ils` capture that only the agent process needs.
pub fn get_full_commit_log(worktree: &Path, base: &str) -> Result<String, GitError> {
    let merge_base = super::refs::merge_base(worktree, base, "HEAD")?;
    let range = format!("{merge_base}..HEAD");

    // --reverse gives oldest-first ordering
    // %x00 = record separator, %x01 = field separator
    // %at = author time — the prefix orders the interleave and is also the
    // date the caller displays (the branch card shows author time too), and
    // author time is the one a rebase preserves, so rewritten commits keep
    // their place among the session messages they're interleaved with
    // %B is the full commit message (subject + body)
    let output = cli::run_smart(
        worktree,
        &[
            "log",
            "--reverse",
            "--format=%x00%at%x01commit %H%nAuthor: %an%n%n%B",
            &range,
        ],
    )?;

    Ok(output)
}

/// Check if a branch exists in the repository.
pub fn branch_exists(repo: &Path, branch_name: &str) -> Result<bool, GitError> {
    let result = cli::run(
        repo,
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch_name}"),
        ],
    );
    Ok(result.is_ok())
}

/// Set the upstream tracking branch for a local branch.
///
/// Runs `git branch --set-upstream-to=origin/<remote_branch> <local_branch>`.
/// Returns `Ok(true)` if the upstream was set, `Ok(false)` if the remote branch
/// doesn't exist (so there's nothing to track).
///
/// Git refuses `--set-upstream-to` when no `remote.origin.fetch` refspec maps
/// the branch — a `--single-branch` clone, for everything but its one branch
/// — even though `origin/<remote_branch>` exists. The tracking entry is then
/// written directly: `branch.<local>.remote` and `branch.<local>.merge`,
/// exactly what `git push -u` writes in that clone, and what
/// `upstream_branch_name` reads back. Nothing is added to the fetch refspecs:
/// a configured non-glob refspec outlives the PR and makes a bare `git fetch
/// origin` fail once the head is deleted.
pub fn set_upstream_to_origin(
    repo: &Path,
    local_branch: &str,
    remote_branch: &str,
) -> Result<bool, GitError> {
    if !remote_branch_exists(repo, remote_branch)? {
        return Ok(false);
    }
    let upstream = format!("origin/{remote_branch}");
    match cli::run(
        repo,
        &["branch", "--set-upstream-to", &upstream, local_branch],
    ) {
        Ok(_) => Ok(true),
        Err(GitError::CommandFailed(msg)) if msg.contains("is not a branch") => {
            log::debug!(
                "No fetch refspec maps origin/{remote_branch}; writing the tracking entry of \
                 '{local_branch}' directly"
            );
            cli::run(
                repo,
                &["config", &format!("branch.{local_branch}.remote"), "origin"],
            )?;
            cli::run(
                repo,
                &[
                    "config",
                    &format!("branch.{local_branch}.merge"),
                    &format!("refs/heads/{remote_branch}"),
                ],
            )?;
            Ok(true)
        }
        Err(e) => Err(e),
    }
}

/// Whether `local_branch` has any upstream configured (`branch.<name>.merge`
/// is set), on origin or elsewhere.
pub fn has_upstream(repo: &Path, local_branch: &str) -> Result<bool, GitError> {
    let key = format!("branch.{local_branch}.merge");
    match cli::run_lite(repo, &["config", "--get", &key]) {
        Ok(value) => Ok(!value.trim().is_empty()),
        // `--get` exits 1 for an unset key.
        Err(GitError::CommandFailed(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Make `local_branch` track `origin/<remote_branch>` when that remote branch
/// exists, and otherwise clear whatever upstream it has.
///
/// Run right after Staged creates a worktree. Git makes a branch created from
/// `origin/<base>` track `<base>`, and a branch that has never been pushed
/// should track nothing until `git push -u origin <local>:<remote>` records
/// the real target — `upstream_branch_name` reads this config as the push
/// target, so it must never be left saying "the base". Returns whether an
/// upstream was set.
pub fn set_upstream_to_origin_or_unset(
    repo: &Path,
    local_branch: &str,
    remote_branch: &str,
) -> Result<bool, GitError> {
    if set_upstream_to_origin(repo, local_branch, remote_branch)? {
        return Ok(true);
    }
    // Fails when there is no upstream to unset, which is the state we want.
    let _ = cli::run(repo, &["branch", "--unset-upstream", local_branch]);
    Ok(false)
}

/// Fetch `origin/<branch_name>` so that it can be tracked and compared.
///
/// Fetches by explicit refspec, which updates the remote-tracking ref whether
/// or not `remote.origin.fetch` covers the branch (a `--single-branch` clone's
/// does not; a bare `git fetch origin <branch>` there only updates
/// `FETCH_HEAD`). The configured refspecs are left alone.
pub fn fetch_origin_branch(repo: &Path, branch_name: &str) -> Result<(), GitError> {
    let refspec = format!("+refs/heads/{branch_name}:refs/remotes/origin/{branch_name}");
    cli::run(repo, &["fetch", "origin", &refspec])?;
    Ok(())
}

/// Check whether a remote tracking branch (`origin/<branch_name>`) exists.
pub fn remote_branch_exists(repo: &Path, branch_name: &str) -> Result<bool, GitError> {
    let ref_name = format!("refs/remotes/origin/{branch_name}");
    let result = cli::run(repo, &["rev-parse", "--verify", &ref_name]);
    Ok(result.is_ok())
}

/// Fetch a PR's head ref and return the resulting commit SHA.
/// Works for fork PRs because GitHub exposes `refs/pull/<N>/head` regardless
/// of whether the head branch lives in the same repo or a fork.
///
/// Fetches into a named local ref (`refs/staged/pr/<N>`) instead of relying on
/// `FETCH_HEAD`, which makes this safe for concurrent use across different PRs.
/// The local ref is intentionally kept after use — refs are tiny and harmless
/// to accumulate, and deleting them would race with concurrent callers fetching
/// the same PR number.
pub fn fetch_pr_head_sha(repo: &Path, pr_number: u64) -> Result<String, GitError> {
    let pr_ref = format!("refs/pull/{pr_number}/head");
    let local_ref = format!("refs/staged/pr/{pr_number}");
    let refspec = format!("+{pr_ref}:{local_ref}");
    cli::run(repo, &["fetch", "origin", &refspec])?;
    let sha = cli::run(repo, &["rev-parse", &local_ref])?
        .trim()
        .to_string();
    Ok(sha)
}

/// Reset HEAD to a specific commit (hard reset).
/// This discards all commits after the specified commit.
pub fn reset_to_commit(worktree: &Path, commit_sha: &str) -> Result<(), GitError> {
    cli::run(worktree, &["reset", "--hard", commit_sha])?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeChangePaths {
    pub revert_paths: Vec<String>,
    pub remove_paths: Vec<String>,
    pub conflicted_paths: Vec<String>,
    pub reset_required: bool,
}

impl WorktreeChangePaths {
    pub fn is_empty(&self) -> bool {
        self.revert_paths.is_empty()
            && self.remove_paths.is_empty()
            && self.conflicted_paths.is_empty()
    }
}

use super::status_parse::is_conflicted_status;

fn literal_pathspec(path: &str) -> String {
    format!(":(literal){path}")
}

/// Parse `git status --porcelain=1 -z --untracked-files=all` into paths that
/// will be reverted by reset and paths that must be removed with clean.
pub fn parse_worktree_status_paths(output: &str) -> WorktreeChangePaths {
    let mut revert_paths = Vec::new();
    let mut remove_paths = Vec::new();
    let mut conflicted_paths = Vec::new();
    let mut reset_required = false;

    let mut parts = output.split('\0').filter(|part| !part.is_empty());
    while let Some(record) = parts.next() {
        let mut chars = record.chars();
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');
        let path = record.get(3..).unwrap_or("").to_string();
        if path.is_empty() {
            continue;
        }

        let original_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            parts.next().map(str::to_string)
        } else {
            None
        };

        if x == '?' && y == '?' {
            remove_paths.push(path);
            continue;
        }

        if is_conflicted_status(x, y) {
            conflicted_paths.push(path);
            continue;
        }

        reset_required = true;
        if matches!(x, 'A' | 'C') || matches!(y, 'A' | 'C') {
            remove_paths.push(path);
        } else if let Some(original) = original_path {
            // Display-only: this arrow format is used for the WorktreeChangesPreview
            // shown to the user. It is never passed back to git commands as a pathspec.
            revert_paths.push(format!("{original} -> {path}"));
        } else {
            revert_paths.push(path);
        }
    }

    revert_paths.sort();
    remove_paths.sort();
    conflicted_paths.sort();

    WorktreeChangePaths {
        revert_paths,
        remove_paths,
        conflicted_paths,
        reset_required,
    }
}

pub fn list_worktree_change_paths(worktree: &Path) -> Result<WorktreeChangePaths, GitError> {
    let output = cli::run(
        worktree,
        &["status", "--porcelain=1", "-z", "--untracked-files=all"],
    )?;
    Ok(parse_worktree_status_paths(&output))
}

pub fn discard_worktree_changes(
    worktree: &Path,
    changes: &WorktreeChangePaths,
) -> Result<(), GitError> {
    if changes.reset_required {
        cli::run(worktree, &["reset", "--hard", "HEAD"])?;
    }

    if !changes.remove_paths.is_empty() {
        let pathspecs = changes
            .remove_paths
            .iter()
            .map(|path| literal_pathspec(path))
            .collect::<Vec<_>>();
        let mut args = vec!["clean", "-fd", "--"];
        args.extend(pathspecs.iter().map(String::as_str));
        cli::run(worktree, &args)?;
    }

    Ok(())
}

/// Get the parent commit SHA of a given commit.
/// Returns None if the commit has no parent (initial commit).
///
/// Runs via `cli::run_smart`: `rev-parse <sha>^` only reads refs and objects,
/// so it never needs the captured shell env. That makes the fast path safe for
/// every caller, not just the foreground one. `build_diff_spec` uses it for
/// commit diffs, where blocking on the `$SHELL -ils` capture was the stall;
/// the delete-commit flow in `timeline.rs` uses it before `reset_to_commit`,
/// which still goes through `cli::run` and so still waits for the captured
/// env. The only behavioural difference from `run` is a lite-env attempt
/// before the captured fallback, as in [`super::refs::merge_base`] and
/// [`get_head_sha`].
pub fn get_parent_commit(worktree: &Path, commit_sha: &str) -> Result<Option<String>, GitError> {
    let result = cli::run_smart(worktree, &["rev-parse", &format!("{commit_sha}^")]);
    match result {
        Ok(output) => Ok(Some(output.trim().to_string())),
        Err(_) => Ok(None), // No parent (initial commit or invalid)
    }
}

/// Create a worktree from a GitHub PR.
///
/// This fetches the PR's head ref and creates a local branch + worktree at that commit.
/// The branch name will be the PR's head_ref (e.g., "feature-x").
///
/// Returns (worktree_path, branch_name, base_branch) where base_branch is the PR's target.
pub fn create_worktree_from_pr(
    repo: &Path,
    pr_number: u64,
    head_ref: &str,
    base_ref: &str,
) -> Result<(PathBuf, String, String), GitError> {
    let worktree_path = worktree_path_for(repo, head_ref)?;
    create_worktree_from_pr_at_path(repo, pr_number, head_ref, base_ref, &worktree_path)
}

/// Create a worktree from a GitHub PR at an explicit path.
pub fn create_worktree_from_pr_at_path(
    repo: &Path,
    pr_number: u64,
    head_ref: &str,
    base_ref: &str,
    worktree_path: &Path,
) -> Result<(PathBuf, String, String), GitError> {
    // Use the PR's head_ref as the local branch name
    let branch_name = head_ref.to_string();

    // Check if branch already exists locally
    if branch_exists(repo, &branch_name)? {
        return Err(GitError::CommandFailed(format!(
            "Branch '{branch_name}' already exists locally"
        )));
    }

    ensure_worktree_parent_exists(worktree_path)?;
    ensure_worktree_absent(worktree_path)?;

    // Fetch the PR head ref into a named local ref (safe for concurrent use)
    let head_sha = fetch_pr_head_sha(repo, pr_number)?;

    let worktree_str = worktree_path
        .to_str()
        .ok_or_else(|| GitError::InvalidPath(worktree_path.display().to_string()))?;

    // Create worktree with new branch at the PR's head commit
    // git worktree add <path> -b <branch> <commit>
    cli::run(
        repo,
        &[
            "worktree",
            "add",
            worktree_str,
            "-b",
            &branch_name,
            &head_sha,
        ],
    )?;

    // The base branch for diffs should be the PR's target (e.g., "origin/main")
    let base_branch = format!("origin/{base_ref}");

    Ok((worktree_path.to_path_buf(), branch_name, base_branch))
}

/// Result of updating a branch from a PR.
#[derive(Debug)]
pub struct UpdateFromPrResult {
    /// The commit SHA before the update
    pub old_sha: String,
    /// The commit SHA after the update (new PR head)
    pub new_sha: String,
    /// Number of new commits pulled in
    pub commits_added: usize,
}

/// Update a local branch's worktree to match the latest PR head.
///
/// This fetches the latest PR head and fast-forwards (or resets) the local branch
/// to match. Works for both clean fast-forwards and force-pushed PRs.
///
/// **Warning**: This will discard any local uncommitted changes and any local
/// commits that are not in the PR. Use with caution.
///
/// Returns information about what changed.
pub fn update_branch_from_pr(
    worktree: &Path,
    pr_number: u64,
) -> Result<UpdateFromPrResult, GitError> {
    // Get the current HEAD before update
    let old_sha = get_head_sha(worktree)?;

    // Fetch the PR head ref into a named local ref (safe for concurrent use)
    let new_sha = fetch_pr_head_sha(worktree, pr_number)?;

    // If already up to date, return early
    if old_sha == new_sha {
        return Ok(UpdateFromPrResult {
            old_sha,
            new_sha,
            commits_added: 0,
        });
    }

    // Check if this is a fast-forward (new_sha is descendant of old_sha)
    let is_fast_forward = cli::run(
        worktree,
        &["merge-base", "--is-ancestor", &old_sha, &new_sha],
    )
    .is_ok();

    if is_fast_forward {
        // Fast-forward: just move HEAD to the new commit
        cli::run(worktree, &["merge", "--ff-only", &new_sha])?;
    } else {
        // Not a fast-forward (PR was force-pushed or rebased)
        // Hard reset to the new PR head
        cli::run(worktree, &["reset", "--hard", &new_sha])?;
    }

    // Count how many commits were added
    // This counts commits between old and new (may be negative for force-push, but we report 0)
    let commits_added = if is_fast_forward {
        let log_output = cli::run(
            worktree,
            &["log", "--oneline", &format!("{old_sha}..{new_sha}")],
        )?;
        log_output.lines().count()
    } else {
        // For force-push/rebase, just count commits from merge-base to new
        let merge_base = cli::run(worktree, &["merge-base", &old_sha, &new_sha])
            .unwrap_or_default()
            .trim()
            .to_string();
        if merge_base.is_empty() {
            0
        } else {
            let log_output = cli::run(
                worktree,
                &["log", "--oneline", &format!("{merge_base}..{new_sha}")],
            )?;
            log_output.lines().count()
        }
    };

    Ok(UpdateFromPrResult {
        old_sha,
        new_sha,
        commits_added,
    })
}

/// Switch a worktree to a different branch.
///
/// This checks out the specified branch in the worktree.
/// The branch must already exist (locally or as a remote tracking branch).
///
/// Returns an error if the checkout fails (e.g., uncommitted changes, branch doesn't exist).
pub fn switch_branch(worktree: &Path, branch_name: &str) -> Result<(), GitError> {
    cli::run(worktree, &["checkout", branch_name])?;
    Ok(())
}

/// Check if the local branch has commits not yet pushed to the remote.
///
/// Compares the local HEAD with `origin/<remote_branch>` — the branch this
/// checkout pushes to, which is the PR head rather than the local name for a
/// de-duplicated PR import. Returns `true` if there are commits in the local
/// branch that are not in the remote tracking branch, and `true` if the
/// remote tracking branch doesn't exist (e.g., never pushed).
///
/// Runs via `cli::run_bounded`: both reads only walk refs and commits, and the
/// home page runs this eagerly for the merged branches of every visible
/// project to decide whether deleting it needs a confirmation, so it shouldn't
/// queue behind a cold capture of the worktree's shell env.
pub fn has_unpushed_commits(worktree: &Path, remote_branch: &str) -> Result<bool, GitError> {
    let remote_ref = format!("origin/{remote_branch}");
    // Check that the remote ref exists first
    if cli::run_bounded(worktree, &["rev-parse", "--verify", &remote_ref]).is_err() {
        // Remote tracking branch doesn't exist — all local commits are unpushed
        return Ok(true);
    }
    let output = cli::run_bounded(worktree, &["rev-list", &format!("{remote_ref}..HEAD")])?;
    Ok(!output.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_clocks_and_orders_from_the_oldest_commit() {
        let commits = parse_commit_info_lines(concat!(
            "def456\x1fdef456a\x1fTest\x1ftest@example.com\x1f9200\x1f1200\x1ffix: lexer\n",
            "abc123\x1fabc123a\x1fTest\x1ftest@example.com\x1f9100\x1f1100\x1ffeat: parser\n",
        ));

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "fix: lexer");
        assert_eq!(commits[0].timestamp, 9200);
        assert_eq!(commits[0].author_timestamp, 1200);
        assert_eq!(commits[1].subject, "feat: parser");
        assert_eq!(commits[1].timestamp, 9100);
        assert_eq!(commits[1].author_timestamp, 1100);
        // git log is newest-first, so the last line is the oldest commit.
        assert_eq!(commits[0].order, 1);
        assert_eq!(commits[1].order, 0);
    }

    /// git permits `|` in `user.name` (and technically in emails) and it's
    /// common in subjects, which is why the separator is `%x1f` — none of
    /// these shift the fields after them.
    #[test]
    fn keeps_pipes_in_names_emails_and_subjects_intact() {
        let commits = parse_commit_info_lines(
            "abc123\x1fabc123a\x1fFoo | Bar\x1fa|b@example.com\x1f9100\x1f1100\x1ffeat: parse a|b unions\n",
        );

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].author, "Foo | Bar");
        assert_eq!(commits[0].author_email, "a|b@example.com");
        assert_eq!(commits[0].subject, "feat: parse a|b unions");
        assert_eq!(commits[0].timestamp, 9100);
        assert_eq!(commits[0].author_timestamp, 1100);
    }

    /// The subject is the trailing field so even a separator byte in it can't
    /// shift the timestamps out from under the parse.
    #[test]
    fn keeps_a_subject_containing_the_separator_intact() {
        let commits = parse_commit_info_lines(
            "abc123\x1fabc123a\x1fTest\x1ftest@example.com\x1f9100\x1f1100\x1ffeat: a\x1fb\n",
        );

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].subject, "feat: a\x1fb");
        assert_eq!(commits[0].timestamp, 9100);
        assert_eq!(commits[0].author_timestamp, 1100);
    }

    #[test]
    fn skips_lines_that_are_missing_fields() {
        assert!(parse_branch_commit_line("").is_none());
        assert!(
            parse_branch_commit_line("abc123\x1fabc123a\x1fTest\x1ftest@example.com\x1f9100")
                .is_none()
        );
        // A record with no SHA isn't a commit.
        assert!(parse_branch_commit_line(
            "\x1fabc123a\x1fTest\x1ftest@example.com\x1f9100\x1f1100\x1fs"
        )
        .is_none());
    }

    #[test]
    fn test_worktree_path_sanitization() {
        let repo = Path::new("/Users/test/myrepo");
        let path = worktree_path_for(repo, "feature/auth-flow").unwrap();

        // Should sanitize slashes
        assert!(path.to_string_lossy().contains("feature-auth-flow"));
        assert!(!path.to_string_lossy().contains("feature/auth-flow"));
    }

    #[test]
    fn test_project_worktree_path_groups_by_project_then_repo() {
        let path =
            project_worktree_path_for("project-123", "squareup/builderbot", "feature/auth-flow")
                .unwrap();

        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap();
        let project_dir = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap();
        let projects_root = path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap();

        assert_eq!(projects_root, "projects");
        assert_eq!(project_dir, "project-123");
        assert_eq!(file_name, "squareup-builderbot--feature-auth-flow");
    }

    #[test]
    fn test_project_worktree_path_sanitizes_repo_slug() {
        let path = project_worktree_path_for("project-123", "///", "branch").unwrap();

        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap();
        assert_eq!(file_name, "repo--branch");
    }

    #[test]
    fn test_project_worktree_root_for() {
        let root = project_worktree_root_for("project-123").unwrap();

        let project_dir = root.file_name().and_then(|s| s.to_str()).unwrap();
        let projects_root = root
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap();

        assert_eq!(projects_root, "projects");
        assert_eq!(project_dir, "project-123");
    }

    #[test]
    fn parses_worktree_status_paths_for_preview_and_discard() {
        let status = " M tracked.txt\0A  staged-new.txt\0?? untracked.txt\0UU conflicted.txt\0";

        let paths = parse_worktree_status_paths(status);

        assert_eq!(paths.revert_paths, vec!["tracked.txt"]);
        assert_eq!(paths.remove_paths, vec!["staged-new.txt", "untracked.txt"]);
        assert_eq!(paths.conflicted_paths, vec!["conflicted.txt"]);
        assert!(paths.reset_required);
    }

    /// The reason this goes through `git worktree move`: after the move the
    /// worktree still has to be a working tree at its new path, which requires
    /// both the gitfile and the repo's `gitdir` pointer to have been rewritten.
    #[test]
    fn move_worktree_leaves_a_working_tree_at_the_new_path() {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("tracked.txt", "base\n");
        repo.commit("base");

        repo.run_git(&["branch", "feature"]);

        let name = repo.path().file_name().unwrap().to_str().unwrap();
        let parent = repo.path().parent().unwrap();
        let from = parent.join(format!("{name}-wt-from"));
        let to = parent.join(format!("{name}-wt-to"));
        create_worktree_for_existing_branch_at_path(repo.path(), "feature", &from).unwrap();

        move_worktree(repo.path(), &from, &to).unwrap();

        assert!(!from.exists());
        assert!(to.join("tracked.txt").exists());
        // A dangling gitfile or gitdir pointer fails both of these.
        assert!(cli::run(&to, &["status", "--porcelain"]).is_ok());
        assert_eq!(
            cli::run(&to, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap()
                .trim(),
            "feature"
        );

        let _ = std::fs::remove_dir_all(&to);
    }

    fn local_branches(repo: &crate::test_utils::TempGitRepo) -> String {
        repo.run_git(&["branch", "--list", "--format=%(refname:short)"])
    }

    #[test]
    fn remove_worktree_deletes_the_worktree_and_its_branch() {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("tracked.txt", "base\n");
        repo.commit("base");
        repo.run_git(&["branch", "feature"]);

        let root = crate::test_utils::TempDir::new("remove-worktree");
        let worktree = root.path().join("wt");
        create_worktree_for_existing_branch_at_path(repo.path(), "feature", &worktree).unwrap();
        std::fs::write(worktree.join("untracked.txt"), "scratch\n").unwrap();

        remove_worktree(repo.path(), &worktree).unwrap();

        assert!(!worktree.exists());
        assert_eq!(local_branches(&repo).trim(), "main");
    }

    /// Point `repo` at a `reference-transaction` hook whose interpreter,
    /// `staged-test-hook-interp`, is on no `PATH` but the one a test adds,
    /// as a hook that needs a tool only the login shell's `PATH` has. Returns
    /// a dir holding that interpreter, for the test's captured env.
    #[cfg(unix)]
    fn hook_needing_captured_path(
        repo: &crate::test_utils::TempGitRepo,
        root: &Path,
    ) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let write_executable = |path: &Path, content: &str| {
            std::fs::write(path, content).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let hooks = root.join("hooks");
        let bin = root.join("bin");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        write_executable(
            &hooks.join("reference-transaction"),
            "#!/usr/bin/env staged-test-hook-interp\n",
        );
        write_executable(
            &bin.join("staged-test-hook-interp"),
            "#!/bin/sh\ncat >/dev/null\nexit 0\n",
        );
        repo.run_git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
        bin
    }

    /// `branch -D` in `repo` with `bin` ahead of the inherited `PATH`, as the
    /// captured env would run it.
    #[cfg(unix)]
    fn run_with_path(bin: &Path, repo: &Path, args: &[&str]) -> Result<String, GitError> {
        let mut command = std::process::Command::new("git");
        command.arg("-C").arg(repo).args(args);
        crate::git::strip_git_env(&mut command);
        let path = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", format!("{}:{path}", bin.display()));
        command.env("LC_ALL", "C");
        let output = command.output().unwrap();
        if output.status.success() {
            Ok(String::from_utf8(output.stdout).unwrap())
        } else {
            Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ))
        }
    }

    /// A `reference-transaction` hook that can't start under the inherited
    /// env aborts the branch delete, and the teardown says so instead of
    /// reporting success with the branch left behind. The worktree itself
    /// is still removed with the lite env.
    #[cfg(unix)]
    #[test]
    fn remove_worktree_fails_when_a_hook_aborts_the_branch_delete() {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("tracked.txt", "base\n");
        repo.commit("base");
        repo.run_git(&["branch", "feature"]);

        let root = crate::test_utils::TempDir::new("remove-worktree-hook");
        let worktree = root.path().join("wt");
        create_worktree_for_existing_branch_at_path(repo.path(), "feature", &worktree).unwrap();
        hook_needing_captured_path(&repo, root.path());

        // The test build's captured env is the lite one, so the retry fails
        // the same way.
        let error = remove_worktree(repo.path(), &worktree)
            .expect_err("the hook aborted the branch delete");
        assert!(
            error.to_string().contains("reference-transaction"),
            "{error}"
        );
        assert!(!worktree.exists());
        assert!(local_branches(&repo).lines().any(|b| b == "feature"));
    }

    /// A delete the lite env's hook aborted is tried again with the captured
    /// env, which has what the hook needs.
    #[cfg(unix)]
    #[test]
    fn force_delete_branch_retries_a_hook_failure_with_the_captured_env() {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("tracked.txt", "base\n");
        repo.commit("base");
        repo.run_git(&["branch", "feature"]);
        let root = crate::test_utils::TempDir::new("force-delete-branch-hook");
        let bin = hook_needing_captured_path(&repo, root.path());

        let lite_error = cli::run_lite(repo.path(), &["branch", "-D", "feature"])
            .expect_err("the hook can't start under the inherited env");
        assert!(
            settled_branch_delete("feature", lite_error.to_string()).is_none(),
            "{lite_error}"
        );

        let retried = std::cell::Cell::new(false);
        force_delete_branch_with(repo.path(), "feature", cli::run_lite, |repo, args| {
            retried.set(true);
            run_with_path(&bin, repo, args)
        })
        .unwrap();
        assert!(retried.get());
        assert_eq!(local_branches(&repo).trim(), "main");
    }

    /// A branch that is already gone, or checked out in another worktree, is
    /// git's answer with any env: the delete succeeds without waiting on the
    /// captured env, and leaves a branch in use alone.
    #[test]
    fn force_delete_branch_settles_routine_failures_without_the_captured_env() {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("tracked.txt", "base\n");
        repo.commit("base");
        repo.run_git(&["branch", "feature"]);
        let root = crate::test_utils::TempDir::new("force-delete-branch-routine");
        let worktree = root.path().join("wt");
        create_worktree_for_existing_branch_at_path(repo.path(), "feature", &worktree).unwrap();

        let no_retry = |_: &Path, args: &[&str]| -> Result<String, GitError> {
            panic!("retried {args:?} with the captured env")
        };
        force_delete_branch_with(repo.path(), "feature", cli::run_lite, no_retry).unwrap();
        assert!(local_branches(&repo).lines().any(|b| b == "feature"));
        force_delete_branch_with(repo.path(), "no-such-branch", cli::run_lite, no_retry).unwrap();

        assert!(settled_branch_delete(
            "feature",
            "error: Cannot delete branch 'feature' checked out at '/tmp/wt'\n".into()
        )
        .is_some());
        assert!(settled_branch_delete(
            "feature",
            "fatal: in 'preparing' phase, update aborted by the reference-transaction hook\n"
                .into()
        )
        .is_none());
    }

    /// git records a worktree by its real path, so a worktree created through
    /// a symlinked directory (`/tmp` on macOS) is listed under a different
    /// string than the one stored for it. The branch must still be found and
    /// deleted, or it blocks recreating a branch of the same name.
    #[cfg(unix)]
    #[test]
    fn remove_worktree_deletes_the_branch_of_a_worktree_recorded_through_a_symlink() {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("tracked.txt", "base\n");
        repo.commit("base");
        repo.run_git(&["branch", "feature"]);

        let root = crate::test_utils::TempDir::new("remove-worktree-symlink");
        std::fs::create_dir(root.path().join("real")).unwrap();
        std::os::unix::fs::symlink(root.path().join("real"), root.path().join("link")).unwrap();
        let worktree = root.path().join("link").join("wt");
        create_worktree_for_existing_branch_at_path(repo.path(), "feature", &worktree).unwrap();

        remove_worktree(repo.path(), &worktree).unwrap();

        assert!(!root.path().join("real").join("wt").exists());
        assert_eq!(local_branches(&repo).trim(), "main");
    }

    /// git keeps listing a worktree whose directory is gone, under the real
    /// path it recorded. Resolving only the part of the stored path that still
    /// exists has to land on that same path.
    #[cfg(unix)]
    #[test]
    fn remove_worktree_deletes_the_branch_of_an_already_deleted_worktree_behind_a_symlink() {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("tracked.txt", "base\n");
        repo.commit("base");
        repo.run_git(&["branch", "feature"]);

        let root = crate::test_utils::TempDir::new("remove-worktree-gone");
        std::fs::create_dir(root.path().join("real")).unwrap();
        std::os::unix::fs::symlink(root.path().join("real"), root.path().join("link")).unwrap();
        let worktree = root.path().join("link").join("wt");
        create_worktree_for_existing_branch_at_path(repo.path(), "feature", &worktree).unwrap();
        std::fs::remove_dir_all(root.path().join("real").join("wt")).unwrap();

        remove_worktree(repo.path(), &worktree).unwrap();

        assert_eq!(local_branches(&repo).trim(), "main");
        assert_eq!(
            repo.run_git(&["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1
        );
    }

    #[test]
    fn discard_worktree_changes_resets_tracked_and_removes_new_files() {
        let repo = crate::test_utils::TempGitRepo::new();
        repo.write_file("tracked.txt", "base\n");
        repo.commit("base");

        repo.write_file("tracked.txt", "modified\n");
        repo.write_file("staged-new.txt", "new\n");
        repo.run_git(&["add", "staged-new.txt"]);
        repo.write_file("untracked.txt", "untracked\n");

        let changes = list_worktree_change_paths(repo.path()).unwrap();
        discard_worktree_changes(repo.path(), &changes).unwrap();

        assert_eq!(
            std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
            "base\n"
        );
        assert!(!repo.path().join("staged-new.txt").exists());
        assert!(!repo.path().join("untracked.txt").exists());
        assert!(repo.run_git(&["status", "--porcelain"]).trim().is_empty());
    }
}
