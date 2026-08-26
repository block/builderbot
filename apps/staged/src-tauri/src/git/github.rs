//! GitHub integration for fetching pull requests.
//!
//! Uses the GitHub CLI (`gh`) for authentication and API access.
//! Includes caching to minimize API calls.

use super::cli::GitError;
use super::DiffSpec;
use super::GitRef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

// =============================================================================
// Types
// =============================================================================

/// GitHub authentication status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubAuthStatus {
    pub authenticated: bool,
    /// Help text if not authenticated (e.g., "run: gh auth login")
    pub setup_hint: Option<String>,
}

/// A pull request from GitHub (for display in picker)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub author: String,
    /// Target branch (e.g., "main")
    pub base_ref: String,
    /// Source branch (e.g., "feature-x") - not useful for forks
    pub head_ref: String,
    /// The repository the PR's head branch lives in (e.g., "fork-owner/repo" for fork PRs).
    /// None if the head repository has been deleted.
    pub head_repo: Option<String>,
    pub draft: bool,
    pub updated_at: String,
}

/// Status of a pull request, including CI checks and review state
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrStatus {
    /// Overall PR state (OPEN, CLOSED, MERGED)
    pub state: String,
    /// Whether the PR is a draft
    pub is_draft: bool,
    /// Whether the PR is mergeable
    pub mergeable: String,
    /// Review decision (APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED, or empty)
    pub review_decision: Option<String>,
    /// Summary of status checks
    pub checks_summary: ChecksSummary,
    /// The SHA of the PR's head commit on GitHub
    pub head_sha: Option<String>,
    /// Failed checks from the most recent GitHub status rollup.
    pub failed_checks: Vec<FailedCheck>,
}

/// Summary of CI/status checks for a PR
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecksSummary {
    /// Total number of checks
    pub total: u32,
    /// Number of passing checks
    pub passed: u32,
    /// Number of failed checks
    pub failed: u32,
    /// Number of pending checks
    pub pending: u32,
    /// Overall state (SUCCESS, FAILURE, PENDING, or EXPECTED if no checks)
    pub state: String,
}

/// A failed check from GitHub's PR status rollup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FailedCheck {
    pub name: String,
    pub state: String,
    pub details_url: Option<String>,
}

/// A GitHub issue (for display in picker)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub author: String,
    pub updated_at: String,
    pub labels: Vec<String>,
}

/// Owner login for deserializing `gh repo view --json=parent`.
#[derive(Deserialize)]
struct RepoOwner {
    login: String,
}

/// Parent repo info from `gh repo view --json=parent`.
#[derive(Deserialize)]
struct ParentRepoInfo {
    owner: RepoOwner,
    name: String,
}

/// Top-level response from `gh repo view --json=parent`.
#[derive(Deserialize)]
struct ParentRepoView {
    parent: Option<ParentRepoInfo>,
}

// =============================================================================
// Cache
// =============================================================================

/// How long to cache PR lists before they're considered stale.
const CACHE_TTL: Duration = Duration::from_secs(5 * 60); // 5 minutes

/// Cached PR list with expiration.
struct CachedPRList {
    prs: Vec<PullRequest>,
    fetched_at: Instant,
}

/// Global cache for PR lists, keyed by repo path.
static PR_CACHE: RwLock<Option<HashMap<String, CachedPRList>>> = RwLock::new(None);
static REPO_CLONE_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

fn get_cached_prs(repo: &Path) -> Option<Vec<PullRequest>> {
    let key = repo.to_string_lossy().to_string();
    let cache = PR_CACHE.read().ok()?;
    let cache = cache.as_ref()?;
    let entry = cache.get(&key)?;

    if entry.fetched_at.elapsed() < CACHE_TTL {
        Some(entry.prs.clone())
    } else {
        None
    }
}

fn set_cached_prs(repo: &Path, prs: Vec<PullRequest>) {
    let key = repo.to_string_lossy().to_string();
    let mut cache = match PR_CACHE.write() {
        Ok(c) => c,
        Err(_) => return,
    };

    let cache = cache.get_or_insert_with(HashMap::new);
    cache.insert(
        key,
        CachedPRList {
            prs,
            fetched_at: Instant::now(),
        },
    );
}

/// Clear the cache for a specific repo, forcing a fresh fetch.
pub fn invalidate_cache(repo: &Path) {
    let key = repo.to_string_lossy().to_string();
    if let Ok(mut cache) = PR_CACHE.write() {
        if let Some(ref mut map) = *cache {
            map.remove(&key);
        }
    }
}

// =============================================================================
// GitHub CLI Integration
// =============================================================================

/// Common paths where `gh` might be installed.
/// GUI apps on macOS don't inherit the shell's PATH, so we check these explicitly.
const GH_SEARCH_PATHS: &[&str] = &[
    "/opt/homebrew/bin",              // Homebrew on Apple Silicon
    "/usr/local/bin",                 // Homebrew on Intel Mac, common Linux location
    "/usr/bin",                       // System binaries
    "/home/linuxbrew/.linuxbrew/bin", // Linuxbrew
];

/// Find the `gh` CLI executable.
fn find_gh() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    // First, check if `gh` is directly available
    if let Ok(output) = Command::new("gh").arg("--version").output() {
        if output.status.success() {
            return Some(PathBuf::from("gh"));
        }
    }

    // Check common installation paths
    for dir in GH_SEARCH_PATHS {
        let path = PathBuf::from(dir).join("gh");
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// Spawn a gh command, drain its pipes concurrently, and enforce a 60s timeout.
fn spawn_gh_with_timeout(cmd: &mut Command) -> Result<std::process::Output, GitError> {
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| GitError::CommandFailed(format!("Failed to run gh: {e}")))?;

    // Drain stdout/stderr in background threads to avoid deadlock when the
    // child fills the OS pipe buffer before exiting.
    let stdout_thread = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut s, &mut buf).unwrap_or_default();
            buf
        })
    });
    let stderr_thread = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut s, &mut buf).unwrap_or_default();
            buf
        })
    });

    let timeout = Duration::from_secs(60);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Join drain threads so they don't leak — pipe close
                    // from kill makes them terminate quickly.
                    if let Some(t) = stdout_thread {
                        let _ = t.join();
                    }
                    if let Some(t) = stderr_thread {
                        let _ = t.join();
                    }
                    return Err(GitError::CommandFailed(
                        "gh command timed out after 60s".to_string(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(GitError::CommandFailed(format!(
                    "Failed to wait for gh: {e}"
                )));
            }
        }
    };

    let stdout = stdout_thread.map_or_else(Vec::new, |t| t.join().unwrap_or_default());
    let stderr = stderr_thread.map_or_else(Vec::new, |t| t.join().unwrap_or_default());

    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// Check the output of a gh command and return stdout as a string.
fn check_gh_output(output: std::process::Output) -> Result<String, GitError> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not logged in") || stderr.contains("no oauth token") {
            return Err(GitError::CommandFailed(
                "Not authenticated with GitHub CLI. Run: gh auth login".to_string(),
            ));
        }
        return Err(GitError::CommandFailed(stderr.into_owned()));
    }

    String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8)
}

/// Run a gh command in the context of a local repo directory
fn run_gh(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    let gh_path = find_gh().ok_or_else(|| {
        GitError::CommandFailed("GitHub CLI not found. Install with: brew install gh".to_string())
    })?;

    let output = spawn_gh_with_timeout(Command::new(&gh_path).current_dir(repo).args(args))?;
    check_gh_output(output)
}

/// Run a gh command without needing a local directory (uses `-R owner/repo` or global commands).
fn run_gh_global(args: &[&str]) -> Result<String, GitError> {
    let gh_path = find_gh().ok_or_else(|| {
        GitError::CommandFailed("GitHub CLI not found. Install with: brew install gh".to_string())
    })?;

    let output = spawn_gh_with_timeout(Command::new(&gh_path).args(args))?;
    check_gh_output(output)
}

// =============================================================================
// Public API
// =============================================================================

/// Check if GitHub CLI is installed and authenticated
pub fn check_github_auth() -> GitHubAuthStatus {
    let gh_path = match find_gh() {
        Some(p) => p,
        None => {
            return GitHubAuthStatus {
                authenticated: false,
                setup_hint: Some("GitHub CLI not found. Install with: brew install gh".to_string()),
            }
        }
    };

    let output = match Command::new(&gh_path).args(["auth", "status"]).output() {
        Ok(o) => o,
        Err(e) => {
            return GitHubAuthStatus {
                authenticated: false,
                setup_hint: Some(format!("Failed to run gh: {e}")),
            }
        }
    };

    if output.status.success() {
        GitHubAuthStatus {
            authenticated: true,
            setup_hint: None,
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        GitHubAuthStatus {
            authenticated: false,
            setup_hint: Some(if stderr.contains("not logged in") {
                "Run: gh auth login".to_string()
            } else {
                stderr.trim().to_string()
            }),
        }
    }
}

// =============================================================================
// GitHub repo-based operations (no local clone needed)
// =============================================================================

/// A GitHub repository from `gh repo list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubRepo {
    pub name: String,
    pub name_with_owner: String,
    pub description: Option<String>,
    pub is_private: bool,
    pub updated_at: String,
}

/// Response from `gh repo list --json`
#[derive(Debug, Deserialize)]
struct GhRepoListItem {
    name: String,
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
    description: Option<String>,
    #[serde(rename = "isPrivate")]
    is_private: bool,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

impl From<GhRepoListItem> for GitHubRepo {
    fn from(item: GhRepoListItem) -> Self {
        GitHubRepo {
            name: item.name,
            name_with_owner: item.name_with_owner,
            description: item.description,
            is_private: item.is_private,
            updated_at: item.updated_at,
        }
    }
}

/// List the authenticated user's GitHub organization memberships.
pub fn list_github_orgs() -> Result<Vec<String>, GitError> {
    let output = run_gh_global(&["api", "/user/orgs", "--jq", ".[].login"])?;
    Ok(output
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

/// List GitHub repositories for the authenticated user or a specific owner.
pub fn list_github_repos(owner: Option<&str>) -> Result<Vec<GitHubRepo>, GitError> {
    let mut args = vec!["repo", "list"];
    // When an owner is provided, pass it as a positional arg to scope the listing
    let owner_string;
    if let Some(o) = owner {
        owner_string = o.to_string();
        args.push(&owner_string);
    }
    args.extend_from_slice(&[
        "--json=name,nameWithOwner,description,isPrivate,updatedAt",
        "--limit=100",
        "--no-archived",
    ]);

    let output = run_gh_global(&args)?;

    let items: Vec<GhRepoListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(items.into_iter().map(Into::into).collect())
}

/// List repositories the authenticated user has recently interacted with.
/// Uses the Events API to find repos from actual user activity (pushes, PRs, issues, etc).
/// This is more accurate than /user/repos which includes all org repos.
pub fn list_user_repos(limit: u32) -> Result<Vec<GitHubRepo>, GitError> {
    // Resolve the authenticated user's login (needed for user events endpoint)
    let login = run_gh_global(&["api", "/user", "--jq", ".login"])?
        .trim()
        .to_string();

    // Fetch user events - this shows actual activity, not just membership
    // Events API returns up to 300 events (10 pages of 30) going back ~90 days
    let events_limit = 100.min(limit * 3); // Fetch more events since we'll dedupe
    let endpoint = format!("/users/{login}/events?per_page={events_limit}");
    let output = run_gh_global(&["api", &endpoint])?;

    #[derive(Debug, Deserialize)]
    struct Event {
        repo: EventRepo,
    }

    #[derive(Debug, Deserialize)]
    struct EventRepo {
        name: String, // "owner/repo" format
    }

    let events: Vec<Event> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    // Extract unique repo names in order of recency (events are already sorted)
    let mut seen = std::collections::HashSet::new();
    let mut repo_names: Vec<String> = Vec::new();

    for event in events {
        if seen.insert(event.repo.name.clone()) {
            repo_names.push(event.repo.name);
            if repo_names.len() >= limit as usize {
                break;
            }
        }
    }

    // Fetch full repo details for each unique repo
    // We do this in parallel-ish by collecting results
    let mut repos = Vec::new();
    for name in repo_names {
        let parts: Vec<&str> = name.split('/').collect();
        if parts.len() == 2 {
            match fetch_github_repo(parts[0], parts[1]) {
                Ok(Some(repo)) => repos.push(repo),
                Ok(None) => {} // Repo no longer exists or no access
                Err(_) => {}   // Skip on error
            }
        }
    }

    Ok(repos)
}

/// Fetch a single GitHub repository by owner/repo.
/// Returns None if the repo doesn't exist or user lacks access.
pub fn fetch_github_repo(owner: &str, repo: &str) -> Result<Option<GitHubRepo>, GitError> {
    let endpoint = format!("/repos/{owner}/{repo}");
    let output = run_gh_global(&["api", &endpoint]);

    match output {
        Ok(json) => {
            #[derive(Debug, Deserialize)]
            struct ApiRepo {
                name: String,
                full_name: String,
                description: Option<String>,
                private: bool,
                pushed_at: Option<String>,
            }

            let item: ApiRepo =
                serde_json::from_str(&json).map_err(|e| GitError::CommandFailed(e.to_string()))?;

            Ok(Some(GitHubRepo {
                name: item.name,
                name_with_owner: item.full_name,
                description: item.description,
                is_private: item.private,
                updated_at: item.pushed_at.unwrap_or_default(),
            }))
        }
        Err(e) => {
            let msg = e.to_string();
            // 404 means repo doesn't exist or no access - not an error
            if msg.contains("404") || msg.contains("Not Found") {
                Ok(None)
            } else {
                Err(e)
            }
        }
    }
}

/// Search GitHub repositories.
/// - If owner contains "/", treat it as "owner/partial-repo" and search within that org
/// - If owner is provided without "/", search within that org
/// - If owner is None, search all of GitHub
pub fn search_github_repos(query: &str, owner: Option<&str>) -> Result<Vec<GitHubRepo>, GitError> {
    let mut args = vec!["search", "repos", query];
    let owner_flag;

    if let Some(o) = owner {
        owner_flag = format!("--owner={o}");
        args.push(&owner_flag);
    }
    // No owner = search all of GitHub (don't add --owner flag)

    args.extend_from_slice(&[
        "--json=name,fullName,description,isPrivate,updatedAt",
        "--limit=30",
    ]);

    let output = run_gh_global(&args)?;

    // gh search repos uses `fullName` instead of `nameWithOwner`
    #[derive(Debug, Deserialize)]
    struct GhSearchRepoItem {
        name: String,
        #[serde(rename = "fullName")]
        full_name: String,
        description: Option<String>,
        #[serde(rename = "isPrivate")]
        is_private: bool,
        #[serde(rename = "updatedAt")]
        updated_at: String,
    }

    let items: Vec<GhSearchRepoItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(items
        .into_iter()
        .map(|item| GitHubRepo {
            name: item.name,
            name_with_owner: item.full_name,
            description: item.description,
            is_private: item.is_private,
            updated_at: item.updated_at,
        })
        .collect())
}

/// Fetch a single pull request by number using `-R owner/repo` (no local dir needed).
pub fn get_pr_for_repo(github_repo: &str, pr_number: u64) -> Result<PullRequest, GitError> {
    let output = run_gh_global(&[
        "pr",
        "view",
        &pr_number.to_string(),
        "-R",
        github_repo,
        "--json=number,title,body,author,baseRefName,headRefName,headRepository,isDraft,updatedAt",
    ])?;

    let item: GhPrListItem =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(item.into())
}

/// Find the open PR (if any) whose head branch matches `branch_name`.
pub fn get_pr_for_branch_for_repo(
    github_repo: &str,
    branch_name: &str,
) -> Result<Option<PullRequest>, GitError> {
    let output = run_gh_global(&[
        "pr",
        "list",
        "-R",
        github_repo,
        "--head",
        branch_name,
        "--state=open",
        "--limit=1",
        "--json=number,title,body,author,baseRefName,headRefName,headRepository,isDraft,updatedAt",
    ])?;

    let items: Vec<GhPrListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(items.into_iter().next().map(Into::into))
}

/// List open pull requests for a repo using `-R owner/repo` (no local dir needed).
pub fn list_pull_requests_for_repo(github_repo: &str) -> Result<Vec<PullRequest>, GitError> {
    let output = run_gh_global(&[
        "pr",
        "list",
        "-R",
        github_repo,
        "--state=open",
        "--limit=50",
        "--json=number,title,body,author,baseRefName,headRefName,headRepository,isDraft,updatedAt",
    ])?;

    let items: Vec<GhPrListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(items.into_iter().map(Into::into).collect())
}

/// List open issues for a repo using `-R owner/repo` (no local dir needed).
pub fn list_issues_for_repo(github_repo: &str) -> Result<Vec<Issue>, GitError> {
    let output = run_gh_global(&[
        "issue",
        "list",
        "-R",
        github_repo,
        "--state=open",
        "--limit=50",
        "--json=number,title,body,author,updatedAt,labels",
    ])?;

    let items: Vec<GhIssueListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(items.into_iter().map(Into::into).collect())
}

/// Detect the default branch for a repo via GitHub API (no local clone needed).
pub fn detect_default_branch_for_repo(github_repo: &str) -> Result<String, GitError> {
    let output = run_gh_global(&["repo", "view", github_repo, "--json=defaultBranchRef"])?;

    #[derive(Debug, Deserialize)]
    struct RepoView {
        #[serde(rename = "defaultBranchRef")]
        default_branch_ref: DefaultBranchRef,
    }
    #[derive(Debug, Deserialize)]
    struct DefaultBranchRef {
        name: String,
    }

    let view: RepoView =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(view.default_branch_ref.name)
}

/// Resolve the default branch for a repo, using a prefetched value when available
/// and falling back to the GitHub API. Returns an `origin/`-prefixed ref.
pub fn resolve_default_branch(prefetched: Option<String>, github_repo: &str) -> String {
    let detected = prefetched
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            detect_default_branch_for_repo(github_repo).unwrap_or_else(|_| "main".to_string())
        });
    if detected.starts_with("origin/") {
        detected
    } else {
        format!("origin/{detected}")
    }
}

/// A git branch reference from the GitHub API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubBranchRef {
    pub name: String,
}

/// List branches for a repo via GitHub API (no local clone needed).
pub fn list_branches_for_repo(github_repo: &str) -> Result<Vec<super::BranchRef>, GitError> {
    let endpoint = format!("repos/{github_repo}/branches?per_page=100");
    let output = run_gh_global(&["api", &endpoint])?;

    #[derive(Debug, Deserialize)]
    struct ApiBranch {
        name: String,
    }

    let branches: Vec<ApiBranch> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(branches
        .into_iter()
        .map(|b| super::BranchRef {
            name: b.name,
            is_remote: true,
            remote: Some("origin".to_string()),
        })
        .collect())
}

/// Prune remote refs for a repo. With the GitHub-repo-based model, this is a no-op
/// since we use the GitHub API for branch listing. Kept for API compatibility.
pub fn prune_remote_for_repo(_github_repo: &str) -> Result<(), GitError> {
    Ok(())
}

fn remove_stale_clone_dir(clone_path: &Path) -> Result<(), GitError> {
    if !clone_path.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(clone_path).map_err(|e| {
        GitError::CommandFailed(format!(
            "Failed to clear stale clone directory '{}': {e}",
            clone_path.display()
        ))
    })
}

fn clone_lock_key(github_repo: &str) -> String {
    // GitHub repo slugs are case-insensitive. Normalizing avoids duplicate
    // lock entries for case variants like Owner/Repo vs owner/repo.
    github_repo.trim().to_ascii_lowercase()
}

fn clone_lock_for_repo(github_repo: &str) -> Arc<Mutex<()>> {
    let locks = REPO_CLONE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut lock_map = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Opportunistically drop lock entries that are no longer referenced by any
    // active clone operation so the map does not grow without bound.
    lock_map.retain(|_, lock| Arc::strong_count(lock) > 1);
    let key = clone_lock_key(github_repo);
    lock_map
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Fetch the latest from origin and reset the main checkout's working tree to
/// the remote default branch tip.
///
/// This ensures the files on disk in the main clone reflect the latest
/// upstream state — critical for action detection which reads files from the
/// working tree. Worktrees are separate directories and are **not** affected
/// by this reset.
///
/// Errors are logged but not propagated; a stale working tree is better than
/// failing action detection entirely.
pub fn update_clone_to_remote_head(repo_path: &std::path::Path, github_repo: &str) {
    let https_url = format!("https://github.com/{github_repo}.git");

    // Fetch all remote refs so we have the latest commits.
    if let Err(e) = super::cli::run(repo_path, &["fetch", "origin"]) {
        log::warn!(
            "fetch origin failed for '{}': {}. Retrying with HTTPS origin.",
            github_repo,
            e
        );
        if super::cli::run(repo_path, &["remote", "set-url", "origin", &https_url]).is_err() {
            log::warn!("failed to set HTTPS origin for '{}'", github_repo);
            return;
        }
        if let Err(e) = super::cli::run(repo_path, &["fetch", "origin"]) {
            log::warn!(
                "fetch origin (HTTPS retry) failed for '{}': {}",
                github_repo,
                e
            );
            return;
        }
    }

    // Detect the default branch from remote refs (e.g. origin/main).
    let default_branch = match super::refs::detect_default_branch(repo_path) {
        Ok(branch) => branch,
        Err(e) => {
            log::warn!(
                "could not detect default branch for '{}': {}",
                github_repo,
                e
            );
            return;
        }
    };

    // Reset the working tree + index to the remote default branch tip.
    // This is equivalent to `git reset --hard origin/main` and only touches
    // the main checkout — worktrees have their own HEAD and working tree.
    if let Err(e) = super::cli::run(repo_path, &["reset", "--hard", &default_branch]) {
        log::warn!(
            "reset --hard {} failed for '{}': {}",
            default_branch,
            github_repo,
            e
        );
    }
}

/// Ensure a local clone exists at `<repos_dir>/<owner>/<repo>/`.
///
/// If the directory already exists, returns the path immediately without
/// fetching. Callers that need fresh refs should call [`fetch_for_worktree`]
/// afterwards. If not cloned yet, clones the repo. Returns the path to the
/// local clone.
pub fn ensure_local_clone(github_repo: &str) -> Result<std::path::PathBuf, GitError> {
    // Multiple setup flows (frontend + backend) can request the same clone at once.
    // Serialize clone creation/deletion per repo to avoid races on first run.
    let repo_lock = clone_lock_for_repo(github_repo);
    let _clone_guard = repo_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let repos = crate::paths::repos_dir()
        .ok_or_else(|| GitError::CommandFailed("Cannot determine data directory".to_string()))?;

    let clone_path = repos.join(github_repo);
    let https_url = format!("https://github.com/{github_repo}.git");

    if clone_path.join(".git").exists() {
        super::config_apply::apply_to_clone(&clone_path);
        return Ok(clone_path);
    }

    // If a previous clone attempt failed, clear the stale directory so clone can retry.
    remove_stale_clone_dir(&clone_path)?;

    // Create parent directory
    if let Some(parent) = clone_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            GitError::CommandFailed(format!("Failed to create clone directory: {e}"))
        })?;
    }

    // Prefer `gh repo clone` first (works well when gh auth is configured).
    // If this fails (e.g. gh is set to SSH protocol and org requires SSH certs),
    // fall back to plain HTTPS git clone.
    let clone_str = clone_path.to_string_lossy().to_string();
    let gh_clone_result = (|| -> Result<(), GitError> {
        let gh_path = find_gh().ok_or_else(|| {
            GitError::CommandFailed(
                "GitHub CLI not found. Install with: brew install gh".to_string(),
            )
        })?;

        let output = Command::new(&gh_path)
            .env("GH_GIT_PROTOCOL", "https")
            .args(["repo", "clone", github_repo, &clone_str])
            .output()
            .map_err(|e| GitError::CommandFailed(format!("Failed to run gh: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not logged in") || stderr.contains("no oauth token") {
                return Err(GitError::CommandFailed(
                    "Not authenticated with GitHub CLI. Run: gh auth login".to_string(),
                ));
            }
            return Err(GitError::CommandFailed(stderr.into_owned()));
        }
        Ok(())
    })();

    if gh_clone_result.is_err() || !clone_path.join(".git").exists() {
        log::warn!(
            "gh repo clone failed for '{}', retrying with direct HTTPS git clone",
            github_repo
        );
        // `gh repo clone` can leave a non-empty directory behind even on
        // failure. Clear it before falling back to `git clone` so we don't
        // fail with "destination path ... already exists".
        remove_stale_clone_dir(&clone_path)?;
        if let Some(parent) = clone_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                GitError::CommandFailed(format!("Failed to create clone directory: {e}"))
            })?;
        }
        super::cli::run(
            clone_path.parent().unwrap_or(Path::new("/")),
            &["clone", &https_url, &clone_str],
        )?;
    }

    super::config_apply::apply_to_clone(&clone_path);

    Ok(clone_path)
}

/// Spawn a command and stream its stderr line-by-line to a callback.
///
/// Git (and gh) write progress output to stderr using `\r` for in-place
/// updates, so we split on both `\r` and `\n`.
///
/// Stderr is also accumulated so that callers can inspect error output on
/// failure (e.g. for auth-specific error detection).
fn spawn_streaming<F>(
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    on_line: &mut F,
) -> Result<(), GitError>
where
    F: FnMut(&str),
{
    use std::io::{BufReader, Read};
    use std::process::Stdio;

    let mut cmd = std::process::Command::new(program);
    cmd.args(args).stdout(Stdio::null()).stderr(Stdio::piped());
    for &(key, val) in envs {
        cmd.env(key, val);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| GitError::CommandFailed(format!("Failed to run {program}: {e}")))?;

    let mut all_stderr = String::new();

    if let Some(stderr) = child.stderr.take() {
        let mut buf = Vec::new();
        for byte in BufReader::new(stderr).bytes() {
            let byte = byte.map_err(|e| GitError::CommandFailed(e.to_string()))?;
            if byte == b'\r' || byte == b'\n' {
                if !buf.is_empty() {
                    if let Ok(line) = std::str::from_utf8(&buf) {
                        all_stderr.push_str(line);
                        all_stderr.push('\n');
                        on_line(line);
                    }
                    buf.clear();
                }
            } else {
                buf.push(byte);
            }
        }
        if !buf.is_empty() {
            if let Ok(line) = std::str::from_utf8(&buf) {
                all_stderr.push_str(line);
                all_stderr.push('\n');
                on_line(line);
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;
    if !status.success() {
        return Err(GitError::CommandFailed(all_stderr));
    }
    Ok(())
}

/// Like [`ensure_local_clone`] but streams stderr progress lines to a callback.
///
/// This allows callers to report detailed clone progress (e.g.
/// "Receiving objects — 27%") to the UI.
pub fn ensure_local_clone_with_progress<F>(
    github_repo: &str,
    mut on_stderr_line: F,
) -> Result<std::path::PathBuf, GitError>
where
    F: FnMut(&str),
{
    let repo_lock = clone_lock_for_repo(github_repo);
    let _clone_guard = repo_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let repos = crate::paths::repos_dir()
        .ok_or_else(|| GitError::CommandFailed("Cannot determine data directory".to_string()))?;

    let clone_path = repos.join(github_repo);
    let https_url = format!("https://github.com/{github_repo}.git");

    if clone_path.join(".git").exists() {
        super::config_apply::apply_to_clone(&clone_path);
        return Ok(clone_path);
    }

    // If a previous clone attempt failed, clear the stale directory so clone can retry.
    remove_stale_clone_dir(&clone_path)?;

    // Create parent directory
    if let Some(parent) = clone_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            GitError::CommandFailed(format!("Failed to create clone directory: {e}"))
        })?;
    }

    let clone_str = clone_path.to_string_lossy().to_string();

    // Prefer `gh repo clone` first (works well when gh auth is configured).
    // If this fails (e.g. gh is set to SSH protocol and org requires SSH certs),
    // fall back to plain HTTPS git clone.
    let gh_clone_result = (|| -> Result<(), GitError> {
        let gh_path = find_gh().ok_or_else(|| {
            GitError::CommandFailed(
                "GitHub CLI not found. Install with: brew install gh".to_string(),
            )
        })?;

        let gh_str = gh_path.to_string_lossy().to_string();
        let result = spawn_streaming(
            &gh_str,
            &["repo", "clone", github_repo, &clone_str, "--", "--progress"],
            &[("GH_GIT_PROTOCOL", "https")],
            &mut on_stderr_line,
        );

        // Surface auth-specific errors with a helpful message instead of
        // falling through to the generic HTTPS fallback.
        if let Err(GitError::CommandFailed(ref stderr)) = result {
            if stderr.contains("not logged in") || stderr.contains("no oauth token") {
                return Err(GitError::CommandFailed(
                    "Not authenticated with GitHub CLI. Run: gh auth login".to_string(),
                ));
            }
        }

        result
    })();

    if gh_clone_result.is_err() || !clone_path.join(".git").exists() {
        log::warn!(
            "gh repo clone failed for '{}', retrying with direct HTTPS git clone",
            github_repo
        );
        remove_stale_clone_dir(&clone_path)?;
        if let Some(parent) = clone_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                GitError::CommandFailed(format!("Failed to create clone directory: {e}"))
            })?;
        }
        spawn_streaming(
            "git",
            &["clone", "--progress", &https_url, &clone_str],
            &[],
            &mut on_stderr_line,
        )?;
    }

    super::config_apply::apply_to_clone(&clone_path);

    Ok(clone_path)
}

/// Fetch only the refs needed for worktree creation.
///
/// Fetches `base_branch` (always required, always present on the remote) and
/// does a best-effort fetch of `branch_name` (which may not exist on the
/// remote yet for newly-created branches). This is far cheaper than
/// `git fetch origin` and avoids the ref-lock races that occur when hundreds
/// of remote-tracking refs are updated simultaneously in a shared clone.
///
/// If `base_branch` cannot be fetched, the origin URL is switched to HTTPS
/// and the fetch is retried once (same behaviour as the original fetch in
/// `ensure_local_clone`).
pub fn fetch_for_worktree(
    repo_path: &std::path::Path,
    github_repo: &str,
    branch_name: &str,
    base_branch: &str,
) -> Result<(), GitError> {
    let https_url = format!("https://github.com/{github_repo}.git");

    // Strip any "origin/" prefix — base_branch is stored in the DB with this
    // prefix (normalised at creation time) but the remote tracks the bare ref.
    let base_ref = base_branch.strip_prefix("origin/").unwrap_or(base_branch);
    let branch_ref = branch_name.strip_prefix("origin/").unwrap_or(branch_name);

    // Fetch the base branch — always needed, always exists on the remote.
    if let Err(e) = super::cli::run(repo_path, &["fetch", "origin", base_ref]) {
        let err_str = e.to_string();
        if err_str.contains("incorrect old value provided") {
            // Ref-update CAS race: a concurrent fetch already updated
            // refs/remotes/origin/<base_ref> between when git read the old
            // value and when it tried to write the new one.  The downloaded
            // data and FETCH_HEAD are valid; the tracking ref is at least as
            // up-to-date as we need.
            log::warn!(
                "fetch origin {} for '{}' hit a ref-update race (non-fatal): {}",
                base_ref,
                github_repo,
                e
            );
        } else {
            log::warn!(
                "fetch origin {} failed for '{}': {}. Retrying with HTTPS origin.",
                base_ref,
                github_repo,
                e
            );
            super::cli::run(repo_path, &["remote", "set-url", "origin", &https_url])?;
            super::cli::run(repo_path, &["fetch", "origin", base_ref])?;
        }
    }

    // Best-effort fetch of the branch itself — it may not exist on the remote
    // yet for new local branches.
    if branch_ref != base_ref {
        if let Err(e) = super::cli::run(repo_path, &["fetch", "origin", branch_ref]) {
            let err_str = e.to_string();
            if !err_str.contains("couldn't find remote ref") {
                log::warn!(
                    "fetch origin {} for '{}' failed (non-fatal): {}",
                    branch_ref,
                    github_repo,
                    e
                );
            }
        }
    }

    Ok(())
}

// =============================================================================
// PR and Issue listing (local-dir-based, legacy)
// =============================================================================

/// Response from `gh pr list --json`
#[derive(Debug, Deserialize)]
struct GhPrListItem {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    author: GhAuthor,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "headRepository")]
    head_repository: Option<GhRepository>,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct GhAuthor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhRepository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

impl From<GhPrListItem> for PullRequest {
    fn from(item: GhPrListItem) -> Self {
        PullRequest {
            number: item.number,
            title: item.title,
            body: item.body,
            author: item.author.login,
            base_ref: item.base_ref_name,
            head_ref: item.head_ref_name,
            head_repo: item.head_repository.map(|r| r.name_with_owner),
            draft: item.is_draft,
            updated_at: item.updated_at,
        }
    }
}

/// List open pull requests for the repo
pub fn list_pull_requests(repo: &Path) -> Result<Vec<PullRequest>, GitError> {
    // Check cache first
    if let Some(cached) = get_cached_prs(repo) {
        return Ok(cached);
    }

    let output = run_gh(
        repo,
        &[
            "pr",
            "list",
            "--state=open",
            "--limit=50",
            "--json=number,title,body,author,baseRefName,headRefName,headRepository,isDraft,updatedAt",
        ],
    )?;

    let items: Vec<GhPrListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    let prs: Vec<PullRequest> = items.into_iter().map(Into::into).collect();

    // Cache the result
    set_cached_prs(repo, prs.clone());

    Ok(prs)
}

/// Search for pull requests on GitHub using a query string.
/// Uses GitHub's search syntax via `gh pr list --search`.
/// Does not use caching since search queries vary.
pub fn search_pull_requests(repo: &Path, query: &str) -> Result<Vec<PullRequest>, GitError> {
    let output = run_gh(
        repo,
        &[
            "pr",
            "list",
            "--state=open",
            "--limit=50",
            &format!("--search={query}"),
            "--json=number,title,body,author,baseRefName,headRefName,headRepository,isDraft,updatedAt",
        ],
    )?;

    let items: Vec<GhPrListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(items.into_iter().map(Into::into).collect())
}

// =============================================================================
// Issues
// =============================================================================

/// Response from `gh issue list --json`
#[derive(Debug, Deserialize)]
struct GhIssueListItem {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    author: GhAuthor,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    labels: Vec<GhLabel>,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

impl From<GhIssueListItem> for Issue {
    fn from(item: GhIssueListItem) -> Self {
        Issue {
            number: item.number,
            title: item.title,
            body: item.body,
            author: item.author.login,
            updated_at: item.updated_at,
            labels: item.labels.into_iter().map(|l| l.name).collect(),
        }
    }
}

/// List open issues for the repo
pub fn list_issues(repo: &Path) -> Result<Vec<Issue>, GitError> {
    let output = run_gh(
        repo,
        &[
            "issue",
            "list",
            "--state=open",
            "--limit=50",
            "--json=number,title,body,author,updatedAt,labels",
        ],
    )?;

    let items: Vec<GhIssueListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(items.into_iter().map(Into::into).collect())
}

/// Search for issues on GitHub using a query string.
/// Uses GitHub's search syntax via `gh issue list --search`.
pub fn search_issues(repo: &Path, query: &str) -> Result<Vec<Issue>, GitError> {
    let output = run_gh(
        repo,
        &[
            "issue",
            "list",
            "--state=open",
            "--limit=50",
            &format!("--search={query}"),
            "--json=number,title,body,author,updatedAt,labels",
        ],
    )?;

    let items: Vec<GhIssueListItem> =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(items.into_iter().map(Into::into).collect())
}

/// Fetch PR refs and compute merge-base
///
/// - Fetches refs/pull/{number}/head
/// - Fetches origin/{base_ref}
/// - Computes merge-base
///
/// Returns DiffSpec with two concrete SHAs: Rev(merge_base)..Rev(head_sha)
pub fn fetch_pr(repo: &Path, base_ref: &str, pr_number: u64) -> Result<DiffSpec, GitError> {
    use super::cli;
    use super::worktree::fetch_pr_head_sha;

    // Fetch the PR head ref using a named local ref (safe for concurrent use)
    let head_sha = fetch_pr_head_sha(repo, pr_number)?;

    // Fetch the base branch
    let base_remote_ref = format!("origin/{base_ref}");
    cli::run(repo, &["fetch", "origin", base_ref])?;

    // Compute merge-base between base and PR head
    let merge_base_sha = cli::run(repo, &["merge-base", &base_remote_ref, &head_sha])?
        .trim()
        .to_string();

    Ok(DiffSpec {
        base: GitRef::Rev(merge_base_sha),
        head: GitRef::Rev(head_sha),
    })
}

// =============================================================================
// Review Sync
// =============================================================================

use crate::store::Comment;

/// Get the GitHub token from `gh auth token`.
fn get_github_token() -> Result<String, GitError> {
    let gh_path = find_gh().ok_or_else(|| {
        GitError::CommandFailed("GitHub CLI not found. Install with: brew install gh".to_string())
    })?;

    let output = Command::new(&gh_path)
        .args(["auth", "token"])
        .output()
        .map_err(|e| GitError::CommandFailed(format!("Failed to run gh: {e}")))?;

    if output.status.success() {
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            Err(GitError::CommandFailed(
                "GitHub CLI returned empty token. Run: gh auth login".to_string(),
            ))
        } else {
            Ok(token)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not logged in") || stderr.contains("no oauth token") {
            Err(GitError::CommandFailed(
                "Not authenticated with GitHub CLI. Run: gh auth login".to_string(),
            ))
        } else {
            Err(GitError::CommandFailed(format!(
                "GitHub CLI error: {}",
                stderr.trim()
            )))
        }
    }
}

/// Get the GitHub owner/repo from the repo's origin remote.
pub fn get_github_repo(repo: &Path) -> Result<(String, String), GitError> {
    use super::cli;

    let url = cli::run(repo, &["remote", "get-url", "origin"])?;
    let url = url.trim();

    // Parse SSH format: git@github.com:owner/repo.git
    // Also handles org-*@github.com:owner/repo.git (GitHub App installs)
    if url.contains("github.com:") {
        if let Some(idx) = url.find("github.com:") {
            let after = &url[idx + "github.com:".len()..];
            let path = after.strip_suffix(".git").unwrap_or(after);
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() == 2 {
                return Ok((parts[0].to_string(), parts[1].to_string()));
            }
        }
    }

    // Parse HTTPS format: https://github.com/owner/repo.git
    if url.contains("github.com/") {
        if let Some(idx) = url.find("github.com/") {
            let after = &url[idx + "github.com/".len()..];
            let path = after.strip_suffix(".git").unwrap_or(after);
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() >= 2 {
                return Ok((parts[0].to_string(), parts[1].to_string()));
            }
        }
    }

    Err(GitError::CommandFailed(format!(
        "Could not parse GitHub repo from origin URL: {url}"
    )))
}

/// Comment for creating a review (request body format).
#[derive(Debug, Serialize)]
struct GitHubReviewComment {
    path: String,
    body: String,
    line: u32,
    side: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_side: Option<&'static str>,
}

/// Convert a local Comment to a GitHub review comment.
///
/// If `valid_lines` is provided, checks if the comment's lines are within the diff.
/// Returns `None` for comments outside the diff (they fall back to a PR issue
/// comment, which needs no line placement).
fn convert_comment(
    comment: &Comment,
    valid_lines: Option<&std::collections::HashSet<u32>>,
) -> Option<GitHubReviewComment> {
    // Convert 0-indexed span to 1-indexed line numbers
    let line = comment.span.end; // end line (1-indexed, since end is exclusive)
    let start_line = comment.span.start + 1; // start line (1-indexed)

    // Check if this line is within the diff
    let line_in_diff = valid_lines
        .map(|lines| lines.contains(&line))
        .unwrap_or(true);

    if !line_in_diff {
        return None;
    }

    // For single-line comments, don't use start_line
    let is_multiline = comment.span.end > comment.span.start + 1;

    Some(GitHubReviewComment {
        path: comment.path.clone(),
        body: comment.content.clone(),
        line,
        side: "RIGHT", // Always RIGHT since we only support comments on new code
        start_line: if is_multiline { Some(start_line) } else { None },
        start_side: if is_multiline { Some("RIGHT") } else { None },
    })
}

/// Agent-authored comments carry a robot-emoji suffix so readers can tell them
/// apart on GitHub. Human-authored comments are posted verbatim.
pub fn github_single_comment_body(comment: &Comment) -> String {
    if comment.author == crate::store::CommentAuthor::Agent {
        format!("{}\n\n🤖", comment.content)
    } else {
        comment.content.clone()
    }
}

pub fn github_issue_comment_body(comment: &Comment, body: &str) -> String {
    let line = comment.span.end;
    let start_line = comment.span.start + 1;
    let line_info = if comment.span.end > comment.span.start + 1 {
        format!("Lines {start_line}-{line}")
    } else {
        format!("Line {line}")
    };

    format!("**{}** ({})\n\n{}", comment.path, line_info, body)
}

/// Parse a unified diff patch into the set of line numbers that can carry a
/// RIGHT-side comment (added and context lines, 1-indexed in the new file).
fn valid_lines_from_patch(patch: Option<&str>) -> std::collections::HashSet<u32> {
    let mut valid_lines = std::collections::HashSet::new();
    let Some(patch) = patch else {
        return valid_lines;
    };

    let mut current_line: u32 = 0;

    for line in patch.lines() {
        if line.starts_with("@@") {
            // Parse hunk header: @@ -X,Y +Z,W @@
            if let Some(plus_pos) = line.find('+') {
                let after_plus = &line[plus_pos + 1..];
                if let Some(comma_or_space) = after_plus.find([',', ' ']) {
                    if let Ok(start) = after_plus[..comma_or_space].parse::<u32>() {
                        current_line = start;
                    }
                }
            }
        } else if line.starts_with('-') {
            // Deleted line - doesn't increment new file line number
        } else if line.starts_with('+') || !line.starts_with('\\') {
            // Added line or context line - valid for RIGHT side comments
            valid_lines.insert(current_line);
            current_line += 1;
        }
    }

    valid_lines
}

/// Extract the `rel="next"` URL from a GitHub `Link` header, if present.
fn parse_link_next(link_header: &str) -> Option<&str> {
    link_header.split(',').find_map(|part| {
        let mut segments = part.split(';');
        let url = segments.next()?.trim();
        if !segments.any(|s| s.trim() == "rel=\"next\"") {
            return None;
        }
        url.strip_prefix('<')?.strip_suffix('>')
    })
}

/// Fetch the valid line numbers for each file in a PR diff.
/// Returns a map of file path -> set of valid line numbers (1-indexed, RIGHT side).
///
/// Follows `Link` pagination: a file missing from the map is treated as entirely
/// outside the diff, so stopping at the first page would turn in-diff comments
/// on large PRs into hard 422s instead of inline comments.
async fn fetch_pr_diff_lines(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<std::collections::HashMap<String, std::collections::HashSet<u32>>, GitError> {
    #[derive(Deserialize)]
    struct PullRequestFile {
        filename: String,
        patch: Option<String>,
    }

    let mut next_url = Some(format!(
        "https://api.github.com/repos/{owner}/{repo}/pulls/{pr_number}/files?per_page=100"
    ));
    let mut result = std::collections::HashMap::new();

    while let Some(url) = next_url {
        log::info!("Fetching PR files from: {url}");

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "staged-app")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| GitError::CommandFailed(format!("Failed to fetch PR files: {e}")))?;

        if !response.status().is_success() {
            return Err(GitError::CommandFailed(format!(
                "Failed to fetch PR files from {}/{} PR #{}: {}",
                owner,
                repo,
                pr_number,
                response.status()
            )));
        }

        next_url = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_link_next)
            .map(ToString::to_string);

        let files: Vec<PullRequestFile> = response
            .json()
            .await
            .map_err(|e| GitError::CommandFailed(format!("Failed to parse PR files: {e}")))?;

        for file in files {
            result.insert(file.filename, valid_lines_from_patch(file.patch.as_deref()));
        }
    }

    Ok(result)
}

// -----------------------------------------------------------------------------
// GraphQL
// -----------------------------------------------------------------------------

/// Collect the messages from a GraphQL `errors` array, if present and non-empty.
///
/// GraphQL reports failures with HTTP 200 and a top-level `errors` array, so a
/// response has to be inspected even when the request itself "succeeded".
fn graphql_error_message(response: &serde_json::Value) -> Option<String> {
    let errors = response.get("errors")?.as_array()?;
    if errors.is_empty() {
        return None;
    }

    Some(
        errors
            .iter()
            .map(|error| {
                error
                    .get("message")
                    .and_then(|message| message.as_str())
                    .unwrap_or("unknown error")
            })
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// Send a GraphQL request to the GitHub API and return its `data` object.
///
/// Variables are passed as JSON rather than interpolated into the document:
/// comment bodies are arbitrary markdown, and hand-escaping them is a bug farm.
async fn graphql_request(
    client: &reqwest::Client,
    token: &str,
    query: &str,
    variables: serde_json::Value,
    context: &str,
) -> Result<serde_json::Value, GitError> {
    let response = client
        .post("https://api.github.com/graphql")
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "staged-app")
        .json(&serde_json::json!({ "query": query, "variables": variables }))
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("{context}: {e}")))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| GitError::CommandFailed(format!("{context}: {e}")))?;

    if !status.is_success() {
        return Err(GitError::CommandFailed(format!(
            "{context}: {status} - {body}"
        )));
    }

    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| GitError::CommandFailed(format!("{context}: {e}")))?;

    if let Some(message) = graphql_error_message(&parsed) {
        return Err(GitError::CommandFailed(format!("{context}: {message}")));
    }

    parsed
        .get("data")
        .filter(|data| !data.is_null())
        .cloned()
        .ok_or_else(|| GitError::CommandFailed(format!("{context}: response contained no data")))
}

// -----------------------------------------------------------------------------
// Pending reviews
// -----------------------------------------------------------------------------

/// Find the GraphQL node ID of the viewer's pending (draft) review on a PR.
///
/// One GraphQL round trip instead of `GET /user` + `GET /pulls/{n}/reviews`, and
/// `states: [PENDING]` filters server-side so a PR with many reviews cannot push
/// the pending one off the first page.
async fn find_viewer_pending_review(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Option<String>, GitError> {
    const QUERY: &str = r"query($owner: String!, $name: String!, $number: Int!) {
        repository(owner: $owner, name: $name) {
            pullRequest(number: $number) {
                reviews(states: [PENDING], first: 20) {
                    nodes { id viewerDidAuthor }
                }
            }
        }
    }";

    let data = graphql_request(
        client,
        token,
        QUERY,
        serde_json::json!({ "owner": owner, "name": repo, "number": pr_number }),
        "Failed to look up pending review",
    )
    .await?;

    Ok(data
        .pointer("/repository/pullRequest/reviews/nodes")
        .and_then(|nodes| nodes.as_array())
        .and_then(|nodes| {
            nodes.iter().find(|node| {
                node.get("viewerDidAuthor")
                    .and_then(|authored| authored.as_bool())
                    .unwrap_or(false)
            })
        })
        .and_then(|node| node.get("id"))
        .and_then(|id| id.as_str())
        .map(ToString::to_string))
}

/// [`find_viewer_pending_review`], degraded to "no pending review" on failure.
///
/// Appending to a draft review is an improvement on posting a standalone
/// comment, not a precondition for it, so a failed lookup must not block the
/// post: the caller falls back to `POST /pulls/{n}/comments` and the conflict
/// retry still covers the case where a draft really does exist.
async fn find_viewer_pending_review_or_none(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Option<String> {
    match find_viewer_pending_review(client, token, owner, repo, pr_number).await {
        Ok(review_node_id) => review_node_id,
        Err(e) => {
            log::warn!(
                "Failed to check for a pending review on {owner}/{repo} PR #{pr_number}: {e}"
            );
            None
        }
    }
}

/// Build the `AddPullRequestReviewThreadInput` payload for a review comment.
fn build_add_thread_input(
    gh_comment: &GitHubReviewComment,
    review_node_id: &str,
) -> serde_json::Value {
    let mut input = serde_json::json!({
        "pullRequestReviewId": review_node_id,
        "path": gh_comment.path,
        "body": gh_comment.body,
        "line": gh_comment.line,
        "side": gh_comment.side,
        "subjectType": "LINE",
    });

    // Only multi-line comments carry a start position; sending one for a
    // single-line comment is rejected.
    if let Some(start_line) = gh_comment.start_line {
        input["startLine"] = serde_json::json!(start_line);
    }
    if let Some(start_side) = gh_comment.start_side {
        input["startSide"] = serde_json::json!(start_side);
    }

    input
}

/// Parse a GraphQL `fullDatabaseId`, a `BigInt` scalar serialized as a string.
fn parse_full_database_id(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::String(id) => id.parse().ok(),
        serde_json::Value::Number(id) => id.as_i64(),
        _ => None,
    }
}

/// Read the `{ fullDatabaseId, url }` pair the pending-review mutations return.
///
/// `fullDatabaseId` is the REST comment ID, which is what the stored
/// `#discussion_r{id}` anchor is built from and what later edits address. The
/// GraphQL schema does not expose `databaseId` on a review comment.
fn parse_pending_comment_result(
    comment: &serde_json::Value,
) -> Result<GitHubCommentResult, GitError> {
    let comment_id = comment
        .get("fullDatabaseId")
        .and_then(parse_full_database_id)
        .ok_or_else(|| {
            GitError::CommandFailed(
                "Failed to parse the ID of the comment in the pending review".to_string(),
            )
        })?;
    let comment_url = comment
        .get("url")
        .and_then(|url| url.as_str())
        .ok_or_else(|| {
            GitError::CommandFailed(
                "Failed to parse the URL of the comment in the pending review".to_string(),
            )
        })?
        .to_string();

    Ok(GitHubCommentResult {
        comment_url,
        comment_id,
        comment_type: "review".to_string(),
        pending: true,
    })
}

/// Append a comment to an existing pending review.
///
/// REST has no endpoint for this: `POST /pulls/{n}/comments` creates a
/// *submitted* one-comment review, and GitHub allows one pending review per user
/// per PR, so it 422s when the viewer already holds a draft.
async fn add_comment_to_pending_review(
    client: &reqwest::Client,
    token: &str,
    review_node_id: &str,
    gh_comment: &GitHubReviewComment,
) -> Result<GitHubCommentResult, GitError> {
    const MUTATION: &str = r"mutation($input: AddPullRequestReviewThreadInput!) {
        addPullRequestReviewThread(input: $input) {
            thread {
                comments(first: 1) {
                    nodes { fullDatabaseId url }
                }
            }
        }
    }";

    let data = graphql_request(
        client,
        token,
        MUTATION,
        serde_json::json!({ "input": build_add_thread_input(gh_comment, review_node_id) }),
        "Failed to add comment to pending review",
    )
    .await?;

    let comment = data
        .pointer("/addPullRequestReviewThread/thread/comments/nodes/0")
        .ok_or_else(|| {
            GitError::CommandFailed(
                "Failed to add comment to pending review: GitHub returned no comment".to_string(),
            )
        })?;

    parse_pending_comment_result(comment)
}

/// Find the GraphQL node ID of a review comment sitting in the viewer's pending
/// review.
///
/// The REST review-comment endpoints do not list draft comments, so an edit to
/// one has to go through GraphQL — which addresses comments by node ID, while
/// all Staged persists is the REST database ID. GraphQL cannot look a comment up
/// by database ID, so the viewer's draft is scanned for the match.
///
/// Bounded to the first 100 comments of the draft. Past that the caller reports
/// the failure it already has rather than claiming a false success.
async fn find_pending_review_comment(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
    comment_id: i64,
) -> Result<Option<String>, GitError> {
    const QUERY: &str = r"query($owner: String!, $name: String!, $number: Int!) {
        repository(owner: $owner, name: $name) {
            pullRequest(number: $number) {
                reviews(states: [PENDING], first: 20) {
                    nodes {
                        viewerDidAuthor
                        comments(first: 100) {
                            nodes { id fullDatabaseId }
                        }
                    }
                }
            }
        }
    }";

    let data = graphql_request(
        client,
        token,
        QUERY,
        serde_json::json!({ "owner": owner, "name": repo, "number": pr_number }),
        "Failed to look up comment in pending review",
    )
    .await?;

    Ok(find_pending_review_comment_in_response(&data, comment_id))
}

/// Pure half of [`find_pending_review_comment`].
fn find_pending_review_comment_in_response(
    data: &serde_json::Value,
    comment_id: i64,
) -> Option<String> {
    data.pointer("/repository/pullRequest/reviews/nodes")?
        .as_array()?
        .iter()
        .filter(|review| {
            review
                .get("viewerDidAuthor")
                .and_then(|authored| authored.as_bool())
                .unwrap_or(false)
        })
        .filter_map(|review| review.pointer("/comments/nodes")?.as_array())
        .flatten()
        .find(|comment| {
            comment
                .get("fullDatabaseId")
                .and_then(parse_full_database_id)
                == Some(comment_id)
        })
        .and_then(|comment| comment.get("id"))
        .and_then(|id| id.as_str())
        .map(ToString::to_string)
}

/// [`find_pending_review_comment`], degraded to "not in a pending review" on
/// failure.
///
/// The caller only reaches this after a REST edit has already failed, and that
/// failure is the more useful one to report, so a broken lookup must not replace
/// it with a GraphQL error.
async fn find_pending_review_comment_or_none(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
    comment_id: i64,
) -> Option<String> {
    match find_pending_review_comment(client, token, owner, repo, pr_number, comment_id).await {
        Ok(comment_node_id) => comment_node_id,
        Err(e) => {
            log::warn!(
                "Failed to look up comment {comment_id} in a pending review on \
                 {owner}/{repo} PR #{pr_number}: {e}"
            );
            None
        }
    }
}

/// Edit a review comment that is still part of a pending review.
async fn update_pending_review_comment(
    client: &reqwest::Client,
    token: &str,
    comment_node_id: &str,
    body: &str,
) -> Result<GitHubCommentResult, GitError> {
    const MUTATION: &str = r"mutation($input: UpdatePullRequestReviewCommentInput!) {
        updatePullRequestReviewComment(input: $input) {
            pullRequestReviewComment { fullDatabaseId url }
        }
    }";

    let data = graphql_request(
        client,
        token,
        MUTATION,
        serde_json::json!({
            "input": { "pullRequestReviewCommentId": comment_node_id, "body": body },
        }),
        "Failed to update comment in pending review",
    )
    .await?;

    let comment = data
        .pointer("/updatePullRequestReviewComment/pullRequestReviewComment")
        .ok_or_else(|| {
            GitError::CommandFailed(
                "Failed to update comment in pending review: GitHub returned no comment"
                    .to_string(),
            )
        })?;

    parse_pending_comment_result(comment)
}

/// Whether a `POST /pulls/{n}/comments` failure is GitHub refusing to open a
/// second pending review for the viewer.
fn is_pending_review_conflict(status: reqwest::StatusCode, body: &str) -> bool {
    status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
        && body.contains("one pending review per pull request")
}

/// Result of posting a single comment to a GitHub PR.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubCommentResult {
    /// URL to the posted comment
    pub comment_url: String,
    /// The GitHub API comment ID (for later updates)
    pub comment_id: i64,
    /// The type of comment: "review" (inline) or "issue" (fallback)
    pub comment_type: String,
    /// Whether the comment joined a pending (draft) review, and so is not yet
    /// visible to anyone but its author. Not persisted — it drives a one-shot
    /// message to the user, since a draft comment otherwise looks published.
    pub pending: bool,
}

/// Post a single comment to a GitHub PR.
///
/// Normally this creates an immediately visible standalone review comment. If the
/// viewer already has a pending (draft) review on the PR, the comment is appended
/// to that review instead, because GitHub allows only one pending review per user
/// per PR and rejects the standalone post outright.
///
/// If the comment is on a line outside the PR diff, it falls back to a regular
/// PR issue comment (not inline).
pub async fn post_single_comment_to_github(
    repo: &Path,
    pr_number: u64,
    comment: &Comment,
    current_head_sha: &str,
) -> Result<GitHubCommentResult, GitError> {
    let token = get_github_token()?;
    let (owner, repo_name) = get_github_repo(repo)?;
    let client = reqwest::Client::new();

    // Validate the branch HEAD matches the PR HEAD before creating comments.
    let pr_url = format!("https://api.github.com/repos/{owner}/{repo_name}/pulls/{pr_number}");
    let pr_response = client
        .get(&pr_url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to fetch PR: {e}")))?;

    if !pr_response.status().is_success() {
        return Err(GitError::CommandFailed(format!(
            "Failed to fetch PR: {}",
            pr_response.status()
        )));
    }

    #[derive(Deserialize)]
    struct PrHead {
        sha: String,
    }
    #[derive(Deserialize)]
    struct PrInfo {
        head: PrHead,
    }

    let pr_info: PrInfo = pr_response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse PR info: {e}")))?;

    if pr_info.head.sha != current_head_sha {
        return Err(GitError::CommandFailed(
            "Local branch and PR are not at the same commit. Push your changes first.".to_string(),
        ));
    }

    let body = github_single_comment_body(comment);

    // Try to post as an inline review comment
    let valid_lines_by_file =
        fetch_pr_diff_lines(&client, &token, &owner, &repo_name, pr_number).await?;

    let inline_comment = {
        let mut c = comment.clone();
        c.content = body.clone();
        convert_comment(&c, valid_lines_by_file.get(&comment.path))
    };

    #[derive(Deserialize)]
    struct CommentResponse {
        id: i64,
        html_url: String,
    }

    match inline_comment {
        Some(gh_comment) => {
            // A standalone comment implicitly opens and submits a one-comment
            // review, which GitHub refuses while the viewer holds a draft. Append
            // to the draft instead so the comment joins the review in progress.
            if let Some(review_node_id) =
                find_viewer_pending_review_or_none(&client, &token, &owner, &repo_name, pr_number)
                    .await
            {
                return add_comment_to_pending_review(
                    &client,
                    &token,
                    &review_node_id,
                    &gh_comment,
                )
                .await;
            }

            // Post as a direct pull request review comment (gives us the comment ID)
            let url = format!(
                "https://api.github.com/repos/{owner}/{repo_name}/pulls/{pr_number}/comments"
            );
            let mut request = serde_json::json!({
                "body": gh_comment.body,
                "commit_id": current_head_sha,
                "path": gh_comment.path,
                "line": gh_comment.line,
                "side": gh_comment.side,
            });
            if let Some(start_line) = gh_comment.start_line {
                request["start_line"] = serde_json::json!(start_line);
            }
            if let Some(start_side) = gh_comment.start_side {
                request["start_side"] = serde_json::json!(start_side);
            }

            let response = client
                .post(&url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "staged-app")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .json(&request)
                .send()
                .await
                .map_err(|e| GitError::CommandFailed(format!("Failed to post comment: {e}")))?;

            let status = response.status();
            if !status.is_success() {
                let error_body = response.text().await.unwrap_or_default();

                // A review can be started between the lookup above and this POST,
                // and the lookup itself is allowed to fail silently.
                if is_pending_review_conflict(status, &error_body) {
                    if let Some(review_node_id) = find_viewer_pending_review_or_none(
                        &client, &token, &owner, &repo_name, pr_number,
                    )
                    .await
                    {
                        return add_comment_to_pending_review(
                            &client,
                            &token,
                            &review_node_id,
                            &gh_comment,
                        )
                        .await;
                    }
                }

                return Err(GitError::CommandFailed(format!(
                    "Failed to post comment: {status} - {error_body}"
                )));
            }

            let created: CommentResponse = response.json().await.map_err(|e| {
                GitError::CommandFailed(format!("Failed to parse comment response: {e}"))
            })?;

            Ok(GitHubCommentResult {
                comment_url: created.html_url,
                comment_id: created.id,
                comment_type: "review".to_string(),
                pending: false,
            })
        }
        None => {
            // Fall back to a regular PR issue comment (not inline)
            let fallback_body = github_issue_comment_body(comment, &body);

            let url = format!(
                "https://api.github.com/repos/{owner}/{repo_name}/issues/{pr_number}/comments"
            );

            let response = client
                .post(&url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "staged-app")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .json(&serde_json::json!({ "body": fallback_body }))
                .send()
                .await
                .map_err(|e| GitError::CommandFailed(format!("Failed to post comment: {e}")))?;

            let status = response.status();
            if !status.is_success() {
                let error_body = response.text().await.unwrap_or_default();
                return Err(GitError::CommandFailed(format!(
                    "Failed to post comment: {status} - {error_body}"
                )));
            }

            let created: CommentResponse = response.json().await.map_err(|e| {
                GitError::CommandFailed(format!("Failed to parse comment response: {e}"))
            })?;

            Ok(GitHubCommentResult {
                comment_url: created.html_url,
                comment_id: created.id,
                comment_type: "issue".to_string(),
                pending: false,
            })
        }
    }
}

/// Update an existing comment on GitHub.
///
/// A review comment that joined a pending review may not be addressable through
/// `PATCH /pulls/comments/{id}` at all — the REST review-comment endpoints do
/// not list draft comments — so a not-found response falls back to editing the
/// comment inside the draft over GraphQL.
pub async fn update_comment_on_github(
    repo: &Path,
    pr_number: u64,
    github_comment_id: i64,
    github_comment_type: &str,
    body: &str,
) -> Result<GitHubCommentResult, GitError> {
    let token = get_github_token()?;
    let (owner, repo_name) = get_github_repo(repo)?;
    let client = reqwest::Client::new();

    let url = match github_comment_type {
        "review" => format!(
            "https://api.github.com/repos/{owner}/{repo_name}/pulls/comments/{github_comment_id}"
        ),
        "issue" => format!(
            "https://api.github.com/repos/{owner}/{repo_name}/issues/comments/{github_comment_id}"
        ),
        other => {
            return Err(GitError::CommandFailed(format!(
                "Unknown GitHub comment type: {other}"
            )));
        }
    };

    let response = client
        .patch(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&serde_json::json!({ "body": body }))
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to update comment: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();

        // A comment that joined a draft review is invisible to REST, so a
        // not-found here may just mean "still pending" rather than "gone".
        if github_comment_type == "review" && status == reqwest::StatusCode::NOT_FOUND {
            if let Some(comment_node_id) = find_pending_review_comment_or_none(
                &client,
                &token,
                &owner,
                &repo_name,
                pr_number,
                github_comment_id,
            )
            .await
            {
                return update_pending_review_comment(&client, &token, &comment_node_id, body)
                    .await;
            }
        }

        return Err(GitError::CommandFailed(format!(
            "Failed to update comment: {status} - {error_body}"
        )));
    }

    #[derive(Deserialize)]
    struct CommentResponse {
        id: i64,
        html_url: String,
    }

    let updated: CommentResponse = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse comment response: {e}")))?;

    Ok(GitHubCommentResult {
        comment_url: updated.html_url,
        comment_id: updated.id,
        comment_type: github_comment_type.to_string(),
        // A REST edit reached the comment, so it is published; a draft comment
        // takes the GraphQL path above and reports itself as pending there.
        pending: false,
    })
}

// =============================================================================
// Pull Request Creation
// =============================================================================

/// Result of creating a pull request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePrResult {
    /// The PR number
    pub number: u64,
    /// URL to the PR on GitHub
    pub url: String,
}

/// Extended PR info including body and state (for checking existing PRs)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestInfo {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub author: String,
    pub base_ref: String,
    pub head_ref: String,
    pub draft: bool,
    pub state: String,
    pub url: String,
}

/// Response from `gh pr view --json`
#[derive(Debug, Deserialize)]
struct GhPrViewItem {
    number: u64,
    title: String,
    body: String,
    author: GhAuthor,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    state: String,
    url: String,
}

impl From<GhPrViewItem> for PullRequestInfo {
    fn from(item: GhPrViewItem) -> Self {
        PullRequestInfo {
            number: item.number,
            title: item.title,
            body: item.body,
            author: item.author.login,
            base_ref: item.base_ref_name,
            head_ref: item.head_ref_name,
            draft: item.is_draft,
            state: item.state.to_lowercase(),
            url: item.url,
        }
    }
}

/// Get the PR associated with a branch (if one exists).
/// Returns None if no PR exists for this branch.
pub fn get_pr_for_branch(repo: &Path, branch: &str) -> Result<Option<PullRequestInfo>, GitError> {
    let output = run_gh(
        repo,
        &[
            "pr",
            "view",
            branch,
            "--json=number,title,body,author,baseRefName,headRefName,isDraft,state,url",
        ],
    );

    match output {
        Ok(json) => {
            let item: GhPrViewItem =
                serde_json::from_str(&json).map_err(|e| GitError::CommandFailed(e.to_string()))?;
            Ok(Some(item.into()))
        }
        Err(e) => {
            // "no pull requests found" is not an error, just means no PR exists
            let msg = e.to_string();
            if msg.contains("no pull requests found") || msg.contains("Could not resolve") {
                Ok(None)
            } else {
                Err(e)
            }
        }
    }
}

/// Response from `gh pr view --json` for status fields
#[derive(Debug, Deserialize)]
struct GhPrStatusItem {
    state: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    mergeable: String,
    #[serde(rename = "reviewDecision")]
    review_decision: Option<String>,
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Vec<GhStatusCheck>,
    #[serde(rename = "headRefOid")]
    head_ref_oid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhStatusCheck {
    #[serde(rename = "__typename")]
    typename: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(rename = "detailsUrl", default)]
    details_url: Option<String>,
    #[serde(rename = "targetUrl", default)]
    target_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckRollupAnalysis {
    total: u32,
    passed: u32,
    failed: u32,
    pending: u32,
    state: String,
    failed_checks: Vec<FailedCheck>,
}

fn check_rollup_state(check: &GhStatusCheck) -> Option<String> {
    let state = if check.typename == "StatusContext" {
        check.state.as_deref()
    } else if check.typename == "CheckRun" {
        check.conclusion.as_deref().or(check.status.as_deref())
    } else {
        None
    };

    state.map(str::to_ascii_uppercase)
}

fn check_rollup_name(check: &GhStatusCheck) -> String {
    check
        .name
        .as_deref()
        .or(check.context.as_deref())
        .unwrap_or("Unnamed check")
        .to_string()
}

fn check_rollup_details_url(check: &GhStatusCheck) -> Option<String> {
    check
        .details_url
        .as_ref()
        .or(check.target_url.as_ref())
        .cloned()
}

fn analyze_status_check_rollup(checks: &[GhStatusCheck]) -> CheckRollupAnalysis {
    let mut total = 0u32;
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut pending = 0u32;
    let mut failed_checks = Vec::new();

    for check in checks {
        total += 1;

        match check_rollup_state(check).as_deref() {
            Some("SUCCESS") | Some("COMPLETED") => passed += 1,
            Some(state @ ("FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED")) => {
                failed += 1;
                failed_checks.push(FailedCheck {
                    name: check_rollup_name(check),
                    state: state.to_string(),
                    details_url: check_rollup_details_url(check),
                });
            }
            Some("PENDING") | Some("IN_PROGRESS") | Some("QUEUED") | Some("WAITING") => {
                pending += 1
            }
            _ => pending += 1,
        }
    }

    let state = if total == 0 {
        "EXPECTED".to_string()
    } else if failed > 0 {
        "FAILURE".to_string()
    } else if pending > 0 {
        "PENDING".to_string()
    } else {
        "SUCCESS".to_string()
    };

    CheckRollupAnalysis {
        total,
        passed,
        failed,
        pending,
        state,
        failed_checks,
    }
}

/// Fetch detailed status information for a PR by number.
/// This includes CI checks, review decisions, and mergeability.
pub fn fetch_pr_status(repo: &Path, pr_number: u64) -> Result<PrStatus, GitError> {
    let output = run_gh(
        repo,
        &[
            "pr",
            "view",
            &pr_number.to_string(),
            "--json=state,isDraft,mergeable,reviewDecision,statusCheckRollup,headRefOid",
        ],
    )?;

    let item: GhPrStatusItem =
        serde_json::from_str(&output).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    let checks = analyze_status_check_rollup(&item.status_check_rollup);

    Ok(PrStatus {
        state: item.state.to_uppercase(),
        is_draft: item.is_draft,
        mergeable: item.mergeable.to_uppercase(),
        review_decision: item.review_decision,
        checks_summary: ChecksSummary {
            total: checks.total,
            passed: checks.passed,
            failed: checks.failed,
            pending: checks.pending,
            state: checks.state,
        },
        head_sha: item.head_ref_oid,
        failed_checks: checks.failed_checks,
    })
}

/// Fetch PR status using repo slug format (owner/repo) instead of local path.
/// Useful when you don't have a local clone.
pub fn fetch_pr_status_for_repo(github_repo: &str, pr_number: u64) -> Result<PrStatus, GitError> {
    let gh_args = &[
        "pr",
        "view",
        &pr_number.to_string(),
        "-R",
        github_repo,
        "--json=state,isDraft,mergeable,reviewDecision,statusCheckRollup,headRefOid",
    ];
    let output = match run_gh_global(gh_args) {
        Ok(output) => output,
        Err(e) => {
            log::error!(
                "fetch_pr_status_for_repo: gh command failed for repo={}, pr_number={}: {}",
                github_repo,
                pr_number,
                e
            );
            return Err(e);
        }
    };

    let item: GhPrStatusItem = match serde_json::from_str(&output) {
        Ok(item) => item,
        Err(e) => {
            log::error!(
                "fetch_pr_status_for_repo: failed to parse gh output for repo={}, pr_number={}: {}. Raw output: {}",
                github_repo,
                pr_number,
                e,
                output
            );
            return Err(GitError::CommandFailed(e.to_string()));
        }
    };

    let checks = analyze_status_check_rollup(&item.status_check_rollup);

    Ok(PrStatus {
        state: item.state.to_uppercase(),
        is_draft: item.is_draft,
        mergeable: item.mergeable.to_uppercase(),
        review_decision: item.review_decision,
        checks_summary: ChecksSummary {
            total: checks.total,
            passed: checks.passed,
            failed: checks.failed,
            pending: checks.pending,
            state: checks.state,
        },
        head_sha: item.head_ref_oid,
        failed_checks: checks.failed_checks,
    })
}

/// Fetch the canonical GitHub URL for a pull request.
///
/// PRs always live on the base (upstream) repository, but the stored repo may
/// be the fork (head) repo for fork PRs. This function first tries the given
/// repo, then falls back to checking if it's a fork and querying the parent.
pub fn fetch_pr_url(github_repo: &str, pr_number: u64) -> Result<String, GitError> {
    let pr_num_str = pr_number.to_string();

    // Try the stored repo first — works when it's already the base repo.
    if let Ok(url) = fetch_pr_url_from_repo(github_repo, &pr_num_str) {
        return Ok(url);
    }

    // The PR wasn't found — the stored repo may be the fork. Check for a parent.
    let parent_args = &["repo", "view", github_repo, "--json=parent"];
    let parent_output = run_gh_global(parent_args).map_err(|e| {
        log::warn!(
            "fetch_pr_url: failed to look up parent repo for {}: {}",
            github_repo,
            e
        );
        e
    })?;

    let view: ParentRepoView = serde_json::from_str(&parent_output)
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse parent repo: {e}")))?;

    match view.parent {
        Some(parent) => {
            let parent_slug = format!("{}/{}", parent.owner.login, parent.name);
            fetch_pr_url_from_repo(&parent_slug, &pr_num_str)
        }
        None => {
            // Not a fork — fall back to constructing the URL.
            Ok(format!(
                "https://github.com/{}/pull/{}",
                github_repo, pr_number
            ))
        }
    }
}

/// If `github_repo` is a fork, return the parent repo slug. Returns `None` otherwise.
pub fn get_parent_repo(github_repo: &str) -> Result<Option<String>, GitError> {
    let args = &["repo", "view", github_repo, "--json=parent"];
    let output = run_gh_global(args)?;

    let view: ParentRepoView = serde_json::from_str(&output)
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse repo view: {e}")))?;

    Ok(view.parent.map(|p| format!("{}/{}", p.owner.login, p.name)))
}

fn fetch_pr_url_from_repo(github_repo: &str, pr_num_str: &str) -> Result<String, GitError> {
    let args = &["pr", "view", pr_num_str, "-R", github_repo, "--json=url"];
    let output = run_gh_global(args)?;

    #[derive(Deserialize)]
    struct PrUrl {
        url: String,
    }

    let parsed: PrUrl = serde_json::from_str(&output)
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse PR URL: {e}")))?;
    Ok(parsed.url)
}

/// Push a branch to the remote.
/// If force is true, uses --force-with-lease for safer force pushing.
pub fn push_branch(repo: &Path, branch: &str, force: bool) -> Result<(), GitError> {
    use super::cli;

    let mut args = vec!["push", "-u", "origin", branch];
    if force {
        args.push("--force-with-lease");
    }

    cli::run(repo, &args)?;
    Ok(())
}

/// Create a new pull request.
/// The branch must be pushed to the remote first.
pub fn create_pull_request(
    repo: &Path,
    head_branch: &str,
    base_branch: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<CreatePrResult, GitError> {
    let mut args = vec![
        "pr",
        "create",
        "--head",
        head_branch,
        "--base",
        base_branch,
        "--title",
        title,
        "--body",
        body,
    ];

    if draft {
        args.push("--draft");
    }

    let output = run_gh(repo, &args)?;

    // gh pr create outputs the PR URL on success
    let url = output.trim().to_string();

    // Extract PR number from URL (e.g., https://github.com/owner/repo/pull/123)
    let number = url
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| {
            GitError::CommandFailed(format!("Could not parse PR number from URL: {url}"))
        })?;

    Ok(CreatePrResult { number, url })
}

/// Update an existing pull request's title and/or body.
pub async fn update_pull_request(
    repo: &Path,
    pr_number: u64,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<(), GitError> {
    if title.is_none() && body.is_none() {
        return Ok(());
    }

    let token = get_github_token()?;
    let (owner, repo_name) = get_github_repo(repo)?;

    // Use GraphQL API directly to avoid the deprecated projectCards field
    // that gh pr edit queries by default
    let client = reqwest::Client::new();

    // First, get the PR's node ID
    const PR_ID_QUERY: &str = r"query($owner: String!, $name: String!, $number: Int!) {
        repository(owner: $owner, name: $name) {
            pullRequest(number: $number) { id }
        }
    }";

    let data = graphql_request(
        &client,
        &token,
        PR_ID_QUERY,
        serde_json::json!({ "owner": owner, "name": repo_name, "number": pr_number }),
        "Failed to get PR node ID",
    )
    .await?;

    let pr_id = data
        .pointer("/repository/pullRequest/id")
        .and_then(|id| id.as_str())
        .ok_or_else(|| GitError::CommandFailed("Failed to parse PR ID".to_string()))?;

    // Now update the PR. Titles and bodies go over as variables — they are
    // user-authored text, so interpolating them into the document is not safe.
    let mut input = serde_json::json!({ "pullRequestId": pr_id });
    if let Some(title) = title {
        input["title"] = serde_json::json!(title);
    }
    if let Some(body) = body {
        input["body"] = serde_json::json!(body);
    }

    const UPDATE_MUTATION: &str = r"mutation($input: UpdatePullRequestInput!) {
        updatePullRequest(input: $input) {
            pullRequest { id }
        }
    }";

    graphql_request(
        &client,
        &token,
        UPDATE_MUTATION,
        serde_json::json!({ "input": input }),
        "Failed to update PR",
    )
    .await?;

    Ok(())
}

// =============================================================================
// Subpath Validation
// =============================================================================

fn invalid_repo_path_error() -> GitError {
    GitError::CommandFailed("Invalid path in repo".to_string())
}

fn is_github_not_found_error(msg: &str) -> bool {
    msg.contains("Not Found") || msg.contains("HTTP 404")
}

fn normalize_repo_subpath(subpath: &str) -> Result<Option<PathBuf>, GitError> {
    let trimmed = subpath.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.starts_with('/') || Path::new(trimmed).is_absolute() {
        return Err(invalid_repo_path_error());
    }

    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut normalized = PathBuf::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(invalid_repo_path_error());
        }
        normalized.push(segment);
    }

    Ok(Some(normalized))
}

fn normalized_repo_subpath_string(subpath: &str) -> Result<Option<String>, GitError> {
    let Some(relative_path) = normalize_repo_subpath(subpath)? else {
        return Ok(None);
    };
    Ok(Some(
        relative_path
            .iter()
            .map(|segment| segment.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
    ))
}

fn local_subpath_is_tracked_dir(clone_path: &Path, subpath: &str) -> Result<bool, GitError> {
    let Some(git_path) = normalized_repo_subpath_string(subpath)? else {
        return Ok(true);
    };

    let object = format!("HEAD:{git_path}");
    match super::cli::run_lite(clone_path, &["cat-file", "-t", &object]) {
        Ok(kind) => Ok(kind.trim() == "tree"),
        Err(GitError::CommandFailed(_)) => Ok(false),
        Err(err) => Err(err),
    }
}

fn local_repo_subpath_is_tracked_dir(
    github_repo: &str,
    subpath: &str,
) -> Result<Option<bool>, GitError> {
    let Some(clone_path) = crate::paths::clone_path_for(github_repo) else {
        return Ok(None);
    };
    if !clone_path.join(".git").exists() {
        return Ok(None);
    }
    local_subpath_is_tracked_dir(&clone_path, subpath).map(Some)
}

fn validation_result_after_github_error(
    local_subpath_is_tracked_dir: Option<bool>,
    github_error_msg: &str,
) -> Option<Result<(), GitError>> {
    match local_subpath_is_tracked_dir {
        Some(true) => Some(Ok(())),
        Some(false) => Some(Err(invalid_repo_path_error())),
        None if is_github_not_found_error(github_error_msg) => Some(Err(invalid_repo_path_error())),
        None => None,
    }
}

fn list_local_directories_at_clone(clone_path: &Path, path: &str) -> Result<Vec<String>, GitError> {
    let target = match normalize_repo_subpath(path)? {
        Some(relative_path) => clone_path.join(relative_path),
        None => clone_path.to_path_buf(),
    };

    if !target.is_dir() {
        return Ok(vec![]);
    }

    let entries = std::fs::read_dir(&target).map_err(|e| {
        GitError::CommandFailed(format!(
            "Failed to read local repo path '{}': {e}",
            target.display()
        ))
    })?;

    let mut dirs = entries
        .filter_map(Result::ok)
        .filter_map(|entry| match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => entry.file_name().into_string().ok(),
            _ => None,
        })
        .collect::<Vec<_>>();
    dirs.sort();
    Ok(dirs)
}

fn list_local_repo_directories(
    github_repo: &str,
    path: &str,
) -> Result<Option<Vec<String>>, GitError> {
    let Some(clone_path) = crate::paths::clone_path_for(github_repo) else {
        return Ok(None);
    };
    if !clone_path.join(".git").exists() {
        return Ok(None);
    }
    list_local_directories_at_clone(&clone_path, path).map(Some)
}

/// Validate that a subpath exists as a directory in a GitHub repository.
///
/// Uses the GitHub contents API to check that the path exists and is a
/// directory (the API returns an array for directories). If GitHub cannot be
/// reached, falls back to the existing local clone's HEAD tree when one is
/// available.
/// Returns an error if the path does not exist or points to a file rather than
/// a directory.
pub fn validate_subpath_in_repo(github_repo: &str, subpath: &str) -> Result<(), GitError> {
    let Some(trimmed) = normalized_repo_subpath_string(subpath)? else {
        return Ok(());
    };

    let endpoint = format!("repos/{github_repo}/contents/{trimmed}");
    match run_gh_global(&["api", &endpoint]) {
        Ok(body) => {
            // The contents API returns a JSON array for directories and a JSON
            // object for files. A quick heuristic: arrays start with '['.
            let body = body.trim_start();
            if body.starts_with('[') {
                Ok(())
            } else {
                Err(invalid_repo_path_error())
            }
        }
        Err(e) => {
            let msg = e.to_string();
            let local_result = local_repo_subpath_is_tracked_dir(github_repo, &trimmed)?;
            match validation_result_after_github_error(local_result, &msg) {
                Some(Ok(())) => {
                    log::warn!(
                        "validated '{}' in '{}' from local clone after GitHub validation failed: {}",
                        trimmed,
                        github_repo,
                        msg
                    );
                    Ok(())
                }
                Some(Err(err)) => Err(err),
                None => Err(e),
            }
        }
    }
}

/// List directories at a given path in a GitHub repository.
/// Returns a list of directory names (not files) at the specified path.
/// If `path` is empty, lists directories at the repository root. If GitHub
/// cannot be reached, falls back to the existing local clone when one is
/// available.
pub fn list_repo_directories(github_repo: &str, path: &str) -> Result<Vec<String>, GitError> {
    let trimmed = normalized_repo_subpath_string(path)?.unwrap_or_default();
    let endpoint = if trimmed.is_empty() {
        format!("repos/{github_repo}/contents")
    } else {
        format!("repos/{github_repo}/contents/{trimmed}")
    };

    match run_gh_global(&["api", &endpoint]) {
        Ok(body) => {
            let body = body.trim_start();
            if !body.starts_with('[') {
                // Path points to a file, not a directory – no subdirectories to list
                return Ok(vec![]);
            }

            #[derive(Deserialize)]
            struct Entry {
                name: String,
                #[serde(rename = "type")]
                entry_type: String,
            }

            let entries: Vec<Entry> =
                serde_json::from_str(body).map_err(|e| GitError::CommandFailed(e.to_string()))?;

            let dirs: Vec<String> = entries
                .into_iter()
                .filter(|e| e.entry_type == "dir")
                .map(|e| e.name)
                .collect();

            Ok(dirs)
        }
        Err(e) => {
            let msg = e.to_string();
            match list_local_repo_directories(github_repo, &trimmed)? {
                Some(dirs) => {
                    log::warn!(
                        "listed directories for '{}' in '{}' from local clone after GitHub listing failed: {}",
                        trimmed,
                        github_repo,
                        msg
                    );
                    Ok(dirs)
                }
                None if is_github_not_found_error(&msg) => Ok(vec![]),
                None => Err(e),
            }
        }
    }
}

// =============================================================================
// Monorepo Detection
// =============================================================================

/// Check if a repository is likely a monorepo by counting modules in MODULES.yaml
/// (Block's Owner Owl system). Returns the number of YAML document separators (`---`)
/// found in the file, which corresponds to the module count. Returns 0 if the file
/// does not exist. A count of 20+ is considered a monorepo.
pub fn check_monorepo_modules(github_repo: &str) -> Result<u32, GitError> {
    let modules_endpoint = format!("repos/{github_repo}/contents/MODULES.yaml");

    let modules_result = run_gh_global(&["api", &modules_endpoint]);
    match modules_result {
        Ok(json) => {
            #[derive(Debug, Deserialize)]
            struct FileContent {
                content: String,
            }

            if let Ok(file) = serde_json::from_str::<FileContent>(&json) {
                use base64::Engine;
                if let Ok(decoded) =
                    base64::engine::general_purpose::STANDARD.decode(file.content.replace('\n', ""))
                {
                    if let Ok(content) = String::from_utf8(decoded) {
                        let module_count =
                            content.lines().filter(|line| line.trim() == "---").count() as u32;
                        return Ok(module_count);
                    }
                }
            }

            Ok(0)
        }
        Err(e) => {
            let error_msg = e.to_string();
            if error_msg.contains("Not Found") || error_msg.contains("HTTP 404") {
                Ok(0)
            } else {
                Err(e)
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    static CLONE_LOCK_TEST_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> =
        std::sync::OnceLock::new();

    fn lock_clone_lock_tests() -> std::sync::MutexGuard<'static, ()> {
        CLONE_LOCK_TEST_MUTEX
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear_clone_lock_map() {
        if let Some(locks) = REPO_CLONE_LOCKS.get() {
            locks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clear();
        }
    }

    fn check_run(
        name: &str,
        status: &str,
        conclusion: &str,
        details_url: Option<&str>,
    ) -> GhStatusCheck {
        GhStatusCheck {
            typename: "CheckRun".to_string(),
            name: Some(name.to_string()),
            context: None,
            state: None,
            status: Some(status.to_string()),
            conclusion: Some(conclusion.to_string()),
            details_url: details_url.map(ToString::to_string),
            target_url: None,
        }
    }

    fn status_context(context: &str, state: &str, target_url: Option<&str>) -> GhStatusCheck {
        GhStatusCheck {
            typename: "StatusContext".to_string(),
            name: None,
            context: Some(context.to_string()),
            state: Some(state.to_string()),
            status: None,
            conclusion: None,
            details_url: None,
            target_url: target_url.map(ToString::to_string),
        }
    }

    #[test]
    fn test_local_subpath_is_tracked_dir_accepts_tracked_relative_directory() {
        let repo = crate::test_utils::TempGitRepo::new();
        std::fs::create_dir_all(repo.path().join("packages/app")).expect("create dirs");
        std::fs::write(repo.path().join("packages/app/file.txt"), "tracked").expect("write file");
        repo.commit("init");

        assert!(local_subpath_is_tracked_dir(repo.path(), "packages/app").unwrap());
        assert!(local_subpath_is_tracked_dir(repo.path(), "packages/app/").unwrap());
        assert!(!local_subpath_is_tracked_dir(repo.path(), "packages/missing").unwrap());
    }

    #[test]
    fn test_local_subpath_is_tracked_dir_rejects_unsafe_paths() {
        let temp = tempfile::tempdir().expect("tempdir");
        for path in [
            "/tmp",
            ".",
            "packages/.",
            "..",
            "../repo",
            "packages/../app",
            "packages//app",
        ] {
            let err = local_subpath_is_tracked_dir(temp.path(), path).unwrap_err();
            assert_eq!(err.to_string(), "git command failed: Invalid path in repo");
        }
    }

    #[test]
    fn test_local_subpath_is_tracked_dir_rejects_untracked_git_and_files() {
        let repo = crate::test_utils::TempGitRepo::new();
        std::fs::create_dir_all(repo.path().join("packages/app")).expect("create app dir");
        std::fs::write(repo.path().join("packages/app/file.txt"), "tracked").expect("write file");
        repo.commit("init");

        std::fs::create_dir_all(repo.path().join("node_modules/pkg"))
            .expect("create untracked dir");

        assert!(!local_subpath_is_tracked_dir(repo.path(), "node_modules").unwrap());
        assert!(!local_subpath_is_tracked_dir(repo.path(), ".git").unwrap());
        assert!(!local_subpath_is_tracked_dir(repo.path(), "packages/app/file.txt").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn test_local_subpath_is_tracked_dir_rejects_symlinked_directory() {
        let repo = crate::test_utils::TempGitRepo::new();
        let external = tempfile::tempdir().expect("external tempdir");
        std::os::unix::fs::symlink(external.path(), repo.path().join("external-link"))
            .expect("create symlink");
        repo.commit("init");

        assert!(!local_subpath_is_tracked_dir(repo.path(), "external-link").unwrap());
    }

    #[test]
    fn test_list_local_directories_at_clone_returns_sorted_child_dirs() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("packages/b")).expect("create b");
        std::fs::create_dir_all(temp.path().join("packages/a")).expect("create a");
        std::fs::write(temp.path().join("packages/file.txt"), "not a dir").expect("write file");

        assert_eq!(
            list_local_directories_at_clone(temp.path(), "packages").unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn test_validation_result_after_github_404_prefers_existing_local_dir() {
        assert!(
            validation_result_after_github_error(Some(true), "HTTP 404 Not Found")
                .expect("local fallback should decide validation")
                .is_ok()
        );
    }

    #[test]
    fn test_check_github_auth_returns_status() {
        // This test just verifies the function runs without panicking
        // Actual auth status depends on the environment
        let status = check_github_auth();
        // Either authenticated or has a setup hint
        assert!(status.authenticated || status.setup_hint.is_some());
    }

    #[test]
    fn test_clone_lock_for_repo_normalizes_repo_case() {
        let _test_guard = lock_clone_lock_tests();
        clear_clone_lock_map();

        let upper = clone_lock_for_repo("Owner/Repo");
        let lower = clone_lock_for_repo("owner/repo");

        assert!(std::sync::Arc::ptr_eq(&upper, &lower));

        clear_clone_lock_map();
    }

    #[test]
    fn test_clone_lock_for_repo_prunes_inactive_entries() {
        let _test_guard = lock_clone_lock_tests();
        clear_clone_lock_map();

        let stale_key = clone_lock_key("Owner/StaleRepo");
        {
            let stale_lock = clone_lock_for_repo("Owner/StaleRepo");
            assert!(std::sync::Arc::strong_count(&stale_lock) >= 2);
        }

        let active_lock = clone_lock_for_repo("Owner/ActiveRepo");
        drop(active_lock);

        let lock_map = REPO_CLONE_LOCKS
            .get()
            .expect("clone lock map should be initialized")
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        assert!(
            !lock_map.contains_key(&stale_key),
            "inactive clone lock entry should be pruned"
        );

        drop(lock_map);
        clear_clone_lock_map();
    }

    #[test]
    fn test_analyze_status_check_rollup_captures_failed_check_run() {
        let checks = vec![
            check_run(
                "unit-tests",
                "COMPLETED",
                "FAILURE",
                Some("https://github.com/owner/repo/actions/runs/1"),
            ),
            check_run(
                "lint",
                "COMPLETED",
                "SUCCESS",
                Some("https://github.com/owner/repo/actions/runs/2"),
            ),
        ];

        let analysis = analyze_status_check_rollup(&checks);

        assert_eq!(analysis.total, 2);
        assert_eq!(analysis.passed, 1);
        assert_eq!(analysis.failed, 1);
        assert_eq!(analysis.pending, 0);
        assert_eq!(analysis.state, "FAILURE");
        assert_eq!(
            analysis.failed_checks,
            vec![FailedCheck {
                name: "unit-tests".to_string(),
                state: "FAILURE".to_string(),
                details_url: Some("https://github.com/owner/repo/actions/runs/1".to_string()),
            }]
        );
    }

    #[test]
    fn test_analyze_status_check_rollup_captures_failed_status_context() {
        let checks = vec![
            status_context("lint", "TIMED_OUT", Some("https://ci.example.com/lint/1")),
            status_context("build", "PENDING", Some("https://ci.example.com/build/1")),
        ];

        let analysis = analyze_status_check_rollup(&checks);

        assert_eq!(analysis.total, 2);
        assert_eq!(analysis.passed, 0);
        assert_eq!(analysis.failed, 1);
        assert_eq!(analysis.pending, 1);
        assert_eq!(analysis.state, "FAILURE");
        assert_eq!(
            analysis.failed_checks,
            vec![FailedCheck {
                name: "lint".to_string(),
                state: "TIMED_OUT".to_string(),
                details_url: Some("https://ci.example.com/lint/1".to_string()),
            }]
        );
    }

    #[test]
    fn test_github_single_comment_body_adds_agent_attribution() {
        let comment = Comment::new("src/lib.rs", crate::git::Span::new(4, 5), "Looks good")
            .with_author(crate::store::CommentAuthor::Agent);

        assert_eq!(github_single_comment_body(&comment), "Looks good\n\n🤖");
    }

    #[test]
    fn test_github_single_comment_body_omits_attribution_for_humans() {
        let comment = Comment::new("src/lib.rs", crate::git::Span::new(4, 5), "Looks good")
            .with_author(crate::store::CommentAuthor::User);

        assert_eq!(github_single_comment_body(&comment), "Looks good");
    }

    #[test]
    fn test_github_issue_comment_body_includes_location_heading() {
        let comment = Comment::new("src/lib.rs", crate::git::Span::new(9, 12), "Unused here");

        assert_eq!(
            github_issue_comment_body(&comment, "Unused here"),
            "**src/lib.rs** (Lines 10-12)\n\nUnused here"
        );
    }

    #[test]
    fn test_github_comment_result_serializes_camel_case() {
        let result = GitHubCommentResult {
            comment_url: "https://github.com/owner/repo/pull/1#discussion_r123".to_string(),
            comment_id: 123,
            comment_type: "review".to_string(),
            pending: true,
        };

        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({
                "commentUrl": "https://github.com/owner/repo/pull/1#discussion_r123",
                "commentId": 123,
                "commentType": "review",
                "pending": true,
            })
        );
    }

    /// The 422 GitHub returns for `POST /pulls/{n}/comments` when the viewer
    /// already holds a pending review on the PR.
    const PENDING_REVIEW_CONFLICT_BODY: &str = r#"{"message":"Validation Failed","errors":[{"resource":"PullRequestReview","code":"custom","field":"user_id","message":"user_id can only have one pending review per pull request"}],"documentation_url":"https://docs.github.com/rest/pulls/comments#create-a-review-comment-for-a-pull-request","status":"422"}"#;

    #[test]
    fn test_is_pending_review_conflict_matches_the_real_422() {
        assert!(is_pending_review_conflict(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            PENDING_REVIEW_CONFLICT_BODY
        ));
    }

    #[test]
    fn test_is_pending_review_conflict_rejects_other_failures() {
        // Right status, unrelated validation failure.
        assert!(!is_pending_review_conflict(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            r#"{"message":"Validation Failed","errors":[{"resource":"PullRequestReviewComment","field":"line","code":"invalid"}]}"#
        ));
        // Right body, wrong status.
        assert!(!is_pending_review_conflict(
            reqwest::StatusCode::FORBIDDEN,
            PENDING_REVIEW_CONFLICT_BODY
        ));
    }

    fn review_comment(start_line: Option<u32>) -> GitHubReviewComment {
        GitHubReviewComment {
            path: "src/main.rs".to_string(),
            body: "looks good".to_string(),
            line: 42,
            side: "RIGHT",
            start_line,
            start_side: start_line.map(|_| "RIGHT"),
        }
    }

    #[test]
    fn test_build_add_thread_input_omits_start_position_for_single_line() {
        assert_eq!(
            build_add_thread_input(&review_comment(None), "PRR_node"),
            serde_json::json!({
                "pullRequestReviewId": "PRR_node",
                "path": "src/main.rs",
                "body": "looks good",
                "line": 42,
                "side": "RIGHT",
                "subjectType": "LINE",
            })
        );
    }

    #[test]
    fn test_build_add_thread_input_includes_start_position_for_multi_line() {
        assert_eq!(
            build_add_thread_input(&review_comment(Some(40)), "PRR_node"),
            serde_json::json!({
                "pullRequestReviewId": "PRR_node",
                "path": "src/main.rs",
                "body": "looks good",
                "line": 42,
                "side": "RIGHT",
                "subjectType": "LINE",
                "startLine": 40,
                "startSide": "RIGHT",
            })
        );
    }

    #[test]
    fn test_build_add_thread_input_preserves_awkward_bodies() {
        // Bodies are markdown: quotes, newlines, backslashes and backticks all
        // have to survive as-is, which is why they go over as a JSON variable
        // rather than being interpolated into the GraphQL document.
        let body = "he said \"hi\"\n\n```rust\nlet p = \"C:\\\\tmp\";\n```\n\\end";
        let mut gh_comment = review_comment(None);
        gh_comment.body = body.to_string();

        let input = build_add_thread_input(&gh_comment, "PRR_node");

        assert_eq!(input["body"], serde_json::json!(body));
    }

    #[test]
    fn test_parse_full_database_id_accepts_bigint_string_and_number() {
        assert_eq!(
            parse_full_database_id(&serde_json::json!("2731234567")),
            Some(2_731_234_567)
        );
        assert_eq!(
            parse_full_database_id(&serde_json::json!(2_731_234_567_i64)),
            Some(2_731_234_567)
        );
        assert_eq!(parse_full_database_id(&serde_json::json!("nope")), None);
        assert_eq!(parse_full_database_id(&serde_json::Value::Null), None);
    }

    #[test]
    fn test_parse_pending_comment_result_keeps_the_rest_id_and_flags_pending() {
        let comment = serde_json::json!({
            "fullDatabaseId": "2731234567",
            "url": "https://github.com/owner/repo/pull/1#discussion_r2731234567",
        });

        let result = parse_pending_comment_result(&comment).unwrap();

        assert_eq!(result.comment_id, 2_731_234_567);
        assert_eq!(
            result.comment_url,
            "https://github.com/owner/repo/pull/1#discussion_r2731234567"
        );
        assert_eq!(result.comment_type, "review");
        assert!(result.pending);
    }

    #[test]
    fn test_parse_pending_comment_result_rejects_a_missing_id_or_url() {
        assert!(parse_pending_comment_result(&serde_json::json!({
            "url": "https://github.com/owner/repo/pull/1#discussion_r1",
        }))
        .is_err());
        assert!(
            parse_pending_comment_result(&serde_json::json!({ "fullDatabaseId": "1" })).is_err()
        );
    }

    /// A PR carrying the viewer's draft review alongside somebody else's, which
    /// `states: [PENDING]` does not filter out.
    fn pending_reviews_response() -> serde_json::Value {
        serde_json::json!({
            "repository": { "pullRequest": { "reviews": { "nodes": [
                {
                    "viewerDidAuthor": false,
                    "comments": { "nodes": [
                        { "id": "PRRC_theirs", "fullDatabaseId": "2731000001" },
                    ] },
                },
                {
                    "viewerDidAuthor": true,
                    "comments": { "nodes": [
                        { "id": "PRRC_first", "fullDatabaseId": "2731234567" },
                        { "id": "PRRC_second", "fullDatabaseId": "2731234999" },
                    ] },
                },
            ] } } },
        })
    }

    #[test]
    fn test_find_pending_review_comment_in_response_matches_on_the_rest_id() {
        assert_eq!(
            find_pending_review_comment_in_response(&pending_reviews_response(), 2_731_234_999),
            Some("PRRC_second".to_string())
        );
    }

    #[test]
    fn test_find_pending_review_comment_in_response_ignores_other_authors_drafts() {
        // Editing somebody else's draft comment is not ours to attempt, so the
        // caller should keep its original REST failure instead.
        assert_eq!(
            find_pending_review_comment_in_response(&pending_reviews_response(), 2_731_000_001),
            None
        );
    }

    #[test]
    fn test_find_pending_review_comment_in_response_handles_no_pending_reviews() {
        assert_eq!(
            find_pending_review_comment_in_response(&pending_reviews_response(), 999),
            None
        );
        assert_eq!(
            find_pending_review_comment_in_response(
                &serde_json::json!({
                    "repository": { "pullRequest": { "reviews": { "nodes": [] } } },
                }),
                2_731_234_567
            ),
            None
        );
        assert_eq!(
            find_pending_review_comment_in_response(&serde_json::json!({}), 2_731_234_567),
            None
        );
    }

    #[test]
    fn test_graphql_error_message_joins_messages() {
        // GraphQL reports failures with HTTP 200 and a top-level `errors` array.
        let response = serde_json::json!({
            "data": null,
            "errors": [
                { "message": "Could not resolve to a node with the global id of 'PRR_bogus'." },
                { "type": "FORBIDDEN" },
            ],
        });

        assert_eq!(
            graphql_error_message(&response).unwrap(),
            "Could not resolve to a node with the global id of 'PRR_bogus'.; unknown error"
        );
    }

    #[test]
    fn test_graphql_error_message_ignores_successful_responses() {
        assert_eq!(
            graphql_error_message(&serde_json::json!({ "data": { "repository": null } })),
            None
        );
        assert_eq!(
            graphql_error_message(&serde_json::json!({ "data": {}, "errors": [] })),
            None
        );
    }

    #[test]
    fn test_parse_link_next_finds_the_next_page() {
        let link = "<https://api.github.com/repositories/1/pulls/2/files?per_page=100&page=2>; rel=\"next\", <https://api.github.com/repositories/1/pulls/2/files?per_page=100&page=5>; rel=\"last\"";

        assert_eq!(
            parse_link_next(link),
            Some("https://api.github.com/repositories/1/pulls/2/files?per_page=100&page=2")
        );
    }

    #[test]
    fn test_parse_link_next_returns_none_on_the_last_page() {
        let link = "<https://api.github.com/repositories/1/pulls/2/files?per_page=100&page=4>; rel=\"prev\", <https://api.github.com/repositories/1/pulls/2/files?per_page=100&page=1>; rel=\"first\"";

        assert_eq!(parse_link_next(link), None);
        assert_eq!(parse_link_next(""), None);
    }

    #[test]
    fn test_valid_lines_from_patch_collects_added_and_context_lines() {
        let patch = "@@ -1,3 +1,4 @@\n context\n+added\n-removed\n more context\n\\ No newline at end of file";

        let mut lines: Vec<u32> = valid_lines_from_patch(Some(patch)).into_iter().collect();
        lines.sort_unstable();

        assert_eq!(lines, vec![1, 2, 3]);
        assert!(valid_lines_from_patch(None).is_empty());
    }
}
