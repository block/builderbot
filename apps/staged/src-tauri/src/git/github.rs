//! GitHub integration for fetching pull requests.
//!
//! Uses the GitHub CLI (`gh`) for authentication and API access.
//! Includes caching to minimize API calls.

use super::cli::GitError;
use super::DiffSpec;
use super::GitRef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
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
    pub author: String,
    /// Target branch (e.g., "main")
    pub base_ref: String,
    /// Source branch (e.g., "feature-x") - not useful for forks
    pub head_ref: String,
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

/// Run a gh command in the context of a local repo directory
fn run_gh(repo: &Path, args: &[&str]) -> Result<String, GitError> {
    let gh_path = find_gh().ok_or_else(|| {
        GitError::CommandFailed("GitHub CLI not found. Install with: brew install gh".to_string())
    })?;

    let output = Command::new(&gh_path)
        .current_dir(repo)
        .args(args)
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

    String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8)
}

/// Run a gh command without needing a local directory (uses `-R owner/repo` or global commands).
fn run_gh_global(args: &[&str]) -> Result<String, GitError> {
    let gh_path = find_gh().ok_or_else(|| {
        GitError::CommandFailed("GitHub CLI not found. Install with: brew install gh".to_string())
    })?;

    let output = Command::new(&gh_path)
        .args(args)
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

    String::from_utf8(output.stdout).map_err(|_| GitError::InvalidUtf8)
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

/// List open pull requests for a repo using `-R owner/repo` (no local dir needed).
pub fn list_pull_requests_for_repo(github_repo: &str) -> Result<Vec<PullRequest>, GitError> {
    let output = run_gh_global(&[
        "pr",
        "list",
        "-R",
        github_repo,
        "--state=open",
        "--limit=50",
        "--json=number,title,author,baseRefName,headRefName,isDraft,updatedAt",
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
/// This ensures the files on disk in the bare/main clone reflect the latest
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

    Ok(clone_path)
}

/// Spawn a command and stream its stderr line-by-line to a callback.
///
/// Git (and gh) write progress output to stderr using `\r` for in-place
/// updates, so we split on both `\r` and `\n`.
fn spawn_streaming<F>(program: &str, args: &[&str], on_line: &mut F) -> Result<(), GitError>
where
    F: FnMut(&str),
{
    use std::io::Read;
    use std::process::Stdio;

    let mut child = std::process::Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GitError::CommandFailed(format!("Failed to run {program}: {e}")))?;

    if let Some(stderr) = child.stderr.take() {
        let mut buf = Vec::new();
        for byte in stderr.bytes() {
            let byte = byte.map_err(|e| GitError::CommandFailed(e.to_string()))?;
            if byte == b'\r' || byte == b'\n' {
                if !buf.is_empty() {
                    if let Ok(line) = std::str::from_utf8(&buf) {
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
                on_line(line);
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;
    if !status.success() {
        return Err(GitError::CommandFailed(format!(
            "{} command failed with status {}",
            program, status
        )));
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
    let gh_clone_result = (|| -> Result<(), GitError> {
        let gh_path = find_gh().ok_or_else(|| {
            GitError::CommandFailed(
                "GitHub CLI not found. Install with: brew install gh".to_string(),
            )
        })?;

        let gh_str = gh_path.to_string_lossy().to_string();
        spawn_streaming(
            &gh_str,
            &["repo", "clone", github_repo, &clone_str, "--", "--progress"],
            &mut on_stderr_line,
        )
    })();

    if gh_clone_result.is_ok() && clone_path.join(".git").exists() {
        return Ok(clone_path);
    }

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
            &mut on_stderr_line,
        )?;
    }

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
    author: GhAuthor,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct GhAuthor {
    login: String,
}

impl From<GhPrListItem> for PullRequest {
    fn from(item: GhPrListItem) -> Self {
        PullRequest {
            number: item.number,
            title: item.title,
            author: item.author.login,
            base_ref: item.base_ref_name,
            head_ref: item.head_ref_name,
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
            "--json=number,title,author,baseRefName,headRefName,isDraft,updatedAt",
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
            "--json=number,title,author,baseRefName,headRefName,isDraft,updatedAt",
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

    // Fetch the PR head ref
    let pr_ref = format!("refs/pull/{pr_number}/head");
    cli::run(repo, &["fetch", "origin", &pr_ref])?;

    // Get the SHA of the fetched PR head IMMEDIATELY (before next fetch overwrites FETCH_HEAD)
    let head_sha = cli::run(repo, &["rev-parse", "FETCH_HEAD"])?
        .trim()
        .to_string();

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

/// Result of syncing a review to GitHub.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubSyncResult {
    /// URL to the pending review on GitHub
    pub review_url: String,
    /// Number of comments synced
    pub comment_count: usize,
}

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

/// Request body for creating a review.
#[derive(Debug, Serialize)]
struct CreateReviewRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<String>,
    comments: Vec<GitHubReviewComment>,
}

/// Response from creating a review.
#[derive(Debug, Deserialize)]
struct CreateReviewResponse {
    #[allow(dead_code)]
    id: u64,
    html_url: String,
}

/// A review on GitHub (from list reviews endpoint).
#[derive(Debug, Deserialize)]
struct GitHubReview {
    id: u64,
    state: String,
    user: GhUser,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

/// A comment that couldn't be placed on a specific line (outside the diff).
struct OutOfDiffComment {
    path: String,
    line_info: String,
    content: String,
}

/// Convert a local Comment to a GitHub review comment.
///
/// If `valid_lines` is provided, checks if the comment's lines are within the diff.
/// Returns Err for comments outside the diff (they'll be added to the review body).
fn convert_comment(
    comment: &Comment,
    valid_lines: Option<&std::collections::HashSet<u32>>,
) -> std::result::Result<GitHubReviewComment, OutOfDiffComment> {
    // Convert 0-indexed span to 1-indexed line numbers
    let line = comment.span.end; // end line (1-indexed, since end is exclusive)
    let start_line = comment.span.start + 1; // start line (1-indexed)

    // Check if this line is within the diff
    let line_in_diff = valid_lines
        .map(|lines| lines.contains(&line))
        .unwrap_or(true);

    if line_in_diff {
        // For single-line comments, don't use start_line
        let is_multiline = comment.span.end > comment.span.start + 1;

        Ok(GitHubReviewComment {
            path: comment.path.clone(),
            body: comment.content.clone(),
            line,
            side: "RIGHT", // Always RIGHT since we only support comments on new code
            start_line: if is_multiline { Some(start_line) } else { None },
            start_side: if is_multiline { Some("RIGHT") } else { None },
        })
    } else {
        let line_info = if comment.span.end > comment.span.start + 1 {
            format!("Lines {start_line}-{line}")
        } else {
            format!("Line {line}")
        };

        Err(OutOfDiffComment {
            path: comment.path.clone(),
            line_info,
            content: comment.content.clone(),
        })
    }
}

/// Fetch the valid line numbers for each file in a PR diff.
/// Returns a map of file path -> set of valid line numbers (1-indexed, RIGHT side).
async fn fetch_pr_diff_lines(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<std::collections::HashMap<String, std::collections::HashSet<u32>>, GitError> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/pulls/{pr_number}/files");

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

    #[derive(Deserialize)]
    struct PullRequestFile {
        filename: String,
        patch: Option<String>,
    }

    let files: Vec<PullRequestFile> = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse PR files: {e}")))?;

    let mut result = std::collections::HashMap::new();

    for file in files {
        let mut valid_lines = std::collections::HashSet::new();

        if let Some(patch) = &file.patch {
            // Parse the unified diff to extract valid line numbers
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
        }

        result.insert(file.filename, valid_lines);
    }

    Ok(result)
}

/// Get the current authenticated user's login.
async fn get_current_user(client: &reqwest::Client, token: &str) -> Result<String, GitError> {
    let response = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to get current user: {e}")))?;

    if !response.status().is_success() {
        return Err(GitError::CommandFailed(format!(
            "Failed to get current user: {}",
            response.status()
        )));
    }

    let user: GhUser = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse user response: {e}")))?;

    Ok(user.login)
}

/// Find an existing pending review by the current user.
async fn find_pending_review(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
    username: &str,
) -> Result<Option<GitHubReview>, GitError> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/pulls/{pr_number}/reviews");

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to list reviews: {e}")))?;

    if !response.status().is_success() {
        return Err(GitError::CommandFailed(format!(
            "Failed to list reviews: {}",
            response.status()
        )));
    }

    let reviews: Vec<GitHubReview> = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse reviews: {e}")))?;

    Ok(reviews
        .into_iter()
        .find(|r| r.state == "PENDING" && r.user.login == username))
}

/// Delete a pending review.
async fn delete_pending_review(
    client: &reqwest::Client,
    token: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
    review_id: u64,
) -> Result<(), GitError> {
    let url = format!(
        "https://api.github.com/repos/{owner}/{repo}/pulls/{pr_number}/reviews/{review_id}"
    );

    let response = client
        .delete(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to delete review: {e}")))?;

    if !response.status().is_success() {
        return Err(GitError::CommandFailed(format!(
            "Failed to delete pending review: {}",
            response.status()
        )));
    }

    Ok(())
}

/// Sync local comments to a GitHub PR as a pending review.
///
/// This will:
/// 1. Delete any existing pending review by the current user
/// 2. Create a new pending review with all comments
/// 3. Return the URL to the review
pub async fn sync_review_to_github(
    repo: &Path,
    pr_number: u64,
    comments: &[Comment],
) -> Result<GitHubSyncResult, GitError> {
    if comments.is_empty() {
        return Err(GitError::CommandFailed("No comments to sync".to_string()));
    }

    let token = get_github_token()?;
    let (owner, repo_name) = get_github_repo(repo)?;
    log::info!(
        "Syncing {} comments to GitHub PR #{} in {}/{}",
        comments.len(),
        pr_number,
        owner,
        repo_name
    );
    let client = reqwest::Client::new();

    // Get current user
    let username = get_current_user(&client, &token).await?;

    // Fetch valid diff lines for each file
    let valid_lines_by_file =
        fetch_pr_diff_lines(&client, &token, &owner, &repo_name, pr_number).await?;

    // Check for existing pending review and delete it
    if let Some(existing) =
        find_pending_review(&client, &token, &owner, &repo_name, pr_number, &username).await?
    {
        log::info!("Deleting existing pending review {}", existing.id);
        delete_pending_review(&client, &token, &owner, &repo_name, pr_number, existing.id).await?;
    }

    // Convert comments to GitHub format, checking against valid lines
    let mut gh_comments: Vec<GitHubReviewComment> = Vec::new();
    let mut out_of_diff_comments: Vec<OutOfDiffComment> = Vec::new();

    for comment in comments {
        match convert_comment(comment, valid_lines_by_file.get(&comment.path)) {
            Ok(gh_comment) => gh_comments.push(gh_comment),
            Err(out_of_diff) => out_of_diff_comments.push(out_of_diff),
        }
    }

    let comment_count = gh_comments.len() + out_of_diff_comments.len();

    // Build review body from out-of-diff comments
    let review_body = if out_of_diff_comments.is_empty() {
        None
    } else {
        let mut body = String::from("### Comments on lines outside the diff\n\n");
        for ooc in &out_of_diff_comments {
            body.push_str(&format!(
                "**{}** ({})\n\n{}\n\n---\n\n",
                ooc.path, ooc.line_info, ooc.content
            ));
        }
        Some(body)
    };

    // Create new pending review
    let url = format!("https://api.github.com/repos/{owner}/{repo_name}/pulls/{pr_number}/reviews");

    let request = CreateReviewRequest {
        body: review_body,
        event: None, // None = PENDING
        comments: gh_comments,
    };

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "staged-app")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&request)
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to create review: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(GitError::CommandFailed(format!(
            "Failed to create review: {status} - {error_body}"
        )));
    }

    let review: CreateReviewResponse = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse review response: {e}")))?;

    Ok(GitHubSyncResult {
        review_url: review.html_url,
        comment_count,
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
    state: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
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

    // Analyze status checks
    let mut total = 0u32;
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut pending = 0u32;

    for check in &item.status_check_rollup {
        total += 1;

        // GitHub status checks have different types and fields
        // StatusContext uses 'state', CheckRun uses 'status' and 'conclusion'
        let check_state = if check.typename == "StatusContext" {
            check.state.as_deref()
        } else if check.typename == "CheckRun" {
            // For CheckRun, check conclusion first, then status
            check.conclusion.as_deref().or(check.status.as_deref())
        } else {
            None
        };

        match check_state {
            Some("SUCCESS") | Some("COMPLETED") => passed += 1,
            Some("FAILURE")
            | Some("ERROR")
            | Some("CANCELLED")
            | Some("TIMED_OUT")
            | Some("ACTION_REQUIRED") => failed += 1,
            Some("PENDING") | Some("IN_PROGRESS") | Some("QUEUED") | Some("WAITING") => {
                pending += 1
            }
            _ => pending += 1, // Unknown states treated as pending
        }
    }

    // Determine overall checks state
    let checks_state = if total == 0 {
        "EXPECTED".to_string()
    } else if failed > 0 {
        "FAILURE".to_string()
    } else if pending > 0 {
        "PENDING".to_string()
    } else {
        "SUCCESS".to_string()
    };

    Ok(PrStatus {
        state: item.state.to_uppercase(),
        is_draft: item.is_draft,
        mergeable: item.mergeable.to_uppercase(),
        review_decision: item.review_decision,
        checks_summary: ChecksSummary {
            total,
            passed,
            failed,
            pending,
            state: checks_state,
        },
        head_sha: item.head_ref_oid,
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

    // Analyze status checks (same logic as fetch_pr_status)
    let mut total = 0u32;
    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut pending = 0u32;

    for check in &item.status_check_rollup {
        total += 1;

        let check_state = if check.typename == "StatusContext" {
            check.state.as_deref()
        } else if check.typename == "CheckRun" {
            check.conclusion.as_deref().or(check.status.as_deref())
        } else {
            None
        };

        match check_state {
            Some("SUCCESS") | Some("COMPLETED") => passed += 1,
            Some("FAILURE")
            | Some("ERROR")
            | Some("CANCELLED")
            | Some("TIMED_OUT")
            | Some("ACTION_REQUIRED") => failed += 1,
            Some("PENDING") | Some("IN_PROGRESS") | Some("QUEUED") | Some("WAITING") => {
                pending += 1
            }
            _ => pending += 1,
        }
    }

    let checks_state = if total == 0 {
        "EXPECTED".to_string()
    } else if failed > 0 {
        "FAILURE".to_string()
    } else if pending > 0 {
        "PENDING".to_string()
    } else {
        "SUCCESS".to_string()
    };

    Ok(PrStatus {
        state: item.state.to_uppercase(),
        is_draft: item.is_draft,
        mergeable: item.mergeable.to_uppercase(),
        review_decision: item.review_decision,
        checks_summary: ChecksSummary {
            total,
            passed,
            failed,
            pending,
            state: checks_state,
        },
        head_sha: item.head_ref_oid,
    })
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

    // Build the mutation
    let mut updates = Vec::new();
    if let Some(t) = title {
        updates.push(format!("title: \"{}\"", t.replace('"', "\\\"")));
    }
    if let Some(b) = body {
        updates.push(format!("body: \"{}\"", b.replace('"', "\\\"")));
    }

    // First, get the PR's node ID
    let pr_query = format!(
        r#"query {{
            repository(owner: "{owner}", name: "{repo_name}") {{
                pullRequest(number: {pr_number}) {{
                    id
                }}
            }}
        }}"#
    );

    #[derive(Deserialize)]
    struct PrIdResponse {
        data: PrIdData,
    }

    #[derive(Deserialize)]
    struct PrIdData {
        repository: PrIdRepo,
    }

    #[derive(Deserialize)]
    struct PrIdRepo {
        #[serde(rename = "pullRequest")]
        pull_request: PrIdNode,
    }

    #[derive(Deserialize)]
    struct PrIdNode {
        id: String,
    }

    let response = client
        .post("https://api.github.com/graphql")
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "staged-app")
        .json(&serde_json::json!({ "query": pr_query }))
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to query PR: {e}")))?;

    if !response.status().is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(GitError::CommandFailed(format!(
            "Failed to get PR node ID: {error_body}"
        )));
    }

    let pr_id_response: PrIdResponse = response
        .json()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to parse PR ID: {e}")))?;

    let pr_id = pr_id_response.data.repository.pull_request.id;

    // Now update the PR
    let mutation = format!(
        r#"mutation {{
            updatePullRequest(input: {{
                pullRequestId: "{}"
                {}
            }}) {{
                pullRequest {{
                    id
                }}
            }}
        }}"#,
        pr_id,
        updates.join("\n")
    );

    let response = client
        .post("https://api.github.com/graphql")
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "staged-app")
        .json(&serde_json::json!({ "query": mutation }))
        .send()
        .await
        .map_err(|e| GitError::CommandFailed(format!("Failed to update PR: {e}")))?;

    if !response.status().is_success() {
        let error_body = response.text().await.unwrap_or_default();
        return Err(GitError::CommandFailed(format!(
            "Failed to update PR: {error_body}"
        )));
    }

    Ok(())
}

// =============================================================================
// Subpath Validation
// =============================================================================

/// Validate that a subpath exists as a directory in a GitHub repository.
///
/// Uses the GitHub contents API to check that the path exists and is a
/// directory (the API returns an array for directories). Returns an error
/// if the path does not exist or points to a file rather than a directory.
pub fn validate_subpath_in_repo(github_repo: &str, subpath: &str) -> Result<(), GitError> {
    let trimmed = subpath.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(());
    }

    let endpoint = format!("repos/{github_repo}/contents/{trimmed}");
    match run_gh_global(&["api", &endpoint]) {
        Ok(body) => {
            // The contents API returns a JSON array for directories and a JSON
            // object for files. A quick heuristic: arrays start with '['.
            let body = body.trim_start();
            if body.starts_with('[') {
                Ok(())
            } else {
                Err(GitError::CommandFailed("Invalid path in repo".to_string()))
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Not Found") || msg.contains("HTTP 404") {
                Err(GitError::CommandFailed("Invalid path in repo".to_string()))
            } else {
                Err(e)
            }
        }
    }
}

/// List directories at a given path in a GitHub repository.
/// Returns a list of directory names (not files) at the specified path.
/// If `path` is empty, lists directories at the repository root.
pub fn list_repo_directories(github_repo: &str, path: &str) -> Result<Vec<String>, GitError> {
    let trimmed = path.trim_matches('/');
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
            if msg.contains("Not Found") || msg.contains("HTTP 404") {
                Ok(vec![])
            } else {
                Err(e)
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
}
