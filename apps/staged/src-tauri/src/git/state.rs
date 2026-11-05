use super::cli::{self, EnvSource, GitError};
use super::refs::{branch_name_without_origin, origin_ref_for_branch};
use super::status_parse::is_conflicted_status;
use super::upstream::upstream_branch_name_from_config;
use super::worktree::BRANCH_COMMIT_LOG_FORMAT;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
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

/// Controls whether `git status` enumerates untracked files.
///
/// `Full` (`--untracked-files=all`) walks the entire working tree and on
/// monorepos like cash-server can take 7–17s even with `core.fsmonitor` +
/// `core.untrackedcache` enabled, because untracked-cache invalidates per-
/// directory mtime and doesn't help in heavily-edited microservice trees.
/// `Indexed` (`--untracked-files=no`) only consults the index for tracked-
/// file modifications and returns in tens of ms.
///
/// The foreground timeline path uses `Indexed` for first paint; the
/// background `refresh_branch_git_state` (and destructive ops that need an
/// accurate dirty/untracked count) uses `Full`, so the `git-state-updated`
/// event re-emits the corrected counts shortly after first paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeStatusScope {
    Indexed,
    Full,
}

impl WorktreeStatusScope {
    fn git_flag(self) -> &'static str {
        match self {
            WorktreeStatusScope::Indexed => "--untracked-files=no",
            WorktreeStatusScope::Full => "--untracked-files=all",
        }
    }

    fn script_arg(self) -> &'static str {
        match self {
            WorktreeStatusScope::Indexed => "uno",
            WorktreeStatusScope::Full => "uall",
        }
    }
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
    /// Number of commits `origin/{base_branch}` is ahead of `origin/{branch_name}`.
    ///
    /// Non-zero means the remote branch tip is stale relative to base. The
    /// timeline UI uses this to disable "Rebase onto Origin" when rebasing
    /// onto a behind-base remote tip would skip the latest base commits and
    /// almost always be the wrong action.
    pub behind_base: u32,
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

/// Repo-level fetch tracking for local projects.
///
/// A single `git fetch` updates all remote refs for a repo, so when multiple
/// branches share the same local `.git` directory we only need to hit the
/// network once per TTL window. This cache is keyed by repo path and tracks
/// *which* refspecs were included in the last fetch so we can do a cheap
/// narrow fetch for any new refspecs that appear.
#[derive(Debug, Clone)]
struct RepoFetchEntry {
    fetched_at: i64,
    fetched_refspecs: HashSet<String>,
}

fn repo_fetch_cache() -> &'static Mutex<HashMap<String, RepoFetchEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, RepoFetchEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Extract the repo path portion from a local cache key (`local:{repo_path}:…`).
fn repo_key_from_local_cache_key(cache_key: &str) -> Option<String> {
    let rest = cache_key.strip_prefix("local:")?;
    // The key format is `local:{repo_path}:{branch}:{base}`.
    // repo_path may contain colons (e.g. Windows paths, though unlikely on
    // macOS/Linux). We split from the right to peel off base and branch.
    let (without_base, _base) = rest.rsplit_once(':')?;
    let (repo_path, _branch) = without_base.rsplit_once(':')?;
    Some(format!("local:{repo_path}"))
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

/// Determine which refspecs still need fetching given repo-level cache state.
///
/// Returns `None` if no fetch is needed at all (repo fresh + all refspecs covered).
/// Returns `Some(refspecs)` with the list of refspecs to fetch — either all of
/// them (repo stale) or just the missing ones (repo fresh, new refspecs).
fn refspecs_to_fetch(
    repo_key: Option<&str>,
    needed: &[&str],
    fetch_mode: FetchMode,
    now: i64,
) -> Option<Vec<String>> {
    match fetch_mode {
        FetchMode::Never => None,
        FetchMode::Force => Some(needed.iter().map(|s| s.to_string()).collect()),
        FetchMode::Ttl => {
            let repo_entry = repo_key.and_then(|key| {
                repo_fetch_cache()
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(key).cloned())
            });
            match repo_entry {
                Some(entry) if now.saturating_sub(entry.fetched_at) <= FETCH_TTL_MS => {
                    // Repo is fresh — only fetch refspecs not already covered.
                    let missing: Vec<String> = needed
                        .iter()
                        .filter(|rs| !entry.fetched_refspecs.contains(**rs))
                        .map(|s| s.to_string())
                        .collect();
                    if missing.is_empty() {
                        None
                    } else {
                        Some(missing)
                    }
                }
                _ => Some(needed.iter().map(|s| s.to_string()).collect()),
            }
        }
    }
}

/// What a build will fetch, decided from the caches before any git runs.
struct FetchPlan {
    now: i64,
    previous: Option<FetchCacheEntry>,
    repo_key: Option<String>,
    base_refspec: String,
    /// Every refspec the branch needs current (base, plus the branch's own
    /// when it differs), whether or not this build fetches them.
    needed: Vec<String>,
    /// Refspecs to send over the wire; `None` when nothing is fetched —
    /// `FetchMode::Never`, or the caches say the refs are fresh enough.
    refspecs: Option<Vec<String>>,
}

impl FetchPlan {
    fn will_fetch(&self) -> bool {
        self.refspecs.is_some()
    }

    /// What the last fetch learned about the upstream, which is all a build
    /// that fetches nothing has to go on.
    fn cached_upstream_known_missing(&self) -> bool {
        self.previous
            .as_ref()
            .map(|entry| entry.upstream_known_missing)
            .unwrap_or(false)
    }
}

fn plan_fetch(
    cache_key: &str,
    remote_branch: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
) -> FetchPlan {
    let now = now_ms();
    let previous = fetch_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(cache_key).cloned());

    let base_refspec = refspec_for(base_branch);
    let branch_refspec = refspec_for(remote_branch);

    // Build the list of needed refspecs (deduplicated).
    let needed: Vec<String> = if branch_refspec != base_refspec {
        vec![base_refspec.clone(), branch_refspec]
    } else {
        vec![base_refspec.clone()]
    };

    // For local keys, consult the repo-level cache to avoid redundant fetches
    // when multiple branches share the same repo.
    let repo_key = repo_key_from_local_cache_key(cache_key);
    let needed_refs: Vec<&str> = needed.iter().map(String::as_str).collect();
    let to_fetch = refspecs_to_fetch(repo_key.as_deref(), &needed_refs, fetch_mode, now);

    // Also check the per-branch cache for the legacy should_fetch decision
    // (used when there's no repo key, i.e. remote branches).
    let should_fetch = if repo_key.is_some() {
        to_fetch.is_some()
    } else {
        match (fetch_mode, &previous) {
            (FetchMode::Never, _) => false,
            (FetchMode::Force, _) => true,
            (FetchMode::Ttl, Some(entry)) => now.saturating_sub(entry.fetched_at) > FETCH_TTL_MS,
            (FetchMode::Ttl, None) => true,
        }
    };
    // Fallback for remote keys (no repo-level cache) — fetch all needed.
    let refspecs = should_fetch.then(|| to_fetch.unwrap_or_else(|| needed.clone()));

    FetchPlan {
        now,
        previous,
        repo_key,
        base_refspec,
        needed,
        refspecs,
    }
}

/// Run the fetch a [`FetchPlan`] calls for, or report the cached freshness
/// when it calls for none. Spawns nothing in the latter case.
fn run_planned_fetch<F>(cache_key: &str, plan: &FetchPlan, run_git: &F) -> RefreshOutcome
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    let previous = plan.previous.as_ref();
    let Some(fetch_refspecs) = plan.refspecs.as_ref() else {
        let fetched_at = previous.map(|entry| entry.fetched_at).or_else(|| {
            // For local keys the repo-level cache may have a timestamp even
            // if the per-branch cache doesn't yet.
            plan.repo_key.as_deref().and_then(|key| {
                repo_fetch_cache()
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(key).map(|e| e.fetched_at))
            })
        });
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
            upstream_known_missing: plan.cached_upstream_known_missing(),
        };
    };

    // Is this a narrow supplemental fetch (repo fresh, just filling in gaps)?
    let is_narrow = plan.repo_key.is_some() && fetch_refspecs.len() < plan.needed.len();
    let failed = |error: String| RefreshOutcome {
        fetch: FetchGitState {
            status: FetchStatus::Failed,
            fetched_at: previous.map(|entry| entry.fetched_at),
            error: Some(error.trim().to_string()),
        },
        upstream_known_missing: false,
    };

    let mut upstream_known_missing = false;

    if fetch_refspecs.len() > 1 {
        let refs: Vec<&str> = fetch_refspecs.iter().map(String::as_str).collect();
        let mut args = vec!["fetch"];
        if !is_narrow {
            args.push("--prune");
        }
        args.push("origin");
        args.extend(refs.iter());
        match run_git(&args) {
            Err(error) if is_missing_remote_ref(&error) => {
                // The branch refspec is missing on the remote. Re-fetch with
                // just the base refspec so we still get base branch updates.
                let mut retry_args = vec!["fetch"];
                if !is_narrow {
                    retry_args.push("--prune");
                }
                retry_args.push("origin");
                retry_args.push(plan.base_refspec.as_str());
                if let Err(base_err) = run_git(&retry_args) {
                    return failed(base_err);
                }
                upstream_known_missing = true;
            }
            Err(error) => return failed(error),
            Ok(_) => {}
        }
    } else {
        let refspec = fetch_refspecs
            .first()
            .map(String::as_str)
            .unwrap_or(plan.base_refspec.as_str());
        let mut args = vec!["fetch"];
        if !is_narrow {
            args.push("--prune");
        }
        args.push("origin");
        args.push(refspec);
        if let Err(error) = run_git(&args) {
            if is_missing_remote_ref(&error) {
                upstream_known_missing = true;
            } else {
                return failed(error);
            }
        }
    }

    // Update per-branch cache.
    if let Ok(mut cache) = fetch_cache().lock() {
        cache.insert(
            cache_key.to_string(),
            FetchCacheEntry {
                fetched_at: plan.now,
                upstream_known_missing,
            },
        );
    }

    // Update repo-level cache for local keys.
    if let Some(ref rk) = plan.repo_key {
        if let Ok(mut cache) = repo_fetch_cache().lock() {
            let entry = cache.entry(rk.clone()).or_insert_with(|| RepoFetchEntry {
                fetched_at: plan.now,
                fetched_refspecs: HashSet::new(),
            });
            entry.fetched_at = plan.now;
            if is_narrow {
                for rs in fetch_refspecs {
                    entry.fetched_refspecs.insert(rs.clone());
                }
            } else {
                entry.fetched_refspecs = plan.needed.iter().cloned().collect();
            }
        }
    }

    RefreshOutcome {
        fetch: FetchGitState {
            status: FetchStatus::Fresh,
            fetched_at: Some(plan.now),
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

fn merge_base<F>(run_git: &F, left: &str, right: &str) -> Option<String>
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    run_git(&["merge-base", left, right])
        .ok()
        .and_then(trim_non_empty)
}

fn upstream_relation(ahead: u32, behind: u32) -> UpstreamRelation {
    match (ahead, behind) {
        (0, 0) => UpstreamRelation::InSync,
        (_, 0) => UpstreamRelation::LocalAhead,
        (0, _) => UpstreamRelation::OriginAhead,
        _ => UpstreamRelation::Diverged,
    }
}

/// `HEAD` from one spawn: the commit it points at and the branch it is on.
struct HeadInfo {
    sha: Option<String>,
    branch: Option<String>,
}

fn read_head<F>(run_git: &F) -> HeadInfo
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    match run_git(&["rev-parse", "HEAD", "--symbolic-full-name", "HEAD"]) {
        Ok(output) => parse_head_output(&output),
        // An unborn branch has a name but no commit: `rev-parse HEAD` fails
        // outright, so ask for the name on its own. This is the only case
        // that costs a second spawn.
        Err(_) => HeadInfo {
            sha: None,
            branch: current_branch(run_git),
        },
    }
}

/// `git rev-parse HEAD --symbolic-full-name HEAD` prints the sha, then
/// `refs/heads/<branch>` — or `HEAD` itself when detached, which (like any
/// symref outside `refs/heads/`) is no branch, as `branch --show-current`
/// would also say.
fn parse_head_output(raw: &str) -> HeadInfo {
    let mut lines = raw.lines().map(str::trim);
    let sha = lines
        .next()
        .filter(|line| !line.is_empty())
        .map(str::to_string);
    let branch = lines
        .next()
        .and_then(|line| line.strip_prefix("refs/heads/"))
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    HeadInfo { sha, branch }
}

const FOR_EACH_REF_FORMAT: &str = "--format=%(refname) %(objectname)";

/// Parse `for-each-ref --format='%(refname) %(objectname)'` output into a
/// refname → sha map. Ref names cannot contain spaces, so the first space is
/// the separator; lines without both parts are skipped.
fn parse_for_each_ref_output(raw: &str) -> HashMap<String, String> {
    raw.lines()
        .filter_map(|line| {
            let (refname, sha) = line.trim().split_once(' ')?;
            let sha = sha.trim();
            (!refname.is_empty() && !sha.is_empty()).then(|| (refname.to_string(), sha.to_string()))
        })
        .collect()
}

/// The remote-side refs a build compares against, read once per build.
struct RemoteRefs {
    upstream_sha: Option<String>,
    base_sha: Option<String>,
    /// `merge-base HEAD origin/<base>`: the fork point both the base count
    /// and the commit log start from. `None` when either side is missing or
    /// the histories are unrelated.
    base_merge_base: Option<String>,
}

/// Resolve `origin/<upstream>` and `origin/<base>` with one `for-each-ref`,
/// and the fork point from `origin/<base>` with one `merge-base`, in
/// parallel.
///
/// A ref `for-each-ref` cannot find is simply not listed, which is what makes
/// this one spawn instead of a `rev-parse --verify` per ref. Its patterns
/// match whole path components, so `refs/remotes/origin/main` would also
/// list `refs/remotes/origin/main/<x>` were `origin/main` itself missing; the
/// output is matched on the exact refname. The `merge-base` runs without
/// first checking that `origin/<base>` exists: when it doesn't, that is one
/// failing spawn git rejects immediately ("Not a valid object name"), which
/// the cli layer knows not to retry.
fn read_remote_refs<F>(
    run_git: &F,
    upstream_ref: &str,
    base_ref: &str,
    upstream_known_missing: bool,
) -> RemoteRefs
where
    F: Fn(&[&str]) -> Result<String, String> + Sync,
{
    std::thread::scope(|s| {
        let refs_handle = s.spawn(|| {
            let upstream_full = format!("refs/remotes/{upstream_ref}");
            let base_full = format!("refs/remotes/{base_ref}");
            let mut args = vec!["for-each-ref", FOR_EACH_REF_FORMAT, base_full.as_str()];
            // The fetch cache already knows the upstream is not on origin:
            // don't look for it, and don't report a stale local copy.
            let want_upstream = !upstream_known_missing;
            if want_upstream && upstream_full != base_full {
                args.push(upstream_full.as_str());
            }
            let mut resolved = run_git(&args)
                .map(|output| parse_for_each_ref_output(&output))
                .unwrap_or_default();
            let base_sha = resolved.remove(&base_full);
            let upstream_sha = match (want_upstream, upstream_full == base_full) {
                (false, _) => None,
                (true, true) => base_sha.clone(),
                (true, false) => resolved.remove(&upstream_full),
            };
            (upstream_sha, base_sha)
        });
        let merge_base_handle = s.spawn(|| merge_base(run_git, "HEAD", base_ref));

        let (upstream_sha, base_sha) = refs_handle.join().expect("remote refs thread panicked");
        RemoteRefs {
            upstream_sha,
            base_sha,
            base_merge_base: merge_base_handle
                .join()
                .expect("base merge-base thread panicked"),
        }
    })
}

/// Ahead/behind against the upstream, and their merge base.
///
/// `rev-list --left-right --count` settles the relation, and the merge base
/// follows from it in every case but one: a branch in sync with, ahead of,
/// or behind its upstream has one tip as an ancestor of the other, and the
/// merge base is that tip. Only a diverged pair needs git to find it.
fn upstream_counts_and_merge_base<F>(
    run_git: &F,
    upstream_ref: &str,
    upstream_sha: &str,
    head_sha: &str,
) -> (u32, u32, Option<String>)
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    let counts_range = format!("HEAD...{upstream_ref}");
    let counts = run_git(&["rev-list", "--left-right", "--count", counts_range.as_str()])
        .ok()
        .map(|output| {
            let mut parts = output.split_whitespace();
            (parse_u32(parts.next()), parse_u32(parts.next()))
        });
    let merge_base_sha = match counts {
        Some((_, 0)) => Some(upstream_sha.to_string()),
        Some((0, _)) => Some(head_sha.to_string()),
        _ => merge_base(run_git, "HEAD", upstream_ref),
    };
    let (ahead, behind) = counts.unwrap_or((0, 0));
    (ahead, behind, merge_base_sha)
}

fn compute_worktree_state<F>(run_git: &F, scope: WorktreeStatusScope) -> WorktreeGitState
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    let Ok(output) = run_git(&["status", "--porcelain=1", scope.git_flag()]) else {
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

/// What to compute the state of.
///
/// `branch_name` is the local branch HEAD is expected to be on (drives
/// `expected_branch_matches`); `remote_branch` is the branch on `origin` the
/// checkout pushes to and is compared against (drives the fetch refspec and
/// `upstream`). They differ for a PR import whose local name was
/// de-duplicated (`feature-x-2` tracking `origin/feature-x`).
#[derive(Debug, Clone, Copy)]
pub struct BranchGitStateQuery<'a> {
    pub branch_name: &'a str,
    pub remote_branch: &'a str,
    pub base_branch: &'a str,
    pub fetch_mode: FetchMode,
    pub worktree_scope: WorktreeStatusScope,
}

/// Compute the git state of a checkout. See [`BranchGitStateQuery`] for the
/// `branch_name` / `remote_branch` split.
pub fn compute_branch_git_state<F>(
    cache_key: &str,
    run_git: F,
    branch_name: &str,
    remote_branch: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
    worktree_scope: WorktreeStatusScope,
) -> BranchGitState
where
    F: Fn(&[&str]) -> Result<String, String> + Sync,
{
    let query = BranchGitStateQuery {
        branch_name,
        remote_branch,
        base_branch,
        fetch_mode,
        worktree_scope,
    };
    compute_branch_git_state_with(cache_key, run_git, query, |_| ()).0
}

/// [`compute_branch_git_state`] with a companion read that needs the fork
/// point from `origin/<base>`.
///
/// `with_base_merge_base` runs on its own thread in the final phase, next to
/// the ahead/behind counts, and is handed `merge-base HEAD origin/<base>` as
/// this build computed it (`None` when there is none). The branch timeline
/// passes its `git log` here: the log starts at the same fork point the
/// `commitsSinceFork` count does, so the merge base is computed once and the
/// two reads overlap instead of running back to back.
///
/// Spawns, when nothing is fetched: `rev-parse`, `status`, `for-each-ref`
/// and `merge-base` in parallel, then `rev-list --count` for the base, the
/// upstream counts when the upstream exists (plus a second `merge-base` only
/// when the two have diverged), and the companion — two sequential spawns on
/// the critical path. With a fetch, the remote-ref reads wait for it and add
/// a phase.
pub fn compute_branch_git_state_with<F, T>(
    cache_key: &str,
    run_git: F,
    query: BranchGitStateQuery<'_>,
    with_base_merge_base: impl FnOnce(Option<&str>) -> T + Send,
) -> (BranchGitState, T)
where
    F: Fn(&[&str]) -> Result<String, String> + Sync,
    T: Send,
{
    let run_git = &run_git;
    let plan = plan_fetch(
        cache_key,
        query.remote_branch,
        query.base_branch,
        query.fetch_mode,
    );
    let upstream_ref_name = origin_ref_for_branch(query.remote_branch);
    let base_ref_name = origin_ref_for_branch(query.base_branch);
    let (upstream_ref, base_ref) = (upstream_ref_name.as_str(), base_ref_name.as_str());

    // Phase 1: the fetch (when planned) alongside the local reads that don't
    // depend on it. When nothing is fetched the remote-ref reads join this
    // phase too, since nothing could move the refs under them.
    let (refresh, head, worktree, remote_refs) = std::thread::scope(|s| {
        let fetch_handle = s.spawn(|| run_planned_fetch(cache_key, &plan, run_git));
        let head_handle = s.spawn(|| read_head(run_git));
        let worktree_handle = s.spawn(|| compute_worktree_state(run_git, query.worktree_scope));
        let refs_handle = (!plan.will_fetch()).then(|| {
            s.spawn(|| {
                read_remote_refs(
                    run_git,
                    upstream_ref,
                    base_ref,
                    plan.cached_upstream_known_missing(),
                )
            })
        });

        (
            fetch_handle.join().expect("fetch thread panicked"),
            head_handle.join().expect("head thread panicked"),
            worktree_handle.join().expect("worktree thread panicked"),
            refs_handle.map(|handle| handle.join().expect("remote refs thread panicked")),
        )
    });
    // Only after a fetch: the refs it may have moved.
    let remote_refs = remote_refs.unwrap_or_else(|| {
        read_remote_refs(
            run_git,
            upstream_ref,
            base_ref,
            refresh.upstream_known_missing,
        )
    });

    let detached_head = head.sha.is_some() && head.branch.is_none();
    let expected_branch = branch_name_without_origin(query.branch_name);
    let expected_branch_matches = head
        .branch
        .as_deref()
        .map(|current| current == expected_branch)
        .unwrap_or(false);

    // Phase 2: the counts the refs feed, in parallel with the companion.
    let base_merge_base = remote_refs.base_merge_base.as_deref();
    let upstream_pair = match (remote_refs.upstream_sha.as_deref(), head.sha.as_deref()) {
        (Some(upstream_sha), Some(head_sha)) => Some((upstream_sha, head_sha)),
        _ => None,
    };
    let (commits_since_fork, upstream_counts, behind_base, companion) = std::thread::scope(|s| {
        let base_handle = s.spawn(move || {
            base_merge_base
                .map(|mb| rev_count(run_git, &format!("{mb}..{base_ref}")))
                .unwrap_or(0)
        });
        let counts_handle = upstream_pair.map(|(upstream_sha, head_sha)| {
            s.spawn(move || {
                upstream_counts_and_merge_base(run_git, upstream_ref, upstream_sha, head_sha)
            })
        });
        // How many commits `origin/{base}` is ahead of `origin/{branch}`.
        // When the upstream ref *is* the base ref this is always 0, which
        // is the correct signal for "branch tracking its own base"
        // workflows.
        let behind_base_handle = (upstream_pair.is_some() && upstream_ref != base_ref)
            .then(|| s.spawn(move || rev_count(run_git, &format!("{upstream_ref}..{base_ref}"))));
        let companion_handle = s.spawn(move || with_base_merge_base(base_merge_base));

        (
            base_handle.join().expect("base count thread panicked"),
            counts_handle.map(|handle| handle.join().expect("upstream counts thread panicked")),
            behind_base_handle
                .map(|handle| handle.join().expect("behind-base thread panicked"))
                .unwrap_or(0),
            companion_handle.join().expect("companion thread panicked"),
        )
    });

    let upstream = match upstream_counts {
        Some((ahead, behind, merge_base_sha)) => UpstreamGitState {
            r#ref: upstream_ref_name,
            exists: true,
            sha: remote_refs.upstream_sha,
            relation: upstream_relation(ahead, behind),
            ahead,
            behind,
            merge_base_sha,
            behind_base,
        },
        None => UpstreamGitState {
            r#ref: upstream_ref_name,
            exists: remote_refs.upstream_sha.is_some(),
            sha: remote_refs.upstream_sha,
            relation: UpstreamRelation::Missing,
            ahead: 0,
            behind: 0,
            merge_base_sha: None,
            behind_base: 0,
        },
    };

    let state = BranchGitState {
        upstream,
        base: BaseGitState {
            r#ref: base_ref_name,
            sha: remote_refs.base_sha,
            commits_since_fork,
        },
        worktree,
        fetch: refresh.fetch,
        head_sha: head.sha,
        current_branch: head.branch,
        detached_head,
        expected_branch_matches,
    };
    (state, companion)
}

/// See [`BranchGitStateQuery`] for the `branch_name` / `remote_branch` split.
///
/// `env_source` is `Lite` or `Captured`. A `CapturedBounded` build goes
/// through [`compute_local_branch_git_state_bounded`], since a read that only
/// failed because the capture wasn't ready would read here as an answer.
pub fn compute_local_branch_git_state(
    repo: &Path,
    branch_name: &str,
    remote_branch: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
    worktree_scope: WorktreeStatusScope,
    env_source: EnvSource,
) -> BranchGitState {
    let query = BranchGitStateQuery {
        branch_name,
        remote_branch,
        base_branch,
        fetch_mode,
        worktree_scope,
    };
    compute_local_branch_git_state_with(repo, query, env_source, |_| ()).0
}

/// [`compute_branch_git_state_with`] against a local checkout. `env_source`
/// is `Lite` or `Captured`, as for [`compute_local_branch_git_state`].
pub fn compute_local_branch_git_state_with<T: Send>(
    repo: &Path,
    query: BranchGitStateQuery<'_>,
    env_source: EnvSource,
    with_base_merge_base: impl FnOnce(Option<&str>) -> T + Send,
) -> (BranchGitState, T) {
    debug_assert!(
        !matches!(env_source, EnvSource::CapturedBounded),
        "use compute_local_branch_git_state_bounded"
    );
    compute_branch_git_state_with(
        &local_cache_key(repo, &query),
        |args| local_git(repo, env_source, args).map_err(|e| e.to_string()),
        query,
        with_base_merge_base,
    )
}

/// [`compute_local_branch_git_state`] for a display refresh, with
/// [`EnvSource::CapturedBounded`] reads: they wait at most
/// [`cli::run_bounded`]'s bound for the captured env, while the fetch waits
/// for it however long it takes.
///
/// A read that gave up on the capture and then failed in a way the captured
/// env might have avoided ([`GitError::CapturedEnvPending`]) makes this an
/// `Err` instead of a state. The build would read a failed `git status` as a
/// clean tree and a failed count as zero: during a cold capture, a checkout
/// with LFS files and git-lfs missing from the inherited PATH would show no
/// uncommitted changes and offer Pull. The caller keeps the state it has.
/// `refresh_branch_git_state` then queues one follow-up refresh on the
/// capture (`timeline::git_state_follow_up`), which runs once it lands and
/// gets the real state. Nothing retries here: a retry with the captured env
/// would wait out the capture this build exists not to wait for, and a
/// second bounded read of the same capture returns at once without it.
pub fn compute_local_branch_git_state_bounded(
    repo: &Path,
    branch_name: &str,
    remote_branch: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
    worktree_scope: WorktreeStatusScope,
) -> Result<BranchGitState, GitError> {
    let query = BranchGitStateQuery {
        branch_name,
        remote_branch,
        base_branch,
        fetch_mode,
        worktree_scope,
    };
    compute_branch_git_state_settled(
        &local_cache_key(repo, &query),
        |args| local_git(repo, EnvSource::CapturedBounded, args),
        query,
    )
}

/// Fetch-TTL cache key for a local checkout's state builds.
fn local_cache_key(repo: &Path, query: &BranchGitStateQuery<'_>) -> String {
    format!(
        "local:{}:{}:{}",
        repo.display(),
        query.branch_name,
        query.base_branch
    )
}

/// Run one of a state build's git ops in `repo` with `env_source`'s env.
fn local_git(repo: &Path, env_source: EnvSource, args: &[&str]) -> Result<String, GitError> {
    match env_source {
        EnvSource::Lite => cli::run_smart(repo, args),
        // Every op the build issues reads refs, the index or the work tree,
        // except the fetch: that one goes through the user's credential
        // helpers and ssh config, so it waits for the capture however long
        // it takes.
        EnvSource::CapturedBounded if args.first() != Some(&"fetch") => {
            cli::run_bounded(repo, args)
        }
        EnvSource::CapturedBounded | EnvSource::Captured => cli::run(repo, args),
    }
}

/// [`compute_branch_git_state`] over a runner whose reads may fail with
/// [`GitError::CapturedEnvPending`]. Any such read makes the build an `Err`
/// carrying the first one's error, rather than a state that took the failure
/// for an answer. Other failures are read as the build always reads them.
pub(super) fn compute_branch_git_state_settled<F>(
    cache_key: &str,
    run_git: F,
    query: BranchGitStateQuery<'_>,
) -> Result<BranchGitState, GitError>
where
    F: Fn(&[&str]) -> Result<String, GitError> + Sync,
{
    let pending = OnceLock::new();
    let (state, ()) = compute_branch_git_state_with(
        cache_key,
        |args| {
            run_git(args).map_err(|error| {
                let message = error.to_string();
                if matches!(error, GitError::CapturedEnvPending(_)) {
                    let _ = pending.set(error);
                }
                message
            })
        },
        query,
        |_| (),
    );
    match pending.into_inner() {
        Some(error) => Err(error),
        None => Ok(state),
    }
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
///   $2 = base_refspec (`+refs/heads/<base>:refs/remotes/origin/<base>`)
///   $3 = local branch name, whose upstream the script resolves itself
///   $4 = short base branch name (never a push target)
///   $5 = base_ref (`origin/<base>`)
///   $6 = "skip_fetch" to skip the fetch phase (cache fresh)
///   $7 = "uno" (no untracked enumeration) or "uall" (full enumeration)
///   $8 = "upstream_missing" when the fetch cache already knows the branch
///        is not on origin, so upstream state is skipped
///   $9 = the `git log --format=…` argument to print the commits since the
///        fork point with, after the user identity; empty to print neither
const BATCH_GIT_STATE_SCRIPT: &str = concat!(
    "cd \"$1\" || exit 1\n",
    // --- Which branch on origin $3 pushes to ---
    // The same decision as `upstream_branch_name_from_config`, which
    // re-derives it from UP_REMOTE/UP_MERGE on the Rust side: an upstream on
    // origin is the remote branch, unless it is the base ($4); anything else
    // (none, or on another remote) means the branch's own name. The config
    // is read directly rather than via `@{upstream}`, which git only
    // resolves when a fetch refspec maps the branch.
    "up_remote=$(git config --get \"branch.$3.remote\" 2>/dev/null || true)\n",
    "up_merge=$(git config --get \"branch.$3.merge\" 2>/dev/null || true)\n",
    "printf 'UP_REMOTE=%s\\n' \"$up_remote\"\n",
    "printf 'UP_MERGE=%s\\n' \"$up_merge\"\n",
    "remote_branch=\"$3\"\n",
    "if [ \"$up_remote\" = origin ]; then\n",
    "  case \"$up_merge\" in\n",
    "    refs/heads/?*) remote_branch=${up_merge#refs/heads/} ;;\n",
    "  esac\n",
    "fi\n",
    "if [ \"$remote_branch\" = \"$4\" ]; then remote_branch=\"$3\"; fi\n",
    "up_ref=\"origin/$remote_branch\"\n",
    // The fetch cache already knows the upstream is missing: skip resolving it.
    "if [ \"$8\" = 'upstream_missing' ]; then up_ref=''; fi\n",
    "if [ \"$remote_branch\" != \"$4\" ]; then\n",
    "  branch_refspec=\"+refs/heads/$remote_branch:refs/remotes/origin/$remote_branch\"\n",
    "else\n",
    "  branch_refspec=''\n",
    "fi\n",
    // --- Fetch phase (conditional) ---
    "fetch_err=''\n",
    "upstream_missing=''\n",
    "if [ \"$6\" != 'skip_fetch' ]; then\n",
    "  _ferr=$(mktemp) || exit 1\n",
    "  trap 'rm -f \"$_ferr\"' EXIT\n",
    "  if [ -n \"$branch_refspec\" ]; then\n",
    "    if ! git fetch --prune origin \"$2\" \"$branch_refspec\" 2>\"$_ferr\"; then\n",
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
    "if [ \"$7\" = 'uno' ]; then ut_flag='--untracked-files=no'; else ut_flag='--untracked-files=all'; fi\n",
    "echo STATUS_START\n",
    "git status --porcelain=1 \"$ut_flag\" 2>/dev/null || true\n",
    "echo STATUS_END\n",
    "[ -n \"$upstream_missing\" ] && echo 'UPSTREAM_MISSING=true'\n",
    "[ -n \"$fetch_err\" ] && printf 'FETCH_ERR=%s\\n' \"$fetch_err\"\n",
    // --- Upstream state (skip if up_ref is empty) ---
    "if [ -n \"$up_ref\" ] && [ -z \"$upstream_missing\" ]; then\n",
    "  up_sha=$(git rev-parse --verify \"$up_ref\" 2>/dev/null || true)\n",
    "else\n",
    "  up_sha=''\n",
    "fi\n",
    "printf 'UP_SHA=%s\\n' \"$up_sha\"\n",
    "if [ -n \"$up_sha\" ] && [ -n \"$head_sha\" ]; then\n",
    "  printf 'UP_COUNTS=%s\\n' \"$(git rev-list --left-right --count \"$head_sha\"...\"$up_ref\" 2>/dev/null || echo '0 0')\"\n",
    "  printf 'UP_MB=%s\\n' \"$(git merge-base \"$head_sha\" \"$up_ref\" 2>/dev/null || true)\"\n",
    "  if [ \"$up_ref\" != \"$5\" ]; then\n",
    "    printf 'UP_BEHIND_BASE=%s\\n' \"$(git rev-list --count \"$up_ref\"..\"$5\" 2>/dev/null || echo 0)\"\n",
    "  else\n",
    "    printf 'UP_BEHIND_BASE=0\\n'\n",
    "  fi\n",
    "fi\n",
    // --- Base state ---
    "mb=''\n",
    "base_sha=$(git rev-parse --verify \"$5\" 2>/dev/null || true)\n",
    "printf 'BASE_SHA=%s\\n' \"$base_sha\"\n",
    "if [ -n \"$base_sha\" ] && [ -n \"$head_sha\" ]; then\n",
    "  mb=$(git merge-base \"$head_sha\" \"$5\" 2>/dev/null || true)\n",
    "  printf 'BASE_MB=%s\\n' \"$mb\"\n",
    "  if [ -n \"$mb\" ]; then\n",
    "    printf 'BASE_BEHIND=%s\\n' \"$(git rev-list --count \"$mb\"..\"$5\" 2>/dev/null || echo 0)\"\n",
    "  fi\n",
    "fi\n",
    // --- Identity and commit log (only when $9 names a log format) ---
    // The log reuses the merge base above and goes last: everything after
    // LOG_START is one commit per line, so a subject that reads like one of
    // the keys above cannot be mistaken for one. A failed `git log` (no
    // base, no HEAD) is reported through LOG_OK rather than by exiting.
    "if [ -n \"$9\" ]; then\n",
    "  printf 'USER_NAME=%s\\n' \"$(git config user.name 2>/dev/null || true)\"\n",
    "  printf 'USER_EMAIL=%s\\n' \"$(git config user.email 2>/dev/null || true)\"\n",
    "  if [ -n \"$mb\" ]; then log_range=\"$mb..HEAD\"; else log_range=\"$5..HEAD\"; fi\n",
    "  if log_out=$(git log \"$9\" \"$log_range\" 2>/dev/null); then log_ok=true; else log_ok=''; fi\n",
    "  printf 'LOG_OK=%s\\n' \"$log_ok\"\n",
    "  echo LOG_START\n",
    "  [ -n \"$log_out\" ] && printf '%s\\n' \"$log_out\"\n",
    "fi\n",
    "exit 0\n",
);

/// Parsed output from the combined git state script.
struct BatchGitStateOutput {
    /// `branch.<name>.remote` and `branch.<name>.merge`, `None` when unset.
    upstream_remote: Option<String>,
    upstream_merge: Option<String>,
    head_sha: Option<String>,
    branch: Option<String>,
    status_lines: String,
    upstream_missing: bool,
    fetch_error: Option<String>,
    up_sha: Option<String>,
    up_ahead: u32,
    up_behind: u32,
    up_merge_base: Option<String>,
    up_behind_base: u32,
    base_sha: Option<String>,
    base_behind: u32,
    /// `user.name` / `user.email`, when the script was asked for them.
    user_name: Option<String>,
    user_email: Option<String>,
    /// Whether `git log` ran and succeeded; `None` when the script was not
    /// asked for a log.
    log_ok: Option<bool>,
    /// The log lines, newest first, as `git log` printed them.
    log_lines: Vec<String>,
}

fn non_empty_value(raw: &str) -> Option<String> {
    let value = raw.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn parse_batch_git_state_output(raw: &str) -> BatchGitStateOutput {
    let mut out = BatchGitStateOutput {
        upstream_remote: None,
        upstream_merge: None,
        head_sha: None,
        branch: None,
        status_lines: String::new(),
        upstream_missing: false,
        fetch_error: None,
        up_sha: None,
        up_ahead: 0,
        up_behind: 0,
        up_merge_base: None,
        up_behind_base: 0,
        base_sha: None,
        base_behind: 0,
        user_name: None,
        user_email: None,
        log_ok: None,
        log_lines: Vec::new(),
    };
    let mut in_status = false;

    let mut lines = raw.lines();
    for line in lines.by_ref() {
        if line == "STATUS_START" {
            in_status = true;
            continue;
        }
        if line == "STATUS_END" {
            in_status = false;
            continue;
        }
        if in_status {
            if !out.status_lines.is_empty() {
                out.status_lines.push('\n');
            }
            out.status_lines.push_str(line);
            continue;
        }
        // Everything after LOG_START is the log, one commit per line. The
        // script puts it last so a subject that happens to read like one of
        // the keys below cannot be taken for one.
        if line == "LOG_START" {
            break;
        }
        if let Some(val) = line.strip_prefix("UP_REMOTE=") {
            out.upstream_remote = non_empty_value(val);
        } else if let Some(val) = line.strip_prefix("UP_MERGE=") {
            out.upstream_merge = non_empty_value(val);
        } else if let Some(val) = line.strip_prefix("HEAD=") {
            out.head_sha = non_empty_value(val);
        } else if let Some(val) = line.strip_prefix("BRANCH=") {
            out.branch = non_empty_value(val);
        } else if line.starts_with("UPSTREAM_MISSING=") {
            out.upstream_missing = true;
        } else if let Some(val) = line.strip_prefix("FETCH_ERR=") {
            out.fetch_error = non_empty_value(val);
        } else if let Some(val) = line.strip_prefix("UP_SHA=") {
            out.up_sha = non_empty_value(val);
        } else if let Some(val) = line.strip_prefix("UP_COUNTS=") {
            let mut parts = val.split_whitespace();
            out.up_ahead = parse_u32(parts.next());
            out.up_behind = parse_u32(parts.next());
        } else if let Some(val) = line.strip_prefix("UP_MB=") {
            out.up_merge_base = non_empty_value(val);
        } else if let Some(val) = line.strip_prefix("UP_BEHIND_BASE=") {
            out.up_behind_base = parse_u32(Some(val.trim()));
        } else if let Some(val) = line.strip_prefix("BASE_SHA=") {
            out.base_sha = non_empty_value(val);
        } else if line.starts_with("BASE_MB=") {
            // Used internally by the script to compute BASE_BEHIND and to
            // start the log.
        } else if let Some(val) = line.strip_prefix("BASE_BEHIND=") {
            out.base_behind = parse_u32(Some(val.trim()));
        } else if let Some(val) = line.strip_prefix("USER_NAME=") {
            out.user_name = non_empty_value(val);
        } else if let Some(val) = line.strip_prefix("USER_EMAIL=") {
            out.user_email = non_empty_value(val);
        } else if let Some(val) = line.strip_prefix("LOG_OK=") {
            out.log_ok = Some(val.trim() == "true");
        }
    }
    out.log_lines = lines
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();

    out
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

/// Everything the branch timeline needs from a Blox workspace, from one
/// round trip: the git state, the user's identity and the commit log.
pub struct BatchedBranchGitSnapshot {
    pub state: BranchGitState,
    /// `user.name` / `user.email` as the workspace resolves them; both `None`
    /// when the script did not run.
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    /// [`BRANCH_COMMIT_LOG_FORMAT`] lines, newest first, for the commits on
    /// HEAD since it forked from `origin/<base>` — the same fork point
    /// `base.commits_since_fork` counts from. `Err` when the script did not
    /// run or `git log` failed inside it.
    pub commit_lines: Result<Vec<String>, String>,
}

struct BatchedQuery<'a> {
    repo_path: &'a str,
    branch_name: &'a str,
    base_branch: &'a str,
    fetch_mode: FetchMode,
    worktree_scope: WorktreeStatusScope,
    /// The `git log --format=…` argument to run after the state, or `None`
    /// to leave identity and log out of the script.
    log_format: Option<&'a str>,
}

/// Compute branch git state using a single batched shell script.
///
/// This is the remote-optimised counterpart of `compute_branch_git_state`.
/// Instead of many individual `run_git` calls (each a separate `ws_exec`
/// round-trip), it bundles all commands into one shell script that resolves
/// which branch on origin `branch_name` pushes to (its upstream, see
/// [`super::upstream_branch_name`]), performs fetch (when needed), local state
/// collection, and ref comparisons — giving **1 round-trip total** regardless
/// of whether fetch is cached or not.
///
/// Unlike [`compute_branch_git_state`] there is no `remote_branch` argument:
/// the script reads the upstream itself, and the Rust side re-derives the same
/// answer from the `UP_REMOTE`/`UP_MERGE` lines for the returned `upstream.ref`.
pub fn compute_branch_git_state_batched<F>(
    cache_key: &str,
    run_script: F,
    repo_path: &str,
    branch_name: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
    worktree_scope: WorktreeStatusScope,
) -> BranchGitState
where
    F: Fn(&str, &[&str]) -> Result<String, String>,
{
    compute_batched(
        cache_key,
        run_script,
        BatchedQuery {
            repo_path,
            branch_name,
            base_branch,
            fetch_mode,
            worktree_scope,
            log_format: None,
        },
    )
    .state
}

/// [`compute_branch_git_state_batched`] plus the user identity and the commit
/// log the branch timeline shows, still in one round trip.
///
/// On a Blox workspace every `ws_exec` costs about three seconds of transport
/// while all the git work inside it takes tens of milliseconds, so the
/// timeline's five execs per build (three once the identity was cached) were
/// almost entirely round-trip time. The script now also prints `user.name`,
/// `user.email` and `git log <format> <merge-base>..HEAD`, reusing the
/// merge base it already computed for `commits_since_fork`.
pub fn compute_branch_git_snapshot_batched<F>(
    cache_key: &str,
    run_script: F,
    repo_path: &str,
    branch_name: &str,
    base_branch: &str,
    fetch_mode: FetchMode,
    worktree_scope: WorktreeStatusScope,
) -> BatchedBranchGitSnapshot
where
    F: Fn(&str, &[&str]) -> Result<String, String>,
{
    compute_batched(
        cache_key,
        run_script,
        BatchedQuery {
            repo_path,
            branch_name,
            base_branch,
            fetch_mode,
            worktree_scope,
            log_format: Some(BRANCH_COMMIT_LOG_FORMAT),
        },
    )
}

fn compute_batched<F>(
    cache_key: &str,
    run_script: F,
    query: BatchedQuery<'_>,
) -> BatchedBranchGitSnapshot
where
    F: Fn(&str, &[&str]) -> Result<String, String>,
{
    let base_refspec = refspec_for(query.base_branch);
    let base_ref = origin_ref_for_branch(query.base_branch);
    let base_short = branch_name_without_origin(query.base_branch);
    let expected_branch = branch_name_without_origin(query.branch_name);

    // Check fetch cache to decide whether we need the fetch phase.
    let now = now_ms();
    let previous = fetch_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(cache_key).cloned());

    let should_fetch = match (query.fetch_mode, &previous) {
        (FetchMode::Never, _) => false,
        (FetchMode::Force, _) => true,
        (FetchMode::Ttl, Some(entry)) => now.saturating_sub(entry.fetched_at) > FETCH_TTL_MS,
        (FetchMode::Ttl, None) => true,
    };

    // When cache says upstream is missing, tell the script to skip upstream
    // resolution.
    let cached_upstream_missing = previous
        .as_ref()
        .map(|e| e.upstream_known_missing)
        .unwrap_or(false);
    let upstream_missing_arg = if !should_fetch && cached_upstream_missing {
        "upstream_missing"
    } else {
        ""
    };
    let skip_fetch_arg = if should_fetch { "" } else { "skip_fetch" };

    let raw = run_script(
        BATCH_GIT_STATE_SCRIPT,
        &[
            query.repo_path,
            &base_refspec,
            expected_branch,
            base_short,
            &base_ref,
            skip_fetch_arg,
            query.worktree_scope.script_arg(),
            upstream_missing_arg,
            query.log_format.unwrap_or(""),
        ],
    );

    match raw {
        Ok(output) => {
            let parsed = parse_batch_git_state_output(&output);
            let upstream_ref = origin_ref_for_branch(&upstream_branch_name_from_config(
                parsed.upstream_remote.as_deref(),
                parsed.upstream_merge.as_deref(),
                expected_branch,
                query.base_branch,
            ));

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
                upstream_relation(parsed.up_ahead, parsed.up_behind)
            };

            let upstream = UpstreamGitState {
                r#ref: upstream_ref,
                exists: up_exists,
                sha: parsed.up_sha,
                relation,
                ahead: parsed.up_ahead,
                behind: parsed.up_behind,
                merge_base_sha: parsed.up_merge_base,
                behind_base: parsed.up_behind_base,
            };

            let base = BaseGitState {
                r#ref: base_ref,
                sha: parsed.base_sha,
                commits_since_fork: parsed.base_behind,
            };

            let commit_lines = match parsed.log_ok {
                Some(true) => Ok(parsed.log_lines),
                Some(false) => Err("git log failed in the workspace".to_string()),
                None => Err("the workspace script produced no commit log".to_string()),
            };

            BatchedBranchGitSnapshot {
                state: BranchGitState {
                    upstream,
                    base,
                    worktree,
                    fetch: fetch_state,
                    head_sha: parsed.head_sha,
                    current_branch: parsed.branch,
                    detached_head,
                    expected_branch_matches,
                },
                user_name: parsed.user_name,
                user_email: parsed.user_email,
                commit_lines,
            }
        }
        Err(err) => {
            // Script execution itself failed; the upstream could not be read
            // either, so name the branch's own remote-tracking ref.
            let fetched_at = previous.as_ref().map(|e| e.fetched_at);
            let state = BranchGitState {
                upstream: UpstreamGitState {
                    r#ref: origin_ref_for_branch(expected_branch),
                    exists: false,
                    sha: None,
                    relation: UpstreamRelation::Missing,
                    ahead: 0,
                    behind: 0,
                    merge_base_sha: None,
                    behind_base: 0,
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
                        error: Some(err.clone()),
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
            };
            BatchedBranchGitSnapshot {
                state,
                user_name: None,
                user_email: None,
                commit_lines: Err(err),
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

/// Update the repo-level fetch cache for a given repo path.
///
/// Called by the background sync service after a successful `git fetch` so
/// that the existing `FetchMode::Ttl` checks see the repo as fresh and
/// avoid redundant fetches when the user navigates to a project.
pub fn update_repo_fetch_cache(repo_path: &Path) {
    let now = now_ms();
    let repo_key = format!("local:{}", repo_path.display());
    if let Ok(mut cache) = repo_fetch_cache().lock() {
        let entry = cache.entry(repo_key).or_insert_with(|| RepoFetchEntry {
            fetched_at: now,
            fetched_refspecs: HashSet::new(),
        });
        entry.fetched_at = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{run_git_in, TempDir, TempGitRepo};

    /// Drives [`BATCH_GIT_STATE_SCRIPT`] through a local `sh`, shaped the way
    /// `branches::run_workspace_shell` runs it on Blox (`sh -c <script> _
    /// <args…>`).
    fn run_batch_script_locally(script: &str, args: &[&str]) -> Result<String, String> {
        let mut command = std::process::Command::new("sh");
        command.arg("-c").arg(script).arg("_").args(args);
        crate::git::strip_git_env(&mut command);
        let output = command.output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn batched_state(repo: &Path, branch: &str, fetch_mode: FetchMode) -> BranchGitState {
        let repo_str = repo.to_str().unwrap();
        compute_branch_git_state_batched(
            &format!("{repo_str}#{branch}#{fetch_mode:?}"),
            run_batch_script_locally,
            repo_str,
            branch,
            "main",
            fetch_mode,
            WorktreeStatusScope::Full,
        )
    }

    /// The batched script resolves the upstream itself, in the one
    /// round-trip: a de-duplicated `feature-x-2` tracking `origin/feature-x`
    /// is fetched and compared as `feature-x`; a branch tracking its base, or
    /// nothing, as its own name — the same answers as
    /// `upstream_branch_name_from_config`.
    #[test]
    fn batched_state_reads_the_upstream_inside_the_script() {
        let seed = TempGitRepo::new();
        seed.write_file("file.txt", "base\n");
        seed.commit("base");
        seed.run_git(&["checkout", "-b", "feature-x"]);
        seed.write_file("file.txt", "base\nfeature\n");
        seed.commit("feature");
        let origin = TempDir::new("origin");
        let origin_str = origin.path().to_str().unwrap();
        run_git_in(seed.path(), &["clone", "--bare", ".", origin_str]);
        let clone = TempDir::new("clone");
        run_git_in(
            origin.path(),
            &["clone", origin_str, clone.path().to_str().unwrap()],
        );
        let repo = clone.path();
        run_git_in(
            repo,
            &[
                "checkout",
                "--no-track",
                "-b",
                "feature-x-2",
                "origin/feature-x",
            ],
        );
        run_git_in(repo, &["branch", "-u", "origin/feature-x", "feature-x-2"]);

        let state = batched_state(repo, "feature-x-2", FetchMode::Never);
        assert_eq!(state.upstream.r#ref, "origin/feature-x");
        assert!(state.upstream.exists);
        assert!(matches!(state.upstream.relation, UpstreamRelation::InSync));
        assert_eq!(state.current_branch.as_deref(), Some("feature-x-2"));
        assert!(state.expected_branch_matches);

        // origin/feature-x moves on: the fetch has to bring in `feature-x`,
        // not look for a `feature-x-2` that origin does not have.
        seed.write_file("file.txt", "base\nfeature\nmore\n");
        let new_tip = seed.commit("more");
        seed.run_git(&["push", origin_str, "feature-x"]);
        let state = batched_state(repo, "feature-x-2", FetchMode::Force);
        assert!(
            matches!(state.fetch.status, FetchStatus::Fresh),
            "{:?}",
            state.fetch.error
        );
        assert_eq!(state.upstream.r#ref, "origin/feature-x");
        assert_eq!(state.upstream.sha.as_deref(), Some(new_tip.trim()));
        assert!(matches!(
            state.upstream.relation,
            UpstreamRelation::OriginAhead
        ));
        assert_eq!((state.upstream.ahead, state.upstream.behind), (0, 1));
        assert_eq!(
            run_git_in(repo, &["rev-parse", "origin/feature-x"]),
            new_tip.trim()
        );

        // Tracking the base is not a push target: compared as its own name.
        run_git_in(repo, &["branch", "--track", "feature-new", "origin/main"]);
        let state = batched_state(repo, "feature-new", FetchMode::Never);
        assert_eq!(state.upstream.r#ref, "origin/feature-new");
        assert!(!state.upstream.exists);
        assert!(matches!(state.upstream.relation, UpstreamRelation::Missing));

        // No upstream at all: the same.
        run_git_in(
            repo,
            &["branch", "--no-track", "feature-loose", "origin/main"],
        );
        let state = batched_state(repo, "feature-loose", FetchMode::Never);
        assert_eq!(state.upstream.r#ref, "origin/feature-loose");
        assert!(!state.upstream.exists);
    }

    fn batched_snapshot(repo: &Path, branch: &str) -> BatchedBranchGitSnapshot {
        let repo_str = repo.to_str().unwrap();
        compute_branch_git_snapshot_batched(
            &format!("{repo_str}#{branch}#snapshot"),
            run_batch_script_locally,
            repo_str,
            branch,
            "main",
            FetchMode::Never,
            WorktreeStatusScope::Full,
        )
    }

    fn log_shas(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                crate::git::parse_branch_commit_line(line)
                    .unwrap_or_else(|| panic!("not a commit line: {line:?}"))
                    .sha
                    .to_string()
            })
            .collect()
    }

    /// The one round trip also carries the identity and the commit log, the
    /// log starting at the merge base the state already computed — and a
    /// subject that spells the delimiter is still just a subject.
    #[test]
    fn batched_snapshot_carries_identity_and_commit_log() {
        let repo = TempGitRepo::new();
        repo.write_file("file.txt", "base\n");
        let base = repo.commit("base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &base]);
        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("file.txt", "base\none\n");
        let first = repo.commit("feat: one");
        repo.write_file("file.txt", "base\none\ntwo\n");
        let second = repo.commit("LOG_START");

        let snapshot = batched_snapshot(repo.path(), "feature");
        assert_eq!(snapshot.user_name.as_deref(), Some("Test"));
        assert_eq!(snapshot.user_email.as_deref(), Some("test@example.com"));
        let lines = snapshot.commit_lines.unwrap();
        assert_eq!(log_shas(&lines), vec![second.clone(), first.clone()]);
        assert_eq!(
            crate::git::parse_branch_commit_line(&lines[0])
                .unwrap()
                .subject,
            "LOG_START"
        );
        assert_eq!(snapshot.state.head_sha.as_deref(), Some(second.as_str()));
        assert_eq!(snapshot.state.base.sha.as_deref(), Some(base.as_str()));
        assert_eq!(snapshot.state.base.commits_since_fork, 0);
        assert!(!snapshot.state.upstream.exists);

        // The base moves on: the count sees its new commit, the log still
        // starts at the fork point.
        repo.run_git(&["checkout", "main"]);
        repo.write_file("other.txt", "moved\n");
        let moved = repo.commit("base moved");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &moved]);
        repo.run_git(&["checkout", "feature"]);
        let snapshot = batched_snapshot(repo.path(), "feature");
        assert_eq!(snapshot.state.base.commits_since_fork, 1);
        assert_eq!(
            log_shas(&snapshot.commit_lines.unwrap()),
            vec![second, first]
        );
    }

    /// Zero commits ahead is an empty, successful log. A missing base is a
    /// log that could not run, which the snapshot reports rather than guesses.
    #[test]
    fn batched_snapshot_handles_no_commits_and_no_base() {
        let repo = TempGitRepo::new();
        repo.write_file("file.txt", "base\n");
        let base = repo.commit("base");
        repo.run_git(&["checkout", "-b", "feature"]);

        // No origin/main yet.
        let snapshot = batched_snapshot(repo.path(), "feature");
        assert!(snapshot.state.base.sha.is_none());
        assert!(
            snapshot.commit_lines.is_err(),
            "{:?}",
            snapshot.commit_lines
        );
        assert_eq!(snapshot.user_name.as_deref(), Some("Test"));

        repo.run_git(&["update-ref", "refs/remotes/origin/main", &base]);
        let snapshot = batched_snapshot(repo.path(), "feature");
        assert_eq!(snapshot.commit_lines.unwrap(), Vec::<String>::new());
        assert_eq!(snapshot.state.base.sha.as_deref(), Some(base.as_str()));
        assert_eq!(snapshot.state.head_sha.as_deref(), Some(base.as_str()));
        assert_eq!(snapshot.state.base.commits_since_fork, 0);
    }

    /// The state-only entry point leaves `$9` empty so the refresh path's
    /// script prints exactly what it did before; the snapshot passes the log
    /// format through.
    #[test]
    fn only_the_snapshot_asks_the_script_for_a_log() {
        let state_calls = std::cell::RefCell::new(Vec::new());
        let state = compute_branch_git_state_batched(
            "state-only",
            |_script, args| {
                state_calls
                    .borrow_mut()
                    .push(args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
                Ok("HEAD=abc\nBRANCH=feature\nSTATUS_START\nSTATUS_END\nBASE_SHA=def\n".into())
            },
            ".",
            "feature",
            "main",
            FetchMode::Never,
            WorktreeStatusScope::Indexed,
        );
        assert_eq!(state.head_sha.as_deref(), Some("abc"));
        let args = state_calls.borrow();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].len(), 9);
        assert_eq!(args[0][5], "skip_fetch");
        assert_eq!(args[0][8], "");

        let snapshot_calls = std::cell::RefCell::new(Vec::new());
        let snapshot = compute_branch_git_snapshot_batched(
            "snapshot",
            |_script, args| {
                snapshot_calls
                    .borrow_mut()
                    .push(args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
                Err("ssh: connection closed".into())
            },
            ".",
            "feature",
            "main",
            FetchMode::Never,
            WorktreeStatusScope::Indexed,
        );
        assert_eq!(snapshot_calls.borrow()[0][8], BRANCH_COMMIT_LOG_FORMAT);
        assert_eq!(snapshot_calls.borrow()[0][5], "skip_fetch");
        assert_eq!(
            snapshot.commit_lines,
            Err("ssh: connection closed".to_string())
        );
        assert!(snapshot.user_name.is_none());
        assert!(snapshot.state.head_sha.is_none());
    }

    /// The fetch cache already knows the branch is not on origin: the script
    /// is told to skip the upstream (`upstream_missing`), and the identity
    /// and commit log still ride along in the same round trip.
    #[test]
    fn batched_snapshot_with_a_known_missing_upstream_still_carries_the_log() {
        let repo = TempGitRepo::new();
        repo.write_file("file.txt", "base\n");
        let base = repo.commit("base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &base]);
        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("file.txt", "base\none\n");
        let first = repo.commit("feat: one");
        repo.write_file("file.txt", "base\none\ntwo\n");
        let second = repo.commit("feat: two");
        // origin/feature does exist here, so only the cached verdict can
        // make the script leave it alone.
        repo.run_git(&["update-ref", "refs/remotes/origin/feature", &second]);
        let repo_str = repo.path().to_str().unwrap();

        let key = format!("{repo_str}#feature#known-missing");
        fetch_cache().lock().unwrap().insert(
            key.clone(),
            FetchCacheEntry {
                fetched_at: now_ms(),
                upstream_known_missing: true,
            },
        );
        let snapshot = compute_branch_git_snapshot_batched(
            &key,
            run_batch_script_locally,
            repo_str,
            "feature",
            "main",
            FetchMode::Never,
            WorktreeStatusScope::Full,
        );
        assert!(!snapshot.state.upstream.exists);
        assert!(snapshot.state.upstream.sha.is_none());
        assert!(matches!(
            snapshot.state.upstream.relation,
            UpstreamRelation::Missing
        ));
        assert!(matches!(snapshot.state.fetch.status, FetchStatus::Fresh));
        assert_eq!(snapshot.user_name.as_deref(), Some("Test"));
        assert_eq!(snapshot.user_email.as_deref(), Some("test@example.com"));
        assert_eq!(
            log_shas(&snapshot.commit_lines.unwrap()),
            vec![second.clone(), first.clone()]
        );
        assert_eq!(snapshot.state.base.sha.as_deref(), Some(base.as_str()));
        assert_eq!(snapshot.state.head_sha.as_deref(), Some(second.as_str()));
        assert_eq!(snapshot.state.base.commits_since_fork, 0);

        // Without the cached verdict the same repo reads its upstream.
        let plain = batched_snapshot(repo.path(), "feature");
        assert!(plain.state.upstream.exists);
        assert!(matches!(
            plain.state.upstream.relation,
            UpstreamRelation::InSync
        ));
        assert_eq!(log_shas(&plain.commit_lines.unwrap()), vec![second, first]);
    }

    /// The upstream merge base is derived from the `rev-list` counts; when
    /// that count fails there is nothing to derive it from, so the counts
    /// read as zero and git is asked for the merge base — `None` when it
    /// cannot answer that either.
    #[test]
    fn failed_upstream_counts_fall_back_to_a_merge_base_spawn() {
        let calls = std::sync::Mutex::new(Vec::new());
        let failing_counts = |args: &[&str]| -> Result<String, String> {
            calls.lock().unwrap().push(args.join(" "));
            match args[0] {
                "rev-list" => Err("fatal: bad revision 'HEAD...origin/feature'".to_string()),
                "merge-base" => Ok("3333\n".to_string()),
                other => panic!("unexpected git {other}"),
            }
        };
        assert_eq!(
            upstream_counts_and_merge_base(&failing_counts, "origin/feature", "2222", "1111"),
            (0, 0, Some("3333".to_string()))
        );
        assert_eq!(
            calls.into_inner().unwrap(),
            vec![
                "rev-list --left-right --count HEAD...origin/feature",
                "merge-base HEAD origin/feature"
            ]
        );

        let nothing_works = |_: &[&str]| -> Result<String, String> { Err("boom".to_string()) };
        assert_eq!(
            upstream_counts_and_merge_base(&nothing_works, "origin/feature", "2222", "1111"),
            (0, 0, None)
        );

        // For contrast, a count that settles the relation needs no second spawn.
        let behind = |args: &[&str]| -> Result<String, String> {
            assert_eq!(args[0], "rev-list", "{args:?}");
            Ok("0\t2\n".to_string())
        };
        assert_eq!(
            upstream_counts_and_merge_base(&behind, "origin/feature", "2222", "1111"),
            (0, 2, Some("1111".to_string()))
        );
    }

    const FULL_SNAPSHOT_OUTPUT: &str = "UP_REMOTE=origin\nUP_MERGE=refs/heads/feature\nHEAD=1111\nBRANCH=feature\nSTATUS_START\n M file.txt\nSTATUS_END\nUP_SHA=2222\nUP_COUNTS=1\t2\nUP_MB=3333\nUP_BEHIND_BASE=4\nBASE_SHA=5555\nBASE_MB=3333\nBASE_BEHIND=6\nUSER_NAME=Matt Toohey\nUSER_EMAIL=matt@example.com\nLOG_OK=true\nLOG_START\n1111\x1f1111\x1fMatt\x1fmatt@example.com\x1f10\x1f9\x1fUSER_NAME=not a key\n\n0aaa\x1f0aaa\x1fMatt\x1fmatt@example.com\x1f8\x1f7\x1fSTATUS_END\n";

    #[test]
    fn parses_the_full_snapshot_output() {
        let out = parse_batch_git_state_output(FULL_SNAPSHOT_OUTPUT);
        assert_eq!(out.upstream_remote.as_deref(), Some("origin"));
        assert_eq!(out.upstream_merge.as_deref(), Some("refs/heads/feature"));
        assert_eq!(out.head_sha.as_deref(), Some("1111"));
        assert_eq!(out.branch.as_deref(), Some("feature"));
        assert_eq!(out.status_lines, " M file.txt");
        assert_eq!(out.up_sha.as_deref(), Some("2222"));
        assert_eq!((out.up_ahead, out.up_behind), (1, 2));
        assert_eq!(out.up_merge_base.as_deref(), Some("3333"));
        assert_eq!(out.up_behind_base, 4);
        assert_eq!(out.base_sha.as_deref(), Some("5555"));
        assert_eq!(out.base_behind, 6);
        assert_eq!(out.user_name.as_deref(), Some("Matt Toohey"));
        assert_eq!(out.user_email.as_deref(), Some("matt@example.com"));
        assert_eq!(out.log_ok, Some(true));
        // Two commits, blank line dropped, subjects that look like keys or
        // delimiters left alone.
        assert_eq!(out.log_lines.len(), 2);
        assert!(out.log_lines[0].ends_with("\x1fUSER_NAME=not a key"));
        assert!(out.log_lines[1].ends_with("\x1fSTATUS_END"));
    }

    #[test]
    fn parses_snapshot_output_without_an_upstream() {
        let raw = "UP_REMOTE=\nUP_MERGE=\nHEAD=1111\nBRANCH=feature\nSTATUS_START\nSTATUS_END\nUP_SHA=\nBASE_SHA=5555\nBASE_MB=1111\nBASE_BEHIND=0\nUSER_NAME=Matt\nUSER_EMAIL=\nLOG_OK=true\nLOG_START\n";
        let out = parse_batch_git_state_output(raw);
        assert!(out.upstream_remote.is_none());
        assert!(out.up_sha.is_none());
        assert_eq!((out.up_ahead, out.up_behind, out.up_behind_base), (0, 0, 0));
        assert!(out.up_merge_base.is_none());
        assert_eq!(out.base_sha.as_deref(), Some("5555"));
        assert_eq!(out.user_name.as_deref(), Some("Matt"));
        assert!(out.user_email.is_none());
        // Zero commits ahead: a successful, empty log.
        assert_eq!(out.log_ok, Some(true));
        assert!(out.log_lines.is_empty());
    }

    #[test]
    fn parses_a_failed_log_and_a_missing_log_section() {
        let failed = parse_batch_git_state_output(
            "HEAD=1111\nBRANCH=feature\nSTATUS_START\nSTATUS_END\nBASE_SHA=\nUSER_NAME=Matt\nUSER_EMAIL=m@example.com\nLOG_OK=\nLOG_START\n",
        );
        assert_eq!(failed.log_ok, Some(false));
        assert!(failed.log_lines.is_empty());
        assert_eq!(failed.user_email.as_deref(), Some("m@example.com"));

        // The state-only script: no identity, no log, nothing misread.
        let state_only = parse_batch_git_state_output(
            "HEAD=1111\nBRANCH=feature\nSTATUS_START\n?? new.txt\nSTATUS_END\nBASE_SHA=5555\nBASE_MB=1111\nBASE_BEHIND=2\n",
        );
        assert_eq!(state_only.log_ok, None);
        assert!(state_only.log_lines.is_empty());
        assert!(state_only.user_name.is_none());
        assert_eq!(state_only.status_lines, "?? new.txt");
        assert_eq!(state_only.base_behind, 2);
    }

    #[test]
    fn malformed_snapshot_output_is_tolerated() {
        let out = parse_batch_git_state_output(
            "garbage\nUP_COUNTS=x y\nUP_BEHIND_BASE=\nBASE_BEHIND=-3\nHEAD=\nLOG_OK=maybe\n=\n",
        );
        assert!(out.head_sha.is_none());
        assert_eq!((out.up_ahead, out.up_behind, out.up_behind_base), (0, 0, 0));
        assert_eq!(out.base_behind, 0);
        assert_eq!(out.log_ok, Some(false));
        assert!(out.log_lines.is_empty());
        assert!(parse_batch_git_state_output("").head_sha.is_none());
    }

    #[test]
    fn head_output_names_the_branch_or_nothing() {
        let on_branch = parse_head_output("1111\nrefs/heads/team/feature\n");
        assert_eq!(on_branch.sha.as_deref(), Some("1111"));
        assert_eq!(on_branch.branch.as_deref(), Some("team/feature"));

        let detached = parse_head_output("1111\nHEAD\n");
        assert_eq!(detached.sha.as_deref(), Some("1111"));
        assert!(detached.branch.is_none());

        let elsewhere = parse_head_output("1111\nrefs/remotes/origin/main\n");
        assert!(elsewhere.branch.is_none());

        let empty = parse_head_output("");
        assert!(empty.sha.is_none() && empty.branch.is_none());
    }

    #[test]
    fn for_each_ref_output_maps_exact_refnames() {
        let map = parse_for_each_ref_output(
            "refs/remotes/origin/main 1111\nrefs/remotes/origin/main/nested 2222\n\nno-sha\n 3333\nrefs/remotes/origin/feature 4444 extra\n",
        );
        assert_eq!(
            map.get("refs/remotes/origin/main").map(String::as_str),
            Some("1111")
        );
        assert_eq!(
            map.get("refs/remotes/origin/main/nested")
                .map(String::as_str),
            Some("2222")
        );
        assert_eq!(
            map.get("refs/remotes/origin/feature").map(String::as_str),
            Some("4444 extra")
        );
        assert_eq!(map.len(), 3);
        assert!(parse_for_each_ref_output("").is_empty());
    }

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

    #[test]
    fn repo_key_from_local_cache_key_normal_path() {
        assert_eq!(
            repo_key_from_local_cache_key("local:/Users/me/project:feature:main"),
            Some("local:/Users/me/project".to_string()),
        );
    }

    #[test]
    fn repo_key_from_local_cache_key_windows_path() {
        assert_eq!(
            repo_key_from_local_cache_key("local:C:\\Users\\me\\project:feature:main"),
            Some("local:C:\\Users\\me\\project".to_string()),
        );
    }

    #[test]
    fn repo_key_from_local_cache_key_branch_equals_base() {
        assert_eq!(
            repo_key_from_local_cache_key("local:/repo:main:main"),
            Some("local:/repo".to_string()),
        );
    }

    #[test]
    fn repo_key_from_local_cache_key_not_local() {
        assert_eq!(repo_key_from_local_cache_key("remote:foo:bar:baz"), None);
    }

    #[test]
    fn repo_key_from_local_cache_key_too_few_segments() {
        assert_eq!(repo_key_from_local_cache_key("local:only_one"), None);
    }
}
