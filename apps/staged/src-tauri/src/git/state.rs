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
    pub staged: u32,
    pub unstaged: u32,
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
    let mut state = WorktreeGitState {
        dirty: false,
        staged: 0,
        unstaged: 0,
        untracked: 0,
        conflicted: 0,
    };

    let Ok(output) = run_git(&["status", "--porcelain=1", "--untracked-files=all"]) else {
        return state;
    };

    for line in output.lines() {
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
        if x != ' ' {
            state.staged += 1;
        }
        if y != ' ' {
            state.unstaged += 1;
        }
    }

    state.dirty =
        state.staged > 0 || state.unstaged > 0 || state.untracked > 0 || state.conflicted > 0;
    state
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

// ---------------------------------------------------------------------------
// Batched computation for remote projects
// ---------------------------------------------------------------------------
//
// Instead of N separate round-trips (each `run_git` call is a `ws_exec`),
// we batch multiple git commands into two shell scripts:
//
// Script 1 (fetch + local state): fetch, rev-parse HEAD, branch name, status
// Script 2 (post-fetch ref state): upstream SHA/counts/merge-base, base SHA/merge-base/count
//
// The fetch TTL cache still controls whether Script 1 is needed at all.

/// Shell script that performs fetch and collects local state in one round-trip.
/// Arguments: $1=repo_path  $2=base_refspec  $3=branch_refspec (empty if same as base)
const BATCH_FETCH_SCRIPT: &str = concat!(
    "cd \"$1\" || exit 1\n",
    // Fetch — capture stderr for missing-ref detection using unique temp files
    "fetch_err=''\n",
    "upstream_missing=''\n",
    "_ferr=$(mktemp) || exit 1\n",
    "trap 'rm -f \"$_ferr\"' EXIT\n",
    "if [ -n \"$3\" ]; then\n",
    "  if ! git fetch --prune origin \"$2\" \"$3\" 2>\"$_ferr\"; then\n",
    "    if grep -qi 'could.not.find.remote.ref\\|couldn.t.find.remote.ref' \"$_ferr\"; then\n",
    "      if ! git fetch --prune origin \"$2\" 2>\"$_ferr\"; then\n",
    "        fetch_err=$(cat \"$_ferr\")\n",
    "      else\n",
    "        upstream_missing=true\n",
    "      fi\n",
    "    else\n",
    "      fetch_err=$(cat \"$_ferr\")\n",
    "    fi\n",
    "  fi\n",
    "else\n",
    "  if ! git fetch --prune origin \"$2\" 2>\"$_ferr\"; then\n",
    "    fetch_err=$(cat \"$_ferr\")\n",
    "  fi\n",
    "fi\n",
    // Local state (unaffected by fetch)
    "printf 'HEAD=%s\\n' \"$(git rev-parse HEAD 2>/dev/null || true)\"\n",
    "printf 'BRANCH=%s\\n' \"$(git branch --show-current 2>/dev/null || true)\"\n",
    "echo STATUS_START\n",
    "git status --porcelain=1 --untracked-files=all 2>/dev/null || true\n",
    "echo STATUS_END\n",
    "[ -n \"$upstream_missing\" ] && echo 'UPSTREAM_MISSING=true'\n",
    "[ -n \"$fetch_err\" ] && printf 'FETCH_ERR=%s\\n' \"$fetch_err\"\n",
);

/// Shell script that computes upstream and base ref state in one round-trip.
/// Arguments: $1=repo_path  $2=upstream_ref  $3=head_ref(HEAD)  $4=base_ref
const BATCH_REFS_SCRIPT: &str = concat!(
    "cd \"$1\" || exit 1\n",
    // Upstream state
    "up_sha=$(git rev-parse --verify \"$2\" 2>/dev/null || true)\n",
    "printf 'UP_SHA=%s\\n' \"$up_sha\"\n",
    "if [ -n \"$up_sha\" ] && [ -n \"$3\" ]; then\n",
    "  printf 'UP_COUNTS=%s\\n' \"$(git rev-list --left-right --count \"$3\"...\"$2\" 2>/dev/null || echo '0 0')\"\n",
    "  printf 'UP_MB=%s\\n' \"$(git merge-base \"$3\" \"$2\" 2>/dev/null || true)\"\n",
    "fi\n",
    // Base state
    "base_sha=$(git rev-parse --verify \"$4\" 2>/dev/null || true)\n",
    "printf 'BASE_SHA=%s\\n' \"$base_sha\"\n",
    "if [ -n \"$base_sha\" ] && [ -n \"$3\" ]; then\n",
    "  mb=$(git merge-base \"$3\" \"$4\" 2>/dev/null || true)\n",
    "  printf 'BASE_MB=%s\\n' \"$mb\"\n",
    "  if [ -n \"$mb\" ]; then\n",
    "    printf 'BASE_BEHIND=%s\\n' \"$(git rev-list --count \"$mb\"..\"$4\" 2>/dev/null || echo 0)\"\n",
    "  fi\n",
    "fi\n",
);

/// Parsed output from the fetch + local state script.
struct BatchFetchOutput {
    head_sha: Option<String>,
    branch: Option<String>,
    status_lines: String,
    upstream_missing: bool,
    fetch_error: Option<String>,
}

fn parse_batch_fetch_output(raw: &str) -> BatchFetchOutput {
    let mut head_sha = None;
    let mut branch = None;
    let mut upstream_missing = false;
    let mut fetch_error = None;
    let mut status_lines = String::new();
    let mut in_status = false;

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
        }
    }

    BatchFetchOutput {
        head_sha,
        branch,
        status_lines,
        upstream_missing,
        fetch_error,
    }
}

/// Parsed output from the refs state script.
struct BatchRefsOutput {
    up_sha: Option<String>,
    up_ahead: u32,
    up_behind: u32,
    up_merge_base: Option<String>,
    base_sha: Option<String>,
    base_behind: u32,
}

fn parse_batch_refs_output(raw: &str) -> BatchRefsOutput {
    let mut up_sha = None;
    let mut up_ahead = 0u32;
    let mut up_behind = 0u32;
    let mut up_merge_base = None;
    let mut base_sha = None;
    let mut base_behind = 0u32;

    for line in raw.lines() {
        if let Some(val) = line.strip_prefix("UP_SHA=") {
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
            // Parsed by the script internally to compute BASE_BEHIND; not
            // needed in the Rust-side output struct.
        } else if let Some(val) = line.strip_prefix("BASE_BEHIND=") {
            base_behind = parse_u32(Some(val.trim()));
        }
    }

    BatchRefsOutput {
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
        staged: 0,
        unstaged: 0,
        untracked: 0,
        conflicted: 0,
    };

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
        if x != ' ' {
            state.staged += 1;
        }
        if y != ' ' {
            state.unstaged += 1;
        }
    }

    state.dirty =
        state.staged > 0 || state.unstaged > 0 || state.untracked > 0 || state.conflicted > 0;
    state
}

/// Compute branch git state using batched shell scripts.
///
/// This is the remote-optimised counterpart of `compute_branch_git_state`.
/// Instead of many individual `run_git` calls (each a separate `ws_exec`
/// round-trip), it bundles commands into at most two shell scripts:
///
/// - Script 1 (fetch + local state): 1 round-trip instead of 4-5
/// - Script 2 (post-fetch ref state): 1 round-trip instead of 6
///
/// When the fetch TTL cache says "skip fetch", only Script 2 runs
/// (with a simpler local-only preamble), giving **1 round-trip total**.
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

    // Check fetch cache to decide whether we need Script 1 at all.
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

    // Phase 1: fetch + local state (or just local state if cache is fresh)
    let (fetch_state, head_sha, branch, worktree, upstream_known_missing) = if should_fetch {
        // Full fetch + local state script
        let branch_arg = if branch_refspec != base_refspec {
            branch_refspec.as_str()
        } else {
            ""
        };
        let raw = run_script(BATCH_FETCH_SCRIPT, &[repo_path, &base_refspec, branch_arg]);

        match raw {
            Ok(output) => {
                let parsed = parse_batch_fetch_output(&output);
                let fetch = if let Some(ref err) = parsed.fetch_error {
                    FetchGitState {
                        status: FetchStatus::Failed,
                        fetched_at: previous.as_ref().map(|e| e.fetched_at),
                        error: Some(err.clone()),
                    }
                } else {
                    // Update cache on successful fetch
                    if let Ok(mut cache) = fetch_cache().lock() {
                        cache.insert(
                            cache_key.to_string(),
                            FetchCacheEntry {
                                fetched_at: now,
                                upstream_known_missing: parsed.upstream_missing,
                            },
                        );
                    }
                    FetchGitState {
                        status: FetchStatus::Fresh,
                        fetched_at: Some(now),
                        error: None,
                    }
                };
                let worktree = parse_worktree_from_status(&parsed.status_lines);
                (
                    fetch,
                    parsed.head_sha,
                    parsed.branch,
                    worktree,
                    parsed.upstream_missing,
                )
            }
            Err(err) => {
                // Script execution itself failed — treat as fetch failure
                let fetch = FetchGitState {
                    status: FetchStatus::Failed,
                    fetched_at: previous.as_ref().map(|e| e.fetched_at),
                    error: Some(err),
                };
                let worktree = WorktreeGitState {
                    dirty: false,
                    staged: 0,
                    unstaged: 0,
                    untracked: 0,
                    conflicted: 0,
                };
                (fetch, None, None, worktree, false)
            }
        }
    } else {
        // Cache says skip fetch — just get local state with a minimal script
        let local_script = concat!(
            "cd \"$1\" || exit 1\n",
            "printf 'HEAD=%s\\n' \"$(git rev-parse HEAD 2>/dev/null || true)\"\n",
            "printf 'BRANCH=%s\\n' \"$(git branch --show-current 2>/dev/null || true)\"\n",
            "echo STATUS_START\n",
            "git status --porcelain=1 --untracked-files=all 2>/dev/null || true\n",
            "echo STATUS_END\n",
        );
        let fetched_at = previous.as_ref().map(|entry| entry.fetched_at);
        let upstream_known_missing = previous
            .as_ref()
            .map(|e| e.upstream_known_missing)
            .unwrap_or(false);

        match run_script(local_script, &[repo_path]) {
            Ok(output) => {
                let parsed = parse_batch_fetch_output(&output);
                let fetch = FetchGitState {
                    status: if fetched_at.is_some() {
                        FetchStatus::Fresh
                    } else {
                        FetchStatus::Stale
                    },
                    fetched_at,
                    error: None,
                };
                let worktree = parse_worktree_from_status(&parsed.status_lines);
                (
                    fetch,
                    parsed.head_sha,
                    parsed.branch,
                    worktree,
                    upstream_known_missing,
                )
            }
            Err(_) => {
                let fetch = FetchGitState {
                    status: if fetched_at.is_some() {
                        FetchStatus::Fresh
                    } else {
                        FetchStatus::Stale
                    },
                    fetched_at,
                    error: None,
                };
                let worktree = WorktreeGitState {
                    dirty: false,
                    staged: 0,
                    unstaged: 0,
                    untracked: 0,
                    conflicted: 0,
                };
                (fetch, None, None, worktree, upstream_known_missing)
            }
        }
    };

    let detached_head = head_sha.is_some() && branch.is_none();
    let expected_branch_matches = branch
        .as_deref()
        .map(|current| current == expected_branch)
        .unwrap_or(false);

    // Phase 2: upstream + base ref state (single round-trip)
    let head_arg = head_sha.as_deref().unwrap_or("");
    let up_ref_arg = if upstream_known_missing {
        "" // Skip upstream resolution when we know it's missing
    } else {
        upstream_ref.as_str()
    };

    let (upstream, base) = match run_script(
        BATCH_REFS_SCRIPT,
        &[repo_path, up_ref_arg, head_arg, &base_ref],
    ) {
        Ok(output) => {
            let refs = parse_batch_refs_output(&output);

            let up_exists = refs.up_sha.is_some();
            let relation = if !up_exists || head_sha.is_none() {
                UpstreamRelation::Missing
            } else {
                match (refs.up_ahead, refs.up_behind) {
                    (0, 0) => UpstreamRelation::InSync,
                    (_, 0) => UpstreamRelation::LocalAhead,
                    (0, _) => UpstreamRelation::OriginAhead,
                    _ => UpstreamRelation::Diverged,
                }
            };

            let upstream = UpstreamGitState {
                r#ref: upstream_ref,
                exists: up_exists,
                sha: refs.up_sha,
                relation,
                ahead: refs.up_ahead,
                behind: refs.up_behind,
                merge_base_sha: refs.up_merge_base,
            };

            let base = BaseGitState {
                r#ref: base_ref,
                sha: refs.base_sha,
                commits_since_fork: refs.base_behind,
            };

            (upstream, base)
        }
        Err(_) => {
            let upstream = UpstreamGitState {
                r#ref: upstream_ref,
                exists: false,
                sha: None,
                relation: UpstreamRelation::Missing,
                ahead: 0,
                behind: 0,
                merge_base_sha: None,
            };
            let base = BaseGitState {
                r#ref: base_ref,
                sha: None,
                commits_since_fork: 0,
            };
            (upstream, base)
        }
    };

    BranchGitState {
        upstream,
        base,
        worktree,
        fetch: fetch_state,
        head_sha,
        current_branch: branch,
        detached_head,
        expected_branch_matches,
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
