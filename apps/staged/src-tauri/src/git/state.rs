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
        .and_then(|output| {
            let mut parts = output.split_whitespace();
            Some((parse_u32(parts.next()), parse_u32(parts.next())))
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
    F: Fn(&[&str]) -> Result<String, String>,
{
    let refresh = refresh_refs_if_needed(cache_key, &run_git, branch_name, base_branch, fetch_mode);
    let head_sha = run_git(&["rev-parse", "HEAD"])
        .ok()
        .and_then(trim_non_empty);
    let current_branch = current_branch(&run_git);
    let detached_head = head_sha.is_some() && current_branch.is_none();
    let expected_branch = branch_name_without_origin(branch_name);
    let expected_branch_matches = current_branch
        .as_deref()
        .map(|current| current == expected_branch)
        .unwrap_or(false);
    let upstream_ref = origin_ref_for_branch(branch_name);
    let base_ref = origin_ref_for_branch(base_branch);

    BranchGitState {
        upstream: compute_upstream_state(
            &run_git,
            upstream_ref,
            head_sha.as_deref(),
            refresh.upstream_known_missing,
        ),
        base: compute_base_state(&run_git, base_ref, head_sha.as_deref()),
        worktree: compute_worktree_state(&run_git),
        fetch: refresh.fetch,
        head_sha,
        current_branch,
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
