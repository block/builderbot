use super::cli::{self, GitError};
use super::refs::{branch_name_without_origin, origin_ref_for_branch};
use super::status_parse::is_conflicted_status;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const FETCH_TTL_MS: i64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchMode {
    Ttl,
    Force,
    Never,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BranchGitState {
    pub head_sha: Option<String>,
    pub current_branch: Option<String>,
    pub detached_head: bool,
    pub expected_branch_matches: bool,
    pub upstream: UpstreamGitState,
    pub base: BaseGitState,
    pub worktree: WorktreeGitState,
    pub fetch: FetchGitState,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamGitState {
    pub r#ref: String,
    pub exists: bool,
    pub sha: Option<String>,
    pub relation: UpstreamRelation,
    pub ahead: u32,
    pub behind: u32,
    pub merge_base_sha: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UpstreamRelation {
    Missing,
    InSync,
    LocalAhead,
    OriginAhead,
    Diverged,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BaseGitState {
    pub r#ref: String,
    pub sha: Option<String>,
    pub commits_since_fork: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeGitState {
    pub dirty: bool,
    pub modified: u32,
    pub added: u32,
    pub deleted: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FetchGitState {
    pub status: FetchStatus,
    pub fetched_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FetchStatus {
    Fresh,
    Stale,
    Failed,
}

/// Fast (local-only) git state — no fetch, no ref comparisons.
/// Used for the fast stream of the two-stream timeline split.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FastGitState {
    pub head_sha: Option<String>,
    pub current_branch: Option<String>,
    pub detached_head: bool,
    pub expected_branch_matches: bool,
    pub worktree: WorktreeGitState,
}

impl FastGitState {
    /// Convert to a full BranchGitState with placeholder upstream/base/fetch fields.
    /// Used to build the partial timeline before the slow stream (fetch + refs) completes.
    pub fn into_placeholder_git_state(
        self,
        branch_name: &str,
        base_branch: &str,
    ) -> BranchGitState {
        let upstream_ref = origin_ref_for_branch(branch_name);
        let base_ref = origin_ref_for_branch(base_branch);
        BranchGitState {
            head_sha: self.head_sha,
            current_branch: self.current_branch,
            detached_head: self.detached_head,
            expected_branch_matches: self.expected_branch_matches,
            worktree: self.worktree,
            upstream: UpstreamGitState {
                r#ref: upstream_ref,
                exists: false,
                sha: None,
                relation: UpstreamRelation::Missing,
                ahead: 0,
                behind: 0,
                merge_base_sha: None,
            },
            base: BaseGitState {
                r#ref: base_ref,
                sha: None,
                commits_since_fork: 0,
            },
            fetch: FetchGitState {
                status: FetchStatus::Stale,
                fetched_at: None,
                error: None,
            },
        }
    }
}

#[derive(Debug, Clone)]
struct FetchCacheEntry {
    fetched_at: i64,
    upstream_known_missing: bool,
}

#[derive(Debug, Clone)]
struct RefreshOutcome {
    fetch: FetchGitState,
    upstream_known_missing: bool,
}

fn fetch_cache() -> &'static Mutex<HashMap<String, FetchCacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, FetchCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn trim_non_empty(output: String) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn is_missing_remote_ref(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("couldn't find remote ref") || lower.contains("could not find remote ref")
}

fn refspec_for(remote_branch: &str) -> String {
    let branch = branch_name_without_origin(remote_branch);
    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")
}

fn refresh_refs_if_needed<F>(
    cache_key: &str,
    run_git: &F,
    branch_name: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
) -> RefreshOutcome
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    let now = now_ms();
    let previous = fetch_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(cache_key).cloned());

    let should_fetch = match (fetch_mode, &previous) {
        (FetchMode::Never, _) => false,
        (FetchMode::Force, _) => true,
        (FetchMode::Ttl, Some(entry)) => now.saturating_sub(entry.fetched_at) > FETCH_TTL_MS,
        (FetchMode::Ttl, None) => true,
    };

    if !should_fetch {
        let fetched_at = previous.as_ref().map(|entry| entry.fetched_at);
        return RefreshOutcome {
            fetch: FetchGitState {
                status: if fetched_at.is_some() {
                    FetchStatus::Fresh
                } else {
                    FetchStatus::Stale
                },
                fetched_at,
                error: None,
            },
            upstream_known_missing: previous
                .as_ref()
                .map(|entry| entry.upstream_known_missing)
                .unwrap_or(false),
        };
    }

    let base_refspec = refspec_for(base_branch);
    let branch_refspec = refspec_for(branch_name);
    let mut upstream_known_missing = false;

    // Fetch both refspecs in a single network call when they differ.
    if branch_refspec != base_refspec {
        match run_git(&[
            "fetch",
            "--prune",
            "origin",
            base_refspec.as_str(),
            branch_refspec.as_str(),
        ]) {
            Err(error) if is_missing_remote_ref(&error) => {
                // The branch refspec is missing on the remote. Re-fetch with
                // just the base refspec so we still get base branch updates.
                if let Err(base_err) =
                    run_git(&["fetch", "--prune", "origin", base_refspec.as_str()])
                {
                    return RefreshOutcome {
                        fetch: FetchGitState {
                            status: FetchStatus::Failed,
                            fetched_at: previous.map(|entry| entry.fetched_at),
                            error: Some(base_err.trim().to_string()),
                        },
                        upstream_known_missing: false,
                    };
                }
                upstream_known_missing = true;
            }
            Err(error) => {
                return RefreshOutcome {
                    fetch: FetchGitState {
                        status: FetchStatus::Failed,
                        fetched_at: previous.map(|entry| entry.fetched_at),
                        error: Some(error.trim().to_string()),
                    },
                    upstream_known_missing: false,
                };
            }
            Ok(_) => {}
        }
    } else if let Err(error) = run_git(&["fetch", "--prune", "origin", base_refspec.as_str()]) {
        return RefreshOutcome {
            fetch: FetchGitState {
                status: FetchStatus::Failed,
                fetched_at: previous.map(|entry| entry.fetched_at),
                error: Some(error.trim().to_string()),
            },
            upstream_known_missing: false,
        };
    }

    if let Ok(mut cache) = fetch_cache().lock() {
        cache.insert(
            cache_key.to_string(),
            FetchCacheEntry {
                fetched_at: now,
                upstream_known_missing,
            },
        );
    }

    RefreshOutcome {
        fetch: FetchGitState {
            status: FetchStatus::Fresh,
            fetched_at: Some(now),
            error: None,
        },
        upstream_known_missing,
    }
}

fn parse_u32(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.parse::<u32>().ok()).unwrap_or(0)
}

fn rev_count<F>(run_git: &F, range: &str) -> u32
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    run_git(&["rev-list", "--count", range])
        .ok()
        .and_then(trim_non_empty)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}

fn current_branch<F>(run_git: &F) -> Option<String>
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    run_git(&["branch", "--show-current"])
        .ok()
        .and_then(trim_non_empty)
}

fn resolve_ref<F>(run_git: &F, reference: &str) -> Option<String>
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    run_git(&["rev-parse", "--verify", reference])
        .ok()
        .and_then(trim_non_empty)
}

fn merge_base<F>(run_git: &F, left: &str, right: &str) -> Option<String>
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    run_git(&["merge-base", left, right])
        .ok()
        .and_then(trim_non_empty)
}

fn compute_upstream_state<F>(
    run_git: &F,
    upstream_ref: String,
    head_sha: Option<&str>,
    upstream_known_missing: bool,
) -> UpstreamGitState
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    let sha = if upstream_known_missing {
        None
    } else {
        resolve_ref(run_git, &upstream_ref)
    };
    let exists = sha.is_some();

    if !exists || head_sha.is_none() {
        return UpstreamGitState {
            r#ref: upstream_ref,
            exists,
            sha,
            relation: UpstreamRelation::Missing,
            ahead: 0,
            behind: 0,
            merge_base_sha: None,
        };
    }

    let counts_range = format!("HEAD...{upstream_ref}");
    let (ahead, behind) = run_git(&["rev-list", "--left-right", "--count", counts_range.as_str()])
        .ok()
        .map(|output| {
            let mut parts = output.split_whitespace();
            (parse_u32(parts.next()), parse_u32(parts.next()))
        })
        .unwrap_or((0, 0));
    let relation = match (ahead, behind) {
        (0, 0) => UpstreamRelation::InSync,
        (_, 0) => UpstreamRelation::LocalAhead,
        (0, _) => UpstreamRelation::OriginAhead,
        _ => UpstreamRelation::Diverged,
    };
    let merge_base_sha = merge_base(run_git, "HEAD", &upstream_ref);

    UpstreamGitState {
        r#ref: upstream_ref,
        exists,
        sha,
        relation,
        ahead,
        behind,
        merge_base_sha,
    }
}

fn compute_base_state<F>(run_git: &F, base_ref: String, head_sha: Option<&str>) -> BaseGitState
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    let sha = resolve_ref(run_git, &base_ref);
    let commits_since_fork = if sha.is_some() && head_sha.is_some() {
        merge_base(run_git, "HEAD", &base_ref)
            .map(|base| rev_count(run_git, &format!("{base}..{base_ref}")))
            .unwrap_or(0)
    } else {
        0
    };

    BaseGitState {
        r#ref: base_ref,
        sha,
        commits_since_fork,
    }
}

fn compute_worktree_state<F>(run_git: &F) -> WorktreeGitState
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    let Ok(output) = run_git(&["status", "--porcelain=1", "--untracked-files=all"]) else {
        return WorktreeGitState {
            dirty: false,
            modified: 0,
            added: 0,
            deleted: 0,
            untracked: 0,
            conflicted: 0,
        };
    };

    parse_worktree_from_status(&output)
}

pub fn compute_branch_git_state<F>(
    cache_key: &str,
    run_git: F,
    branch_name: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
) -> BranchGitState
where
    F: Fn(&[&str]) -> Result<String, String> + Sync,
{
    // Phase 1: Fetch runs in parallel with local-only commands (HEAD, branch
    // name, worktree status) since those read local state unaffected by fetch.
    let (refresh, head_sha, branch, worktree) = std::thread::scope(|s| {
        let fetch_handle = s.spawn(|| {
            refresh_refs_if_needed(cache_key, &run_git, branch_name, base_branch, fetch_mode)
        });
        let head_handle = s.spawn(|| {
            run_git(&["rev-parse", "HEAD"])
                .ok()
                .and_then(trim_non_empty)
        });
        let branch_handle = s.spawn(|| current_branch(&run_git));
        let worktree_handle = s.spawn(|| compute_worktree_state(&run_git));

        (
            fetch_handle.join().expect("fetch thread panicked"),
            head_handle.join().expect("head thread panicked"),
            branch_handle.join().expect("branch thread panicked"),
            worktree_handle.join().expect("worktree thread panicked"),
        )
    });

    let detached_head = head_sha.is_some() && branch.is_none();
    let expected_branch = branch_name_without_origin(branch_name);
    let expected_branch_matches = branch
        .as_deref()
        .map(|current| current == expected_branch)
        .unwrap_or(false);
    let upstream_ref = origin_ref_for_branch(branch_name);
    let base_ref = origin_ref_for_branch(base_branch);

    // Phase 2: Upstream and base state computations are independent of each
    // other but both need fetch to have completed (for up-to-date remote refs)
    // and head_sha.
    let (upstream, base) = std::thread::scope(|s| {
        let upstream_handle = s.spawn(|| {
            compute_upstream_state(
                &run_git,
                upstream_ref,
                head_sha.as_deref(),
                refresh.upstream_known_missing,
            )
        });
        let base_handle = s.spawn(|| compute_base_state(&run_git, base_ref, head_sha.as_deref()));

        (
            upstream_handle.join().expect("upstream thread panicked"),
            base_handle.join().expect("base thread panicked"),
        )
    });

    BranchGitState {
        upstream,
        base,
        worktree,
        fetch: refresh.fetch,
        head_sha,
        current_branch: branch,
        detached_head,
        expected_branch_matches,
    }
}

pub fn compute_local_branch_git_state(
    repo: &Path,
    branch_name: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
) -> BranchGitState {
    let cache_key = format!("local:{}:{}:{}", repo.display(), branch_name, base_branch);
    compute_branch_git_state(
        &cache_key,
        |args| cli::run(repo, args).map_err(|e| e.to_string()),
        branch_name,
        base_branch,
        fetch_mode,
    )
}

/// Check whether a fetch is needed for the given cache key and mode.
/// Used by timeline to decide whether to use the two-stream path.
pub fn needs_fetch(cache_key: &str, fetch_mode: FetchMode) -> bool {
    let now = now_ms();
    let previous = fetch_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(cache_key).cloned());

    match (fetch_mode, &previous) {
        (FetchMode::Never, _) => false,
        (FetchMode::Force, _) => true,
        (FetchMode::Ttl, Some(entry)) => now.saturating_sub(entry.fetched_at) > FETCH_TTL_MS,
        (FetchMode::Ttl, None) => true,
    }
}

/// Compute fast (local-only) git state for a local branch.
/// Returns HEAD, branch name, and worktree status without any fetch.
pub fn compute_fast_local_git_state(repo: &Path, branch_name: &str) -> FastGitState {
    let run_git = |args: &[&str]| -> Result<String, String> {
        cli::run(repo, args).map_err(|e| e.to_string())
    };
    let (head_sha, branch, worktree) = std::thread::scope(|s| {
        let h = s.spawn(|| {
            run_git(&["rev-parse", "HEAD"])
                .ok()
                .and_then(trim_non_empty)
        });
        let b = s.spawn(|| current_branch(&run_git));
        let w = s.spawn(|| compute_worktree_state(&run_git));
        (
            h.join().expect("head thread panicked"),
            b.join().expect("branch thread panicked"),
            w.join().expect("worktree thread panicked"),
        )
    });
    let expected = branch_name_without_origin(branch_name);
    FastGitState {
        detached_head: head_sha.is_some() && branch.is_none(),
        expected_branch_matches: branch.as_deref().map(|c| c == expected).unwrap_or(false),
        head_sha,
        current_branch: branch,
        worktree,
    }
}

/// Complete a local branch git state: runs fetch + ref comparisons, combining
/// with a pre-computed `FastGitState`. Used by the slow stream after the
/// partial timeline has been emitted.
pub fn complete_local_git_state(
    repo: &Path,
    fast: &FastGitState,
    branch_name: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
) -> BranchGitState {
    let cache_key = format!("local:{}:{}:{}", repo.display(), branch_name, base_branch);
    let run_git = |args: &[&str]| -> Result<String, String> {
        cli::run(repo, args).map_err(|e| e.to_string())
    };

    let refresh =
        refresh_refs_if_needed(&cache_key, &run_git, branch_name, base_branch, fetch_mode);
    let upstream_ref = origin_ref_for_branch(branch_name);
    let base_ref = origin_ref_for_branch(base_branch);

    let (upstream, base) = std::thread::scope(|s| {
        let u = s.spawn(|| {
            compute_upstream_state(
                &run_git,
                upstream_ref,
                fast.head_sha.as_deref(),
                refresh.upstream_known_missing,
            )
        });
        let b = s.spawn(|| compute_base_state(&run_git, base_ref, fast.head_sha.as_deref()));
        (
            u.join().expect("upstream thread panicked"),
            b.join().expect("base thread panicked"),
        )
    });

    BranchGitState {
        head_sha: fast.head_sha.clone(),
        current_branch: fast.current_branch.clone(),
        detached_head: fast.detached_head,
        expected_branch_matches: fast.expected_branch_matches,
        worktree: fast.worktree.clone(),
        fetch: refresh.fetch,
        upstream,
        base,
    }
}

pub fn local_git_state_cache_key(repo: &Path, branch_name: &str, base_branch: &str) -> String {
    format!("local:{}:{}:{}", repo.display(), branch_name, base_branch)
}

// ---------------------------------------------------------------------------
// Batched computation for remote projects
// ---------------------------------------------------------------------------
//
// Instead of N separate round-trips (each `run_git` call is a `ws_exec`),
// we batch all git commands into a single shell script that performs fetch
// (when needed), local state collection, and ref comparisons in one call.
//
// The fetch TTL cache controls whether the fetch phase runs within the
// script (via a "skip_fetch" flag argument).

/// Combined shell script that performs fetch, collects local state, and computes
/// upstream + base ref state in a single round-trip.
///
/// Arguments:
///   $1 = repo_path
///   $2 = base_refspec
///   $3 = branch_refspec (empty if same as base)
///   $4 = upstream_ref (empty to skip upstream resolution)
///   $5 = base_ref
///   $6 = "skip_fetch" to skip the fetch phase (cache fresh)
const BATCH_GIT_STATE_SCRIPT: &str = concat!(
    "cd \"$1\" || exit 1\n",
    // --- Fetch phase (conditional) ---
    "fetch_err=''\n",
    "upstream_missing=''\n",
    "if [ \"$6\" != 'skip_fetch' ]; then\n",
    "  _ferr=$(mktemp) || exit 1\n",
    "  trap 'rm -f \"$_ferr\"' EXIT\n",
    "  if [ -n \"$3\" ]; then\n",
    "    if ! git fetch --prune origin \"$2\" \"$3\" 2>\"$_ferr\"; then\n",
    "      if grep -qi 'could.not.find.remote.ref\\|couldn.t.find.remote.ref' \"$_ferr\"; then\n",
    "        if ! git fetch --prune origin \"$2\" 2>\"$_ferr\"; then\n",
    "          fetch_err=$(cat \"$_ferr\")\n",
    "        else\n",
    "          upstream_missing=true\n",
    "        fi\n",
    "      else\n",
    "        fetch_err=$(cat \"$_ferr\")\n",
    "      fi\n",
    "    fi\n",
    "  else\n",
    "    if ! git fetch --prune origin \"$2\" 2>\"$_ferr\"; then\n",
    "      fetch_err=$(cat \"$_ferr\")\n",
    "    fi\n",
    "  fi\n",
    "fi\n",
    // --- Local state ---
    "head_sha=$(git rev-parse HEAD 2>/dev/null || true)\n",
    "printf 'HEAD=%s\\n' \"$head_sha\"\n",
    "printf 'BRANCH=%s\\n' \"$(git branch --show-current 2>/dev/null || true)\"\n",
    "echo STATUS_START\n",
    "git status --porcelain=1 --untracked-files=all 2>/dev/null || true\n",
    "echo STATUS_END\n",
    "[ -n \"$upstream_missing\" ] && echo 'UPSTREAM_MISSING=true'\n",
    "[ -n \"$fetch_err\" ] && printf 'FETCH_ERR=%s\\n' \"$fetch_err\"\n",
    // --- Upstream state (skip if $4 is empty) ---
    "if [ -n \"$4\" ] && [ -z \"$upstream_missing\" ]; then\n",
    "  up_sha=$(git rev-parse --verify \"$4\" 2>/dev/null || true)\n",
    "else\n",
    "  up_sha=''\n",
    "fi\n",
    "printf 'UP_SHA=%s\\n' \"$up_sha\"\n",
    "if [ -n \"$up_sha\" ] && [ -n \"$head_sha\" ]; then\n",
    "  printf 'UP_COUNTS=%s\\n' \"$(git rev-list --left-right --count \"$head_sha\"...\"$4\" 2>/dev/null || echo '0 0')\"\n",
    "  printf 'UP_MB=%s\\n' \"$(git merge-base \"$head_sha\" \"$4\" 2>/dev/null || true)\"\n",
    "fi\n",
    // --- Base state ---
    "base_sha=$(git rev-parse --verify \"$5\" 2>/dev/null || true)\n",
    "printf 'BASE_SHA=%s\\n' \"$base_sha\"\n",
    "if [ -n \"$base_sha\" ] && [ -n \"$head_sha\" ]; then\n",
    "  mb=$(git merge-base \"$head_sha\" \"$5\" 2>/dev/null || true)\n",
    "  printf 'BASE_MB=%s\\n' \"$mb\"\n",
    "  if [ -n \"$mb\" ]; then\n",
    "    printf 'BASE_BEHIND=%s\\n' \"$(git rev-list --count \"$mb\"..\"$5\" 2>/dev/null || echo 0)\"\n",
    "  fi\n",
    "fi\n",
    "exit 0\n",
);

/// Parsed output from the combined git state script.
struct BatchGitStateOutput {
    head_sha: Option<String>,
    branch: Option<String>,
    status_lines: String,
    upstream_missing: bool,
    fetch_error: Option<String>,
    up_sha: Option<String>,
    up_ahead: u32,
    up_behind: u32,
    up_merge_base: Option<String>,
    base_sha: Option<String>,
    base_behind: u32,
}

fn parse_batch_git_state_output(raw: &str) -> BatchGitStateOutput {
    let mut head_sha = None;
    let mut branch = None;
    let mut upstream_missing = false;
    let mut fetch_error = None;
    let mut status_lines = String::new();
    let mut in_status = false;
    let mut up_sha = None;
    let mut up_ahead = 0u32;
    let mut up_behind = 0u32;
    let mut up_merge_base = None;
    let mut base_sha = None;
    let mut base_behind = 0u32;

    for line in raw.lines() {
        if line == "STATUS_START" {
            in_status = true;
            continue;
        }
        if line == "STATUS_END" {
            in_status = false;
            continue;
        }
        if in_status {
            if !status_lines.is_empty() {
                status_lines.push('\n');
            }
            status_lines.push_str(line);
            continue;
        }
        if let Some(val) = line.strip_prefix("HEAD=") {
            let v = val.trim();
            if !v.is_empty() {
                head_sha = Some(v.to_string());
            }
        } else if let Some(val) = line.strip_prefix("BRANCH=") {
            let v = val.trim();
            if !v.is_empty() {
                branch = Some(v.to_string());
            }
        } else if line.starts_with("UPSTREAM_MISSING=") {
            upstream_missing = true;
        } else if let Some(val) = line.strip_prefix("FETCH_ERR=") {
            let v = val.trim();
            if !v.is_empty() {
                fetch_error = Some(v.to_string());
            }
        } else if let Some(val) = line.strip_prefix("UP_SHA=") {
            let v = val.trim();
            if !v.is_empty() {
                up_sha = Some(v.to_string());
            }
        } else if let Some(val) = line.strip_prefix("UP_COUNTS=") {
            let mut parts = val.split_whitespace();
            up_ahead = parse_u32(parts.next());
            up_behind = parse_u32(parts.next());
        } else if let Some(val) = line.strip_prefix("UP_MB=") {
            let v = val.trim();
            if !v.is_empty() {
                up_merge_base = Some(v.to_string());
            }
        } else if let Some(val) = line.strip_prefix("BASE_SHA=") {
            let v = val.trim();
            if !v.is_empty() {
                base_sha = Some(v.to_string());
            }
        } else if line.starts_with("BASE_MB=") {
            // Used internally by the script to compute BASE_BEHIND.
        } else if let Some(val) = line.strip_prefix("BASE_BEHIND=") {
            base_behind = parse_u32(Some(val.trim()));
        }
    }

    BatchGitStateOutput {
        head_sha,
        branch,
        status_lines,
        upstream_missing,
        fetch_error,
        up_sha,
        up_ahead,
        up_behind,
        up_merge_base,
        base_sha,
        base_behind,
    }
}

fn parse_worktree_from_status(status_output: &str) -> WorktreeGitState {
    let mut state = WorktreeGitState {
        dirty: false,
        modified: 0,
        added: 0,
        deleted: 0,
        untracked: 0,
        conflicted: 0,
    };

    let mut seen = std::collections::HashSet::new();

    for line in status_output.lines() {
        let mut chars = line.chars();
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');

        if x == '?' && y == '?' {
            state.untracked += 1;
            continue;
        }
        if is_conflicted_status(x, y) {
            state.conflicted += 1;
            continue;
        }

        // Skip the space after XY columns
        let path: String = chars.skip(1).collect();
        // For renames/copies the path contains " -> new_path"; use the destination
        let dedup_path = path.split(" -> ").last().unwrap_or(&path).to_string();
        if !seen.insert(dedup_path) {
            continue;
        }

        // Pick the most significant status code (prefer non-space)
        let code = if x != ' ' { x } else { y };
        match code {
            'M' | 'R' => state.modified += 1,
            'A' | 'C' => state.added += 1,
            'D' => state.deleted += 1,
            _ => state.modified += 1, // fallback for unexpected codes
        }
    }

    state.dirty = state.modified > 0
        || state.added > 0
        || state.deleted > 0
        || state.untracked > 0
        || state.conflicted > 0;
    state
}

// ---------------------------------------------------------------------------
// Fast script for remote two-stream split
// ---------------------------------------------------------------------------
//
// When a fetch is needed, the timeline uses two concurrent round-trips:
//   1. BATCH_FAST_SCRIPT — local state + commits (no fetch, returns immediately)
//   2. BATCH_GIT_STATE_SCRIPT — fetch + full ref comparisons (blocks on fetch)
//
// The fast script's output is used to emit a partial timeline event so
// commits + worktree rows appear before the slow stream completes.

/// Fast local-only script for remote projects.
///
/// Arguments:
///   $1 = repo_path
///   $2 = base_ref (e.g., "origin/main") — used for merge-base + git log
const BATCH_FAST_SCRIPT: &str = concat!(
    "cd \"$1\" || exit 1\n",
    "head_sha=$(git rev-parse HEAD 2>/dev/null || true)\n",
    "printf 'HEAD=%s\\n' \"$head_sha\"\n",
    "printf 'BRANCH=%s\\n' \"$(git branch --show-current 2>/dev/null || true)\"\n",
    "echo STATUS_START\n",
    "git status --porcelain=1 --untracked-files=all 2>/dev/null || true\n",
    "echo STATUS_END\n",
    // Commits using locally-cached refs
    "mb=$(git merge-base \"$2\" HEAD 2>/dev/null || true)\n",
    "if [ -n \"$mb\" ]; then\n",
    "  range=\"${mb}..HEAD\"\n",
    "else\n",
    "  range=\"$2..HEAD\"\n",
    "fi\n",
    "echo COMMITS_START\n",
    "git log --format='%H|%h|%s|%an|%ct' \"$range\" 2>/dev/null || true\n",
    "echo COMMITS_END\n",
    "exit 0\n",
);

/// Parsed output from the fast local-only script.
pub struct BatchFastOutput {
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    pub status_lines: String,
    pub commit_lines: Vec<String>,
}

pub fn parse_batch_fast_output(raw: &str) -> BatchFastOutput {
    let mut head_sha = None;
    let mut branch = None;
    let mut status_lines = String::new();
    let mut commit_lines = Vec::new();
    let mut in_status = false;
    let mut in_commits = false;

    for line in raw.lines() {
        if line == "STATUS_START" {
            in_status = true;
            continue;
        }
        if line == "STATUS_END" {
            in_status = false;
            continue;
        }
        if line == "COMMITS_START" {
            in_commits = true;
            continue;
        }
        if line == "COMMITS_END" {
            in_commits = false;
            continue;
        }
        if in_status {
            if !status_lines.is_empty() {
                status_lines.push('\n');
            }
            status_lines.push_str(line);
            continue;
        }
        if in_commits {
            if !line.is_empty() {
                commit_lines.push(line.to_string());
            }
            continue;
        }
        if let Some(val) = line.strip_prefix("HEAD=") {
            let v = val.trim();
            if !v.is_empty() {
                head_sha = Some(v.to_string());
            }
        } else if let Some(val) = line.strip_prefix("BRANCH=") {
            let v = val.trim();
            if !v.is_empty() {
                branch = Some(v.to_string());
            }
        }
    }

    BatchFastOutput {
        head_sha,
        branch,
        status_lines,
        commit_lines,
    }
}

impl BatchFastOutput {
    /// Convert to FastGitState.
    pub fn into_fast_git_state(self, branch_name: &str) -> (FastGitState, Vec<String>) {
        let worktree = parse_worktree_from_status(&self.status_lines);
        let expected = branch_name_without_origin(branch_name);
        let fast = FastGitState {
            detached_head: self.head_sha.is_some() && self.branch.is_none(),
            expected_branch_matches: self
                .branch
                .as_deref()
                .map(|c| c == expected)
                .unwrap_or(false),
            head_sha: self.head_sha,
            current_branch: self.branch,
            worktree,
        };
        (fast, self.commit_lines)
    }
}

/// Run the fast local-only script on a remote workspace and return parsed output.
pub fn compute_fast_git_state_batched<F>(
    run_script: &F,
    repo_path: &str,
    base_branch: &str,
) -> Result<BatchFastOutput, String>
where
    F: Fn(&str, &[&str]) -> Result<String, String>,
{
    let base_ref = origin_ref_for_branch(base_branch);
    let raw = run_script(BATCH_FAST_SCRIPT, &[repo_path, &base_ref])?;
    Ok(parse_batch_fast_output(&raw))
}

/// Compute branch git state using a single batched shell script.
///
/// This is the remote-optimised counterpart of `compute_branch_git_state`.
/// Instead of many individual `run_git` calls (each a separate `ws_exec`
/// round-trip), it bundles all commands into one shell script that performs
/// fetch (when needed), local state collection, and ref comparisons — giving
/// **1 round-trip total** regardless of whether fetch is cached or not.
pub fn compute_branch_git_state_batched<F>(
    cache_key: &str,
    run_script: F,
    repo_path: &str,
    branch_name: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
) -> BranchGitState
where
    F: Fn(&str, &[&str]) -> Result<String, String>,
{
    let base_refspec = refspec_for(base_branch);
    let branch_refspec = refspec_for(branch_name);
    let upstream_ref = origin_ref_for_branch(branch_name);
    let base_ref = origin_ref_for_branch(base_branch);
    let expected_branch = branch_name_without_origin(branch_name);

    // Check fetch cache to decide whether we need the fetch phase.
    let now = now_ms();
    let previous = fetch_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(cache_key).cloned());

    let should_fetch = match (fetch_mode, &previous) {
        (FetchMode::Never, _) => false,
        (FetchMode::Force, _) => true,
        (FetchMode::Ttl, Some(entry)) => now.saturating_sub(entry.fetched_at) > FETCH_TTL_MS,
        (FetchMode::Ttl, None) => true,
    };

    let branch_arg = if branch_refspec != base_refspec {
        branch_refspec.as_str()
    } else {
        ""
    };

    // When cache says upstream is missing, pass empty upstream_ref so the
    // script skips upstream resolution.
    let cached_upstream_missing = previous
        .as_ref()
        .map(|e| e.upstream_known_missing)
        .unwrap_or(false);
    let up_ref_arg = if !should_fetch && cached_upstream_missing {
        ""
    } else {
        upstream_ref.as_str()
    };
    let skip_fetch_arg = if should_fetch { "" } else { "skip_fetch" };

    let raw = run_script(
        BATCH_GIT_STATE_SCRIPT,
        &[
            repo_path,
            &base_refspec,
            branch_arg,
            up_ref_arg,
            &base_ref,
            skip_fetch_arg,
        ],
    );

    match raw {
        Ok(output) => {
            let parsed = parse_batch_git_state_output(&output);

            let upstream_known_missing = if should_fetch {
                parsed.upstream_missing
            } else {
                cached_upstream_missing
            };

            // Build fetch state
            let fetch_state = if should_fetch {
                if let Some(ref err) = parsed.fetch_error {
                    FetchGitState {
                        status: FetchStatus::Failed,
                        fetched_at: previous.as_ref().map(|e| e.fetched_at),
                        error: Some(err.clone()),
                    }
                } else {
                    if let Ok(mut cache) = fetch_cache().lock() {
                        cache.insert(
                            cache_key.to_string(),
                            FetchCacheEntry {
                                fetched_at: now,
                                upstream_known_missing,
                            },
                        );
                    }
                    FetchGitState {
                        status: FetchStatus::Fresh,
                        fetched_at: Some(now),
                        error: None,
                    }
                }
            } else {
                let fetched_at = previous.as_ref().map(|e| e.fetched_at);
                FetchGitState {
                    status: if fetched_at.is_some() {
                        FetchStatus::Fresh
                    } else {
                        FetchStatus::Stale
                    },
                    fetched_at,
                    error: None,
                }
            };

            let worktree = parse_worktree_from_status(&parsed.status_lines);
            let detached_head = parsed.head_sha.is_some() && parsed.branch.is_none();
            let expected_branch_matches = parsed
                .branch
                .as_deref()
                .map(|current| current == expected_branch)
                .unwrap_or(false);

            // Build upstream state from parsed output
            let up_exists = parsed.up_sha.is_some();
            let relation = if !up_exists || parsed.head_sha.is_none() {
                UpstreamRelation::Missing
            } else {
                match (parsed.up_ahead, parsed.up_behind) {
                    (0, 0) => UpstreamRelation::InSync,
                    (_, 0) => UpstreamRelation::LocalAhead,
                    (0, _) => UpstreamRelation::OriginAhead,
                    _ => UpstreamRelation::Diverged,
                }
            };

            let upstream = UpstreamGitState {
                r#ref: upstream_ref,
                exists: up_exists,
                sha: parsed.up_sha,
                relation,
                ahead: parsed.up_ahead,
                behind: parsed.up_behind,
                merge_base_sha: parsed.up_merge_base,
            };

            let base = BaseGitState {
                r#ref: base_ref,
                sha: parsed.base_sha,
                commits_since_fork: parsed.base_behind,
            };

            BranchGitState {
                upstream,
                base,
                worktree,
                fetch: fetch_state,
                head_sha: parsed.head_sha,
                current_branch: parsed.branch,
                detached_head,
                expected_branch_matches,
            }
        }
        Err(err) => {
            // Script execution itself failed
            let fetched_at = previous.as_ref().map(|e| e.fetched_at);
            BranchGitState {
                upstream: UpstreamGitState {
                    r#ref: upstream_ref,
                    exists: false,
                    sha: None,
                    relation: UpstreamRelation::Missing,
                    ahead: 0,
                    behind: 0,
                    merge_base_sha: None,
                },
                base: BaseGitState {
                    r#ref: base_ref,
                    sha: None,
                    commits_since_fork: 0,
                },
                worktree: WorktreeGitState {
                    dirty: false,
                    modified: 0,
                    added: 0,
                    deleted: 0,
                    untracked: 0,
                    conflicted: 0,
                },
                fetch: if should_fetch {
                    FetchGitState {
                        status: FetchStatus::Failed,
                        fetched_at,
                        error: Some(err),
                    }
                } else {
                    FetchGitState {
                        status: if fetched_at.is_some() {
                            FetchStatus::Fresh
                        } else {
                            FetchStatus::Stale
                        },
                        fetched_at,
                        error: None,
                    }
                },
                head_sha: None,
                current_branch: None,
                detached_head: false,
                expected_branch_matches: false,
            }
        }
    }
}

pub fn fast_forward_to_ref(repo: &Path, reference: &str) -> Result<(), GitError> {
    cli::run(repo, &["merge", "--ff-only", reference])?;
    Ok(())
}

pub fn ensure_fast_forward_pullable(state: &BranchGitState) -> Result<(), String> {
    if state.detached_head {
        return Err("Cannot pull while HEAD is detached".to_string());
    }
    if !state.expected_branch_matches {
        let current = state
            .current_branch
            .as_deref()
            .unwrap_or("an unknown branch");
        return Err(format!("Cannot pull while checked out on {current}"));
    }
    if state.worktree.dirty {
        return Err("Cannot pull with uncommitted changes".to_string());
    }
    if state.upstream.relation != UpstreamRelation::OriginAhead {
        return Err("Branch is not behind origin, or cannot be fast-forwarded".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_worktree(
        input: &str,
        dirty: bool,
        modified: u32,
        added: u32,
        deleted: u32,
        untracked: u32,
        conflicted: u32,
    ) {
        let state = parse_worktree_from_status(input);
        assert_eq!(
            state,
            WorktreeGitState {
                dirty,
                modified,
                added,
                deleted,
                untracked,
                conflicted,
            },
            "input: {input:?}"
        );
    }

    #[test]
    fn empty_status() {
        assert_worktree("", false, 0, 0, 0, 0, 0);
    }

    #[test]
    fn single_untracked() {
        assert_worktree("?? untracked.txt", true, 0, 0, 0, 1, 0);
    }

    #[test]
    fn multiple_untracked() {
        assert_worktree("?? ut1.txt\n?? ut2.txt", true, 0, 0, 0, 2, 0);
    }

    #[test]
    fn staged_add() {
        assert_worktree("A  file.txt", true, 0, 1, 0, 0, 0);
    }

    #[test]
    fn unstaged_modify() {
        assert_worktree(" M file.txt", true, 1, 0, 0, 0, 0);
    }

    #[test]
    fn staged_modify() {
        assert_worktree("M  file.txt", true, 1, 0, 0, 0, 0);
    }

    #[test]
    fn staged_and_unstaged_modify() {
        assert_worktree("MM file.txt", true, 1, 0, 0, 0, 0);
    }

    #[test]
    fn unstaged_delete() {
        assert_worktree(" D file.txt", true, 0, 0, 1, 0, 0);
    }

    #[test]
    fn staged_delete() {
        assert_worktree("D  file.txt", true, 0, 0, 1, 0, 0);
    }

    #[test]
    fn staged_rename() {
        assert_worktree("R  file.txt -> renamed.txt", true, 1, 0, 0, 0, 0);
    }

    #[test]
    fn staged_modify_then_unstaged_delete() {
        assert_worktree("MD file.txt", true, 1, 0, 0, 0, 0);
    }

    #[test]
    fn staged_add_then_unstaged_delete() {
        assert_worktree("AD brand_new.txt", true, 0, 1, 0, 0, 0);
    }

    #[test]
    fn mixed_changes() {
        assert_worktree(
            " M a.txt\nD  b.txt\nA  c.txt\n?? d.txt",
            true,
            1,
            1,
            1,
            1,
            0,
        );
    }

    #[test]
    fn all_staged() {
        assert_worktree("M  a.txt\nD  b.txt\nA  e.txt", true, 1, 1, 1, 0, 0);
    }

    #[test]
    fn conflict_both_modified() {
        assert_worktree("UU file.txt", true, 0, 0, 0, 0, 1);
    }

    #[test]
    fn conflict_add_add() {
        assert_worktree("AA both.txt", true, 0, 0, 0, 0, 1);
    }

    #[test]
    fn conflict_delete_update() {
        assert_worktree("UD a.txt", true, 0, 0, 0, 0, 1);
    }

    #[test]
    fn conflict_plus_untracked_plus_modified() {
        assert_worktree(
            "UU file.txt\n?? untracked.txt\n M other.txt",
            true,
            1,
            0,
            0,
            1,
            1,
        );
    }

    #[test]
    fn dedup_same_file_two_lines() {
        assert_worktree("M  file.txt\n M file.txt", true, 1, 0, 0, 0, 0);
    }

    #[test]
    fn rename_destination_collides() {
        assert_worktree("R  old.txt -> new.txt\n M new.txt", true, 1, 0, 0, 0, 0);
    }

    #[test]
    fn is_conflicted_status_pairs() {
        // All 7 conflict pairs
        assert!(is_conflicted_status('D', 'D'));
        assert!(is_conflicted_status('A', 'U'));
        assert!(is_conflicted_status('U', 'D'));
        assert!(is_conflicted_status('U', 'A'));
        assert!(is_conflicted_status('D', 'U'));
        assert!(is_conflicted_status('A', 'A'));
        assert!(is_conflicted_status('U', 'U'));

        // Non-conflict pairs
        assert!(!is_conflicted_status('M', ' '));
        assert!(!is_conflicted_status(' ', 'M'));
        assert!(!is_conflicted_status('A', ' '));
        assert!(!is_conflicted_status(' ', 'D'));
        assert!(!is_conflicted_status('R', ' '));
        assert!(!is_conflicted_status('?', '?'));
    }
}
